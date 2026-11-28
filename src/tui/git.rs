//! Git in the browser: markers beside changed items, the branch, and the diff
//! of the selected file, in the preview pane or full screen.
//!
//! Status and diffs come from `crate::git` on background threads. The status
//! is one call for the whole repository, asked again when its index or HEAD
//! changes or the folder is reloaded. Diffs are cached per file version, and
//! arrive twice: plain first, then with syntax colors.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime};

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Clear, Widget};
use unicode_width::UnicodeWidthChar;

use super::app::{App, Mode};
use super::theme::Theme;
use super::ui::{Ctx, fill, keys, text};
use super::work::Msg;
use crate::doc::code;
use crate::entry::Entry;
use crate::fmt;
use crate::git::{Change, Diff, LineKind, Mark, Repo, Stamp, Status};

/// Syntax colors for each diff line, as (style, text) runs.
pub type Colors = Vec<Vec<(Style, String)>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiffKey {
    path: PathBuf,
    modified: Option<SystemTime>,
    size: u64,
    change: Change,
    reveal: bool,
}

pub struct DiffDoc {
    pub diff: Arc<Diff>,
    pub change: Change,
    /// None until colored, and for plain text.
    pub colors: Option<Colors>,
}

/// Scroll position in the diff of the selected file.
#[derive(Default)]
pub struct DiffView {
    path: PathBuf,
    top: usize,
    left: usize,
    /// Rows and lines from the last draw.
    page: usize,
    len: usize,
    changes: Vec<usize>,
    /// `g` typed, waiting for a second `g`, `d` or `D`.
    g: bool,
}

pub struct Git {
    tx: Sender<Msg>,
    repo: Option<Repo>,
    /// By repository root. None when git gave no answer.
    statuses: HashMap<PathBuf, Option<Arc<Status>>>,
    /// The repository's stamp when its status was last asked for.
    stamps: HashMap<PathBuf, Stamp>,
    /// Roots being read, and whether to read again once done.
    loading: HashMap<PathBuf, bool>,
    diffs: HashMap<DiffKey, Option<Rc<DiffDoc>>>,
    diffs_loading: HashSet<DiffKey>,
    pub view: DiffView,
    /// The last frame had a preview pane or column to show a diff in.
    pub pane: bool,
    /// Show the values in environment files.
    pub reveal: bool,
    /// For `LSD_FRAME_STATS`: when each diff was asked for and whether it
    /// has shown yet, and how long each step took.
    asked: HashMap<DiffKey, (Instant, bool)>,
    timings: Vec<String>,
}

pub enum Diffed {
    /// Not changed, not in a repository, or no text diff.
    No,
    Pending,
    Ready(Rc<DiffDoc>),
}

impl Git {
    pub fn new(tx: Sender<Msg>) -> Git {
        Git {
            tx,
            repo: None,
            statuses: HashMap::new(),
            stamps: HashMap::new(),
            loading: HashMap::new(),
            diffs: HashMap::new(),
            diffs_loading: HashSet::new(),
            view: DiffView::default(),
            pane: false,
            reveal: false,
            asked: HashMap::new(),
            timings: Vec::new(),
        }
    }

    pub fn enter(&mut self, dir: &Path) {
        self.repo = Repo::find(dir);
        self.refresh(false);
    }

