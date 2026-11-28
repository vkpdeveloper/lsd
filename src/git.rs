//! Git, through the `git` command: the status of a whole repository from one
//! `git status --porcelain=v2 -z` call, and the diff of one file.
//!
//! Every call has a timeout and a size cap, and any failure (no repository,
//! no `git`, a repository too big to answer in time) gives None, so callers
//! simply show nothing. Nothing here runs on the UI thread.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

/// A working tree found by looking for `.git` in a folder or above it. No
/// process runs for folders outside a repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    pub root: PathBuf,
    git_dir: PathBuf,
}

/// When these files change, the status may have too: the index (staging,
/// commits), HEAD (checkouts) and its reflog (commits, resets).
pub type Stamp = [Option<SystemTime>; 3];

impl Repo {
    pub fn find(dir: &Path) -> Option<Repo> {
        for root in dir.ancestors() {
            let dot = root.join(".git");
            let Ok(meta) = std::fs::metadata(&dot) else {
                continue;
            };
            let git_dir = if meta.is_dir() {
                dot
            } else {
                // A linked worktree or submodule: `gitdir: <path>`.
                let text = std::fs::read_to_string(&dot).ok()?;
                root.join(text.strip_prefix("gitdir:")?.trim())
            };
            return Some(Repo {
                root: root.to_path_buf(),
                git_dir,
            });
        }
        None
    }

    pub fn stamp(&self) -> Stamp {
        let at = |p: &str| {
            std::fs::metadata(self.git_dir.join(p))
                .and_then(|m| m.modified())
                .ok()
        };
        [at("index"), at("HEAD"), at("logs/HEAD")]
    }
}

/// What happened to a path, least to most important. A folder shows the
/// most important change inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Change {
    Ignored,
    Untracked,
    Added,
    Renamed,
    Deleted,
    Modified,
    Conflicted,
}