    /// Reads the status again when the repository changed since the last
    /// read, or always with `force` (the folder was reloaded).
    pub fn refresh(&mut self, force: bool) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let stamp = repo.stamp();
        let known = self.statuses.contains_key(&repo.root);
        if known && !force && self.stamps.get(&repo.root) == Some(&stamp) {
            return;
        }
        if let Some(again) = self.loading.get_mut(&repo.root) {
            *again = true;
            return;
        }
        self.loading.insert(repo.root.clone(), false);
        self.stamps.insert(repo.root.clone(), stamp);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let status = Status::load(&repo);
            let _ = tx.send(Msg::Status(repo.root, status, started.elapsed()));
        });
    }

    pub fn on_status(&mut self, root: PathBuf, status: Option<Status>, took: Duration) {
        self.timings
            .push(format!("status of {}: {:.1} ms", root.display(), ms(took)));
        let again = self.loading.remove(&root).unwrap_or(false);
        let status = status.map(Arc::new);
        let old = self.statuses.insert(root, status.clone());
        if old.flatten() != status {
            // HEAD may have moved, which changes every diff.
            self.diffs.clear();
        }
        if again {
            self.refresh(true);
        }
    }

    /// The status of the repository holding the current folder.
    pub fn status(&self) -> Option<Arc<Status>> {
        self.statuses.get(&self.repo.as_ref()?.root)?.clone()
    }

    /// The diff of `e`, asked for in the background when not known yet.
    pub fn diff(&mut self, e: &Entry) -> Diffed {
        if e.is_dir {
            return Diffed::No;
        }
        let Some(status) = self.status() else {
            return Diffed::No;
        };
        let Some(change) = status.change(&e.path) else {
            return Diffed::No;
        };
        let key = DiffKey {
            path: e.path.clone(),
            modified: e.modified,
            size: e.size,
            change,
            reveal: self.reveal,
        };
        if let Some(doc) = self.diffs.get(&key) {
            if let Some((at, shown)) = self.asked.get_mut(&key)
                && !*shown
            {
                *shown = true;
                let lines = doc.as_ref().map_or(0, |d| d.diff.lines.len());
                self.timings.push(format!(
                    "diff of {} ({lines} lines) shown after {:.1} ms",
                    e.name,
                    ms(at.elapsed())
                ));
            }
            return match doc {
                Some(doc) if !doc.diff.binary => Diffed::Ready(doc.clone()),
                _ => Diffed::No,
            };
        }
        if self.diffs_loading.insert(key.clone()) {
            self.asked.insert(key.clone(), (Instant::now(), false));
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let Some(mut diff) = Diff::load(&status, &key.path) else {
                    let _ = tx.send(Msg::Diff(key, None));
                    return;
                };
                let name = key.path.file_name().unwrap_or_default().to_string_lossy();
                if !key.reveal && crate::doc::env::is_env(&name) {
                    diff.hide_values();
                }
                let diff = Arc::new(diff);
                let plain = DiffDoc {
                    diff: diff.clone(),
                    change,
                    colors: None,
                };
                if tx
                    .send(Msg::Diff(key.clone(), Some(Box::new(plain))))
                    .is_err()
                    || diff.binary
                {
                    return;
                }
                let colors = colorize(&diff, &key.path);
                let doc = DiffDoc {
                    diff,
                    change,
                    colors: Some(colors),
                };
                let _ = tx.send(Msg::Diff(key, Some(Box::new(doc))));
            });
        }
        Diffed::Pending
    }

    pub fn on_diff(&mut self, key: DiffKey, doc: Option<Box<DiffDoc>>) {
        if !self.diffs_loading.remove(&key) && !self.diffs.contains_key(&key) {
            // Colors for a diff dropped from the cache since.
            return;
        }
        if self.diffs.len() > 64 {
            self.diffs.clear();
        }
        if doc.as_ref().is_some_and(|d| d.colors.is_some())
            && let Some((at, _)) = self.asked.remove(&key)
        {
            self.timings
                .push(format!("  colored after {:.1} ms", ms(at.elapsed())));
        }
        self.diffs.insert(key, doc.map(|d| Rc::new(*d)));
    }

    pub fn report(&self) {
        for t in &self.timings {
            eprintln!("lsd: {t}");
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Syntax colors for the lines of a diff: the new side (context and added
/// lines) and the old side (context and removed lines) are colored as two
/// files, so each reads as code.
fn colorize(diff: &Diff, path: &Path) -> Colors {
    let first = diff
        .lines
        .iter()
        .find(|l| matches!(l.kind, LineKind::Context | LineKind::Added))
        .map_or("", |l| l.text.as_str());
    let syntax = code::syntax_for_path(path, first);
    if syntax.name == "Plain Text" {
        return Vec::new();
    }
    let side = |keep: &[LineKind]| {
        let mut src = String::new();
        for l in diff.lines.iter().filter(|l| keep.contains(&l.kind)) {
            src.push_str(&l.text);
            src.push('\n');
        }
        code::highlight(&src, syntax)
            .map(|line| line.into_iter().map(|s| (s.style, s.text)).collect())
            .collect::<Vec<Vec<_>>>()
            .into_iter()
    };
    let mut new = side(&[LineKind::Context, LineKind::Added]);
    let mut old = side(&[LineKind::Context, LineKind::Removed]);
    diff.lines
        .iter()
        .map(|l| {
            let line = match l.kind {
                LineKind::Context => {
                    old.next();
                    new.next()
                }
                LineKind::Added => new.next(),
                LineKind::Removed => old.next(),
                LineKind::Hunk | LineKind::Note => None,
            };
            line.unwrap_or_default()
        })
        .collect()
}

/// How an item shows in a listing: a letter or dot beside it, and a dimmed
/// name when git ignores it.
pub struct Look {
    pub badge: Option<(&'static str, Style)>,
    pub dim: bool,
}

pub fn look(status: Option<&Status>, path: &Path) -> Look {
    let mark = status.and_then(|s| s.mark(path));
    let color = |c: Change| match c {
        Change::Conflicted | Change::Deleted => Color::Red,
        Change::Untracked | Change::Added => Color::Green,
        Change::Renamed => Color::Cyan,
        Change::Modified | Change::Ignored => Color::Yellow,
    };
    let badge = match mark {
        Some(Mark::Own(Change::Ignored)) | None => None,
        Some(Mark::Own(c)) => {
            let letter = match c {
                Change::Modified => "M",
                Change::Added => "A",
                Change::Untracked => "U",
                Change::Renamed => "R",
                Change::Deleted => "D",
                Change::Conflicted | Change::Ignored => "!",
            };
            let style = Style::new().fg(color(c));
            Some((
                letter,
                if c == Change::Conflicted {
                    style.add_modifier(Modifier::BOLD)
                } else {
                    style
                },
            ))
        }
        Some(Mark::Inside(c)) => Some(("•", Style::new().fg(color(c)))),
    };
    Look {
        badge,
        dim: mark == Some(Mark::Own(Change::Ignored)),
    }
}

/// The branch for the path bar: "main ↑2 ↓1", or the short commit when
/// detached.
pub fn branch(status: &Status) -> Option<String> {
    let mut s = status.branch.label()?;
    let b = &status.branch;
    if b.ahead > 0 {
        s.push_str(&format!(" ↑{}", b.ahead));
    }
    if b.behind > 0 {
        s.push_str(&format!(" ↓{}", b.behind));
    }
    Some(s)
}

fn label(c: Change) -> &'static str {
    match c {
        Change::Modified => "Modified",
        Change::Added => "Added",
        Change::Untracked => "Untracked",
        Change::Renamed => "Renamed",
        Change::Deleted => "Deleted",
        Change::Conflicted => "Conflict",
        Change::Ignored => "Ignored",
    }
}

/// `gd` and `gD`: moves the keys into the diff, in the pane or full screen.
pub fn focus(app: &mut App, full: bool) {
    let Some(e) = app.current().cloned() else {
        return;
    };
    match app.git.diff(&e) {
        Diffed::No if app.git.status().is_none() => app.say("Not in a git repository"),
        Diffed::No if e.is_dir => app.say("Folders have no diff"),
        Diffed::No
            if app
                .git
                .status()
                .is_some_and(|s| s.change(&e.path).is_some()) =>
        {
            app.say("No text diff for this file")
        }
        Diffed::No => app.say("No changes to show"),
        _ => {
            app.mode = Mode::Diff {
                full: full || !app.git.pane,
            };
        }
    }
}

/// The diff of the selected file in place of its preview. False when it has
/// none, so the usual preview shows.
pub fn preview(buf: &mut Buffer, area: Rect, app: &mut App, e: &Entry, cx: &mut Ctx) -> bool {
    let doc = match app.git.diff(e) {
        Diffed::No => return false,
        Diffed::Pending => None,
        Diffed::Ready(doc) => Some(doc),
    };
    if area.width < 8 || area.height < 4 {
        return true;
    }
    let th = cx.th;
    let focused = matches!(app.mode, Mode::Diff { full: false });
    let head = Rect { height: 1, ..area };
    let name = fmt::ellipsize_middle(&e.name, area.width as usize);
    let bold = Style::new().add_modifier(Modifier::BOLD);
    if focused {
        // The selection color shows where the keys go.
        fill(buf, head, th.sel);
    }
    text(
        buf,
        area.x,
        area.y,
        &name,
        area.width,
        if focused { th.sel.patch(bold) } else { bold },
    );
    let Some(doc) = doc else {
        return true;
    };
    let mut y = area.y + 1;
    summary(buf, area.x, y, area.width, &doc, th);
    y += 1;
    if doc.diff.cut {
        text(buf, area.x, y, &too_large(&doc.diff), area.width, th.dim);
        y += 1;
    }
    let body = Rect {
        y: y + 1,
        height: area.bottom().saturating_sub(y + 1),
        ..area
    };
    lines(buf, body, &mut app.git.view, e, &doc, false, th);
    true
}

/// The full-window diff. False when the file has no diff.
pub fn full(buf: &mut Buffer, area: Rect, app: &mut App, e: &Entry, cx: &mut Ctx) -> bool {
    let doc = match app.git.diff(e) {
        Diffed::No => return false,
        Diffed::Pending => None,
        Diffed::Ready(doc) => Some(doc),
    };
    let th = cx.th;
    Clear.render(area, buf);
    let head = Rect { height: 1, ..area };
    let foot = Rect {
        y: area.bottom() - 1,
        height: 1,
        ..area
    };
    fill(buf, head, th.chrome);
    fill(buf, foot, th.chrome);
    let mut right = area.right() - 2;
    if let Some(doc) = &doc {
        let w = summary_width(doc) as u16;
        right = right.saturating_sub(w);
        summary(buf, right, head.y, w, doc, th);
    }
    let name_w = right.saturating_sub(area.x + 4);
    let name = fmt::ellipsize_middle(&e.name, name_w as usize);
    text(
        buf,
        area.x + 2,
        head.y,
        &name,
        name_w,
        Style::new().add_modifier(Modifier::BOLD),
    );
    let pairs = [
        ("j k", "Scroll"),
        ("n N", "Next, previous change"),
        ("h l", "Sideways"),
        ("Enter", "Pane"),
        ("Esc", "Back"),
    ];
    keys(
        buf,
        area.x + 2,
        foot.y,
        area.width.saturating_sub(4),
        &pairs,
        th,
    );

    let Some(doc) = doc else {
        return true;
    };
    let mut body = Rect {
        x: area.x + 1,
        y: area.y + 2,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(4),
    };
    if doc.diff.cut {
        text(
            buf,
            body.x + 1,
            body.y,
            &too_large(&doc.diff),
            body.width,
            th.dim,
        );
        body.y += 2;
        body.height = body.height.saturating_sub(2);
    }
    lines(buf, body, &mut app.git.view, e, &doc, true, th);
    true
}

fn too_large(diff: &Diff) -> String {
    format!(
        "Diff too large, showing the first {} lines",
        diff.lines.len()
    )
}

fn summary_parts(doc: &DiffDoc) -> Vec<String> {
    let d = &doc.diff;
    let mut parts = vec![format!("+{}", d.added), format!("-{}", d.removed)];
    // "Renamed from x" says it already.
    let renamed = d.notes.iter().any(|n| n.starts_with("Renamed"));
    if !renamed {
        parts.push(label(doc.change).to_string());
    }
    parts.extend(d.notes.iter().cloned());
    parts
}

fn summary_width(doc: &DiffDoc) -> usize {
    summary_parts(doc)
        .iter()
        .map(|p| fmt::width(p) + 2)
        .sum::<usize>()
        - 2
}

/// "+12  -3  Modified  Renamed from old.rs"
fn summary(buf: &mut Buffer, x: u16, y: u16, w: u16, doc: &DiffDoc, th: &Theme) {
    let end = x + w;
    let mut x = x;
    for (i, part) in summary_parts(doc).iter().enumerate() {
        let style = match i {
            0 => Style::new().fg(Color::Green),
            1 => Style::new().fg(Color::Red),
            _ => th.dim,
        };
        let part = fmt::ellipsize_end(part, end.saturating_sub(x) as usize);
        x = text(buf, x, y, &part, end.saturating_sub(x), style) + 2;
        if x >= end {
            break;
        }
    }
}

/// Diff lines from the scroll position, with line numbers when `numbers`.
/// Long lines are cut at the edge; `h` and `l` scroll sideways.
fn lines(
    buf: &mut Buffer,
    area: Rect,
    view: &mut DiffView,
    e: &Entry,
    doc: &DiffDoc,
    numbers: bool,
    th: &Theme,
) {
    if view.path != e.path {
        *view = DiffView {
            path: e.path.clone(),
            ..DiffView::default()
        };
    }
    let d = &doc.diff;
    if d.lines.is_empty() {
        text(buf, area.x, area.y, "Content unchanged", area.width, th.dim);
        return;
    }
    view.page = area.height as usize;
    view.len = d.lines.len();
    view.changes.clone_from(&d.changes);
    view.clamp();
    let digits = if numbers {
        let most = d.lines.iter().map(|l| l.old.max(l.new)).max().unwrap_or(0);
        most.max(1).to_string().len()
    } else {
        0
    };
    for (i, y) in (view.top..d.lines.len()).zip(area.top()..area.bottom()) {
        let line = &d.lines[i];
        let row = Rect::new(area.x, y, area.width, 1);
        let (sign, tint, sign_fg) = match line.kind {
            LineKind::Added => ("+", th.add, Color::Green),
            LineKind::Removed => ("-", th.del, Color::Red),
            _ => (" ", Style::new(), Color::Reset),
        };
        fill(buf, row, tint);
        let mut x = area.x;
        if numbers {
            let num = |n: u32| {
                if n == 0 {
                    " ".repeat(digits)
                } else {
                    format!("{n:>digits$}")
                }
            };
            let nums = format!("{} {} ", num(line.old), num(line.new));
            x = text(buf, x, y, &nums, area.width, th.faint.patch(tint));
        }
        match line.kind {
            LineKind::Hunk | LineKind::Note => {
                let style = if line.kind == LineKind::Hunk {
                    th.dim
                } else {
                    th.faint
                };
                let shown = skip(&[(Style::new(), line.text.as_str())], view.left);
                let room = area.right().saturating_sub(x);
                for (_, t) in shown {
                    text(buf, x, y, &t, room, style);
                }
                continue;
            }
            _ => {}
        }
        x = text(buf, x, y, sign, 1, tint.fg(sign_fg));
        if numbers {
            x += 1;
        }
        let colored = doc
            .colors
            .as_ref()
            .and_then(|c| c.get(i))
            .filter(|c| !c.is_empty());
        let spans: Vec<(Style, &str)> = match colored {
            Some(spans) => spans.iter().map(|(s, t)| (*s, t.as_str())).collect(),
            None => vec![(Style::new(), line.text.as_str())],
        };
        for (style, t) in skip(&spans, view.left) {
            if x >= area.right() {
                break;
            }
            x = text(buf, x, y, &t, area.right() - x, style.patch(tint));
        }
    }
}

/// Spans with their first `cols` columns dropped.
fn skip(spans: &[(Style, &str)], cols: usize) -> Vec<(Style, String)> {
    let mut left = cols;
    let mut out = Vec::new();
    for &(style, t) in spans {
        if left == 0 {
            out.push((style, t.to_string()));
            continue;
        }
        let mut rest = t;
        for (i, c) in t.char_indices() {
            if left == 0 {
                break;
            }
            left = left.saturating_sub(c.width().unwrap_or(0));
            rest = &t[i + c.len_utf8()..];
        }
        if !rest.is_empty() && left == 0 {
            out.push((style, rest.to_string()));
        }
    }
    out
}

impl DiffView {
    fn clamp(&mut self) {
        self.top = self.top.min(self.len.saturating_sub(self.page));
    }

    pub fn scroll(&mut self, by: isize) {
        self.clamp();
        self.top = self.top.saturating_add_signed(by);
        self.clamp();
    }

    /// Puts the next or previous run of changes near the top, with a few
    /// lines of context above it.
    fn jump(&mut self, back: bool) {
        const ABOVE: usize = 3;
        self.clamp();
        let here = self.top + ABOVE;
        let to = if back {
            self.changes.iter().rev().find(|&&c| c < here)
        } else {
            self.changes.iter().find(|&&c| c > here)
        };
        if let Some(&c) = to {
            self.top = c.saturating_sub(ABOVE);
            self.clamp();
        }
    }
}

/// Keys while the diff has focus.
pub fn key(app: &mut App, k: KeyEvent) {
    use KeyCode::*;
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let full = matches!(app.mode, Mode::Diff { full: true });
    let v = &mut app.git.view;
    let g = std::mem::take(&mut v.g);
    let half = (v.page / 2).max(1) as isize;
    let page = v.page.max(1) as isize;
    match (k.code, ctrl) {
        (Char('g'), false) if !g => v.g = true,
        (Char('g'), false) | (Home, _) => v.top = 0,
        (Char('G'), false) | (End, _) => v.top = usize::MAX,
        (Char('j'), false) | (Down, _) | (Char('n' | 'e'), true) => v.scroll(1),
        (Char('k'), false) | (Up, _) | (Char('p' | 'y'), true) => v.scroll(-1),
        (Char('d'), true) => v.scroll(half),
        (Char('u'), true) => v.scroll(-half),
        (Char('f'), true) | (PageDown, _) | (Char(' '), false) => v.scroll(page),
        (Char('b'), true) | (PageUp, _) => v.scroll(-page),
        (Char('n' | ']'), false) => v.jump(false),
        (Char('N' | '['), false) => v.jump(true),
        (Char('h'), false) | (Left, _) => v.left = v.left.saturating_sub(8),
        (Char('l'), false) | (Right, _) => v.left += 8,
        (Char('0' | '^'), false) => v.left = 0,
        // `gD` from the pane goes full screen, and back.
        (Char('D'), false) if g => {
            app.mode = Mode::Diff {
                full: !full || !app.git.pane,
            }
        }
        (Enter, _) => {
            app.mode = Mode::Diff {
                full: !full || !app.git.pane,
            }
        }
        (Char('d'), false) if g => app.mode = Mode::Normal,
        (Esc, _) | (Char('q'), false) => app.mode = Mode::Normal,
        (Char('?'), false) => app.mode = Mode::Help,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_columns_across_spans() {
        let s = Style::new();
        let spans = [(s, "ab"), (s, "cdef")];
        let texts = |v: Vec<(Style, String)>| v.into_iter().map(|(_, t)| t).collect::<Vec<_>>();
        assert_eq!(texts(skip(&spans, 0)), ["ab", "cdef"]);
        assert_eq!(texts(skip(&spans, 3)), ["def"]);
        assert_eq!(texts(skip(&spans, 9)), Vec::<String>::new());
    }

    #[test]
    fn jumps_between_changes() {
        let mut v = DiffView {
            page: 10,
            len: 100,
            changes: vec![1, 20, 50, 95],
            ..DiffView::default()
        };
        v.jump(false);
        assert_eq!(v.top, 17);
        v.jump(false);
        assert_eq!(v.top, 47);
        // The last change sits at the bottom of the scroll.
        v.jump(false);
        assert_eq!(v.top, 90);
        v.jump(true);
        assert_eq!(v.top, 47);
    }

    #[test]
    fn colors_each_side_as_code() {
        let text = "@@ -1,2 +1,2 @@\n fn main() {\n-    let a = 1;\n+    let b = \"x\";\n";
        let diff = Diff::parse(text, 100);
        let colors = colorize(&diff, Path::new("main.rs"));
        assert_eq!(colors.len(), diff.lines.len());
        assert!(colors[0].is_empty(), "hunk headers stay plain");
        let fg = |line: usize, t: &str| {
            colors[line]
                .iter()
                .find(|(_, s)| s.contains(t))
                .and_then(|(style, _)| style.fg)
        };
        assert_eq!(fg(1, "fn"), Some(Color::Indexed(5)));
        assert_eq!(fg(2, "let"), Some(Color::Indexed(5)));
        assert_eq!(fg(3, "\"x"), Some(Color::Indexed(2)));
    }
}