/// How an item in a listing relates to git.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// The item itself changed (or is untracked or ignored).
    Own(Change),
    /// A folder with changes somewhere inside.
    Inside(Change),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Branch {
    /// None when HEAD is detached.
    pub name: Option<String>,
    /// None before the first commit.
    pub oid: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

impl Branch {
    /// The branch name, or the short commit when detached.
    pub fn label(&self) -> Option<String> {
        self.name
            .clone()
            .or_else(|| self.oid.as_ref().map(|o| o[..o.len().min(7)].to_string()))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub root: PathBuf,
    pub branch: Branch,
    /// Absolute paths. Untracked and ignored folders git did not look into
    /// are here too, and everything below them inherits their change.
    paths: HashMap<PathBuf, Change>,
    /// Every folder with a change below it, and the most important one.
    folders: HashMap<PathBuf, Change>,
    /// New path to old path, for renames.
    renames: HashMap<PathBuf, PathBuf>,
}

const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const STATUS_MAX_BYTES: usize = 64 << 20;

impl Status {
    /// Runs `git status` for the whole repository.
    pub fn load(repo: &Repo) -> Option<Status> {
        // `--ignored=matching` names ignored folders without listing them,
        // and the user's own setting for untracked files is kept.
        let args = [
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--ignored=matching",
        ];
        let out = run(
            &repo.root,
            &args.map(OsStr::new),
            STATUS_TIMEOUT,
            STATUS_MAX_BYTES,
        )?;
        if out.cut || out.code != 0 {
            return None;
        }
        Some(Status::parse(&repo.root, &out.bytes))
    }

    pub fn parse(root: &Path, out: &[u8]) -> Status {
        let mut s = Status {
            root: root.to_path_buf(),
            ..Status::default()
        };
        let mut records = out.split(|&b| b == 0).map(String::from_utf8_lossy);
        while let Some(rec) = records.next() {
            let (kind, rest) = rec.split_once(' ').unwrap_or((&rec, ""));
            let (change, path) = match kind {
                "#" => {
                    s.header(rest);
                    continue;
                }
                // `1 XY sub mH mI mW hH hI path`
                "1" => {
                    let xy = rest.as_bytes();
                    let change = if xy.first() == Some(&b'A') {
                        Change::Added
                    } else if xy.contains(&b'D') {
                        Change::Deleted
                    } else {
                        Change::Modified
                    };
                    (change, nth_field(rest, 7))
                }
                // `2 XY sub mH mI mW hH hI Xscore path`, then the old path.
                "2" => {
                    let path = nth_field(rest, 8);
                    if let Some(from) = records.next() {
                        s.renames.insert(root.join(path), root.join(from.as_ref()));
                    }
                    (Change::Renamed, path)
                }
                // `u XY sub m1 m2 m3 mW h1 h2 h3 path`
                "u" => (Change::Conflicted, nth_field(rest, 9)),
                "?" => (Change::Untracked, rest),
                "!" => (Change::Ignored, rest),
                _ => continue,
            };
            let path = root.join(path.trim_end_matches('/'));
            if change != Change::Ignored {
                for dir in path.ancestors().skip(1) {
                    if !dir.starts_with(root) {
                        break;
                    }
                    let most = s.folders.entry(dir.to_path_buf()).or_insert(change);
                    *most = (*most).max(change);
                }
            }
            s.paths.insert(path, change);
        }
        s
    }

    fn header(&mut self, line: &str) {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "branch.oid" if value != "(initial)" => self.branch.oid = Some(value.into()),
            "branch.head" if value != "(detached)" => self.branch.name = Some(value.into()),
            "branch.ab" => {
                for n in value.split(' ') {
                    if let Some(a) = n.strip_prefix('+') {
                        self.branch.ahead = a.parse().unwrap_or(0);
                    } else if let Some(b) = n.strip_prefix('-') {
                        self.branch.behind = b.parse().unwrap_or(0);
                    }
                }
            }
            _ => {}
        }
    }

    /// The mark for an item at `path`, if any.
    pub fn mark(&self, path: &Path) -> Option<Mark> {
        if let Some(&c) = self.paths.get(path) {
            return Some(Mark::Own(c));
        }
        if let Some(&c) = self.folders.get(path) {
            return Some(Mark::Inside(c));
        }
        // Inside a folder that is untracked or ignored as a whole.
        path.ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(&self.root) && *dir != self.root)
            .find_map(|dir| match self.paths.get(dir) {
                Some(&c @ (Change::Untracked | Change::Ignored)) => Some(Mark::Own(c)),
                _ => None,
            })
    }

    /// The file's own change, when it has a diff worth showing.
    pub fn change(&self, path: &Path) -> Option<Change> {
        match self.mark(path)? {
            Mark::Own(Change::Ignored) | Mark::Inside(_) => None,
            Mark::Own(c) => Some(c),
        }
    }
}

/// `n` space-separated fields in, the rest of the record (paths may hold spaces).
fn nth_field(rest: &str, n: usize) -> &str {
    rest.splitn(n + 1, ' ').nth(n).unwrap_or("")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// `@@ -1,3 +1,4 @@`
    Hunk,
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file`
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line numbers in the old and new file, 0 where the line is not in it.
    pub old: u32,
    pub new: u32,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub lines: Vec<DiffLine>,
    /// Where each run of added or removed lines starts, for jumping.
    pub changes: Vec<usize>,
    pub added: usize,
    pub removed: usize,
    pub binary: bool,
    /// Facts from the header: a rename, a new file, a mode change.
    pub notes: Vec<String>,
    /// Set when the diff was too large and only its start is here.
    pub cut: bool,
}

/// Diffs longer than this show only their start.
pub const MAX_DIFF_LINES: usize = 20_000;
const DIFF_MAX_BYTES: usize = 8 << 20;
const DIFF_TIMEOUT: Duration = Duration::from_secs(5);

impl Diff {
    /// The working tree file against HEAD, so staged and unstaged changes
    /// both show. Untracked files, and files before the first commit, show
    /// as all added.
    pub fn load(status: &Status, path: &Path) -> Option<Diff> {
        let change = status.change(path)?;
        let rel = path.strip_prefix(&status.root).ok()?;
        let mut args: Vec<&OsStr> = vec!["diff".as_ref()];
        let whole_file =
            change == Change::Untracked || (change == Change::Added && status.branch.oid.is_none());
        if whole_file {
            args.extend(
                [
                    "--no-index",
                    "--no-color",
                    "--no-ext-diff",
                    "--",
                    "/dev/null",
                ]
                .map(OsStr::new),
            );
            args.push(rel.as_os_str());
        } else {
            args.extend(["HEAD", "--no-color", "--no-ext-diff", "-M", "--"].map(OsStr::new));
            // Both names, so git pairs them up as a rename.
            if let Some(from) = status.renames.get(path)
                && let Ok(from) = from.strip_prefix(&status.root)
            {
                args.push(from.as_os_str());
            }
            args.push(rel.as_os_str());
        }
        let out = run(&status.root, &args, DIFF_TIMEOUT, DIFF_MAX_BYTES)?;
        // `--no-index` exits with 1 when the files differ.
        if out.code > 1 {
            return None;
        }
        let mut text = String::from_utf8_lossy(&out.bytes).into_owned();
        if out.cut {
            // The last line may be partial.
            text.truncate(text.rfind('\n').unwrap_or(0));
        }
        let mut diff = Diff::parse(&text, MAX_DIFF_LINES);
        diff.cut |= out.cut;
        Some(diff)
    }

    /// Parses unified diff output, keeping at most `max` lines.
    pub fn parse(text: &str, max: usize) -> Diff {
        let mut d = Diff::default();
        let (mut old, mut new) = (0, 0);
        let mut in_hunk = false;
        let mut prev = LineKind::Hunk;
        for raw in text.lines() {
            if raw.starts_with("diff --git ") {
                in_hunk = false;
                continue;
            }
            if raw.starts_with("@@") {
                (old, new) = hunk_start(raw);
                in_hunk = true;
            } else if !in_hunk {
                d.header(raw);
                continue;
            }
            if d.lines.len() >= max {
                d.cut = true;
                break;
            }
            let (kind, body) = match raw.as_bytes().first() {
                _ if raw.starts_with("@@") => (LineKind::Hunk, raw),
                Some(b'+') => (LineKind::Added, &raw[1..]),
                Some(b'-') => (LineKind::Removed, &raw[1..]),
                Some(b'\\') => (LineKind::Note, raw),
                _ => (LineKind::Context, raw.get(1..).unwrap_or("")),
            };
            let (o, n) = match kind {
                LineKind::Context => {
                    old += 1;
                    new += 1;
                    (old - 1, new - 1)
                }
                LineKind::Added => {
                    new += 1;
                    d.added += 1;
                    (0, new - 1)
                }
                LineKind::Removed => {
                    old += 1;
                    d.removed += 1;
                    (old - 1, 0)
                }
                LineKind::Hunk | LineKind::Note => (0, 0),
            };
            let changed = matches!(kind, LineKind::Added | LineKind::Removed);
            if changed && !matches!(prev, LineKind::Added | LineKind::Removed) {
                d.changes.push(d.lines.len());
            }
            if kind != LineKind::Note {
                prev = kind;
            }
            d.lines.push(DiffLine {
                kind,
                old: o,
                new: n,
                text: body.replace('\t', "    "),
            });
        }
        d
    }

    fn header(&mut self, line: &str) {
        if let Some(from) = line.strip_prefix("rename from ") {
            self.notes.push(format!("Renamed from {from}"));
        } else if let Some(mode) = line.strip_prefix("new mode ") {
            self.notes.push(format!("Mode changed to {mode}"));
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            self.binary = true;
        }
    }
}

/// Where a hunk starts in the old and new file: `@@ -12,5 +12,7 @@`.
fn hunk_start(line: &str) -> (u32, u32) {
    let mut parts = line.split(' ').skip(1);
    let mut num = |sign: char| {
        parts
            .next()
            .and_then(|p| p.strip_prefix(sign))
            .and_then(|p| p.split(',').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };
    let old = num('-');
    (old, num('+'))
}

struct Output {
    bytes: Vec<u8>,
    code: i32,
    /// Output stopped at the size cap.
    cut: bool,
}

/// Runs git in `dir`, reading at most `max` bytes of output and waiting at
/// most `timeout`. None when git is missing, fails to start or takes too long.
fn run(dir: &Path, args: &[&OsStr], timeout: Duration, max: usize) -> Option<Output> {
    let mut child = Command::new("git")
        // Never take the index lock: the user's own git commands come first.
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut stdout).take(max as u64 + 1).read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let Ok(mut bytes) = rx.recv_timeout(timeout) else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let cut = bytes.len() > max;
    if cut {
        bytes.truncate(max);
        let _ = child.kill();
    }
    let code = child.wait().ok()?.code().unwrap_or(-1);
    Some(Output {
        bytes,
        // Killed on purpose: what we read is still good.
        code: if cut { 0 } else { code },
        cut,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/repo";

    fn status(records: &[&str]) -> Status {
        Status::parse(Path::new(ROOT), records.join("\0").as_bytes())
    }

    fn at(p: &str) -> PathBuf {
        Path::new(ROOT).join(p)
    }

    #[test]
    fn parses_porcelain_v2() {
        let s = status(&[
            "# branch.oid 68469d9463432613cca78a66b89413375f7627bb",
            "# branch.head main",
            "# branch.ab +2 -1",
            "1 .M N... 100644 100644 100644 de98 de98 src/my file.rs",
            "1 A. N... 000000 100644 100644 0000 3e75 staged.txt",
            "1 AM N... 000000 100644 100644 0000 3e75 staged and edited.txt",
            "1 .D N... 100644 100644 000000 587b 587b gone.txt",
            "2 R. N... 100644 100644 100644 c96f c96f R100 new name.txt",
            "old name.txt",
            "u UU N... 100644 100644 100644 100644 3281 351b e45c c.txt",
            "? newdir/",
            "? docs/notes.md",
            "! target/",
            "",
        ]);
        assert_eq!(s.branch.name.as_deref(), Some("main"));
        assert_eq!((s.branch.ahead, s.branch.behind), (2, 1));
        let own = |p: &str| s.mark(&at(p));
        assert_eq!(own("src/my file.rs"), Some(Mark::Own(Change::Modified)));
        assert_eq!(own("staged.txt"), Some(Mark::Own(Change::Added)));
        assert_eq!(own("staged and edited.txt"), Some(Mark::Own(Change::Added)));
        assert_eq!(own("gone.txt"), Some(Mark::Own(Change::Deleted)));
        assert_eq!(own("new name.txt"), Some(Mark::Own(Change::Renamed)));
        assert_eq!(
            s.renames.get(&at("new name.txt")),
            Some(&at("old name.txt"))
        );
        assert_eq!(own("c.txt"), Some(Mark::Own(Change::Conflicted)));
        assert_eq!(own("README.md"), None);
        // Folders show what is inside; untracked and ignored ones pass it down.
        assert_eq!(own("src"), Some(Mark::Inside(Change::Modified)));
        assert_eq!(own("docs"), Some(Mark::Inside(Change::Untracked)));
        assert_eq!(own("newdir"), Some(Mark::Own(Change::Untracked)));
        assert_eq!(own("newdir/a/b.txt"), Some(Mark::Own(Change::Untracked)));
        assert_eq!(own("target/debug/lsd"), Some(Mark::Own(Change::Ignored)));
        assert_eq!(s.change(&at("target/debug/lsd")), None);
        // The root holds everything but ignored files; a conflict wins.
        assert_eq!(
            s.mark(Path::new(ROOT)),
            Some(Mark::Inside(Change::Conflicted))
        );
    }

    #[test]
    fn detached_and_initial_heads() {
        let s = status(&[
            "# branch.oid 68469d9463432613cca7",
            "# branch.head (detached)",
        ]);
        assert_eq!(s.branch.label().as_deref(), Some("68469d9"));
        let s = status(&["# branch.oid (initial)", "# branch.head main"]);
        assert_eq!(s.branch.oid, None);
        assert_eq!(s.branch.label().as_deref(), Some("main"));
    }

    const DIFF: &str = "\
diff --git a/old.rs b/new.rs
similarity index 90%
rename from old.rs
rename to new.rs
index 1111111..2222222 100644
--- a/old.rs
+++ b/new.rs
@@ -10,4 +10,5 @@ fn main() {
 let a = 1;
--- a removed line that looks like a header
+let b = 2;
+\tlet c = 3;
 let d = 4;
@@ -40,2 +41,2 @@
-old
+new
\\ No newline at end of file
";

    #[test]
    fn parses_a_unified_diff() {
        let d = Diff::parse(DIFF, MAX_DIFF_LINES);
        assert_eq!(d.notes, ["Renamed from old.rs"]);
        assert_eq!((d.added, d.removed), (3, 2));
        let kinds: Vec<LineKind> = d.lines.iter().map(|l| l.kind).collect();
        use LineKind::*;
        assert_eq!(
            kinds,
            [
                Hunk, Context, Removed, Added, Added, Context, Hunk, Removed, Added, Note
            ]
        );
        assert_eq!(
            d.lines[2].text,
            "-- a removed line that looks like a header"
        );
        assert_eq!(d.lines[4].text, "    let c = 3;");
        assert_eq!((d.lines[1].old, d.lines[1].new), (10, 10));
        assert_eq!((d.lines[4].old, d.lines[4].new), (0, 12));
        assert_eq!((d.lines[5].old, d.lines[5].new), (12, 13));
        assert_eq!((d.lines[7].old, d.lines[8].new), (40, 41));
        assert_eq!(d.changes, [2, 7]);
        assert!(!d.cut);
    }

    #[test]
    fn caps_long_diffs_and_spots_binaries() {
        let d = Diff::parse(DIFF, 4);
        assert_eq!(d.lines.len(), 4);
        assert!(d.cut);
        let bin = "diff --git a/a.png b/a.png\nindex 1..2 100644\nBinary files a/a.png and b/a.png differ\n";
        assert!(Diff::parse(bin, MAX_DIFF_LINES).binary);
    }

    /// A real repository: every kind of change, then the diffs lsd asks for.
    #[test]
    fn status_and_diffs_from_a_real_repo() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let tmp = crate::fsops::tests::TempDir::new();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        let write = |p: &str, text: &str| {
            let path = root.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        git(&["init", "-q", "-b", "main"]);
        write("a.txt", "one\ntwo\nthree\n");
        write("old.txt", "a\nb\nc\nd\n");
        write("sub/s.txt", "s\n");
        write(".gitignore", "*.log\n");
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);
        write("a.txt", "one\n2\nthree\nfour\n");
        git(&["mv", "old.txt", "new.txt"]);
        write("sub/s.txt", "s\nmore\n");
        write("fresh.txt", "hello\n");
        write("x.log", "log\n");

        let repo = Repo::find(&root.join("sub")).unwrap();
        assert_eq!(repo.root, root);
        let s = Status::load(&repo).unwrap();
        assert_eq!(s.branch.label().as_deref(), Some("main"));
        assert_eq!(
            s.mark(&root.join("sub")),
            Some(Mark::Inside(Change::Modified))
        );
        assert_eq!(
            s.mark(&root.join("x.log")),
            Some(Mark::Own(Change::Ignored))
        );

        let d = Diff::load(&s, &root.join("a.txt")).unwrap();
        assert_eq!((d.added, d.removed), (2, 1));
        let d = Diff::load(&s, &root.join("fresh.txt")).unwrap();
        assert_eq!((d.added, d.removed), (1, 0));
        assert_eq!(d.lines.last().unwrap().text, "hello");
        let d = Diff::load(&s, &root.join("new.txt")).unwrap();
        assert_eq!(d.notes, ["Renamed from old.txt"]);
        assert!(Diff::load(&s, &root.join("x.log")).is_none());

        // Outside any repository, nothing runs at all.
        assert_eq!(Repo::find(Path::new("/")), None);
    }
}
