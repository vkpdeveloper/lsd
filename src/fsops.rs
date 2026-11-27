//! File operations used by the browser, with Finder semantics: nothing is ever
//! overwritten, and name clashes get Finder names instead.

mod trash;

use std::ffi::{CString, OsStr, OsString};
use std::fs::{self, File, FileTimes, Metadata};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{OpenOptionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Moves items to the Trash (macOS) or the freedesktop trash (Linux).
pub fn trash(paths: &[PathBuf]) -> io::Result<()> {
    for path in paths {
        trash::trash_item(path).map_err(|err| about(path, err))?;
    }
    Ok(())
}

/// Copies items into `dest_dir`. Copying into the item's own folder gives
/// Finder names ("a copy.txt", "a copy 2.txt"); a clash in another folder
/// gives "a 2.txt". Folders are copied recursively, keeping permissions and
/// modified times, and symlinks are copied as links. Returns the new paths.
pub fn copy_into(srcs: &[PathBuf], dest_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let dest_real = fs::canonicalize(dest_dir)?;
    srcs.iter()
        .map(|src| copy_one(src, dest_dir, &dest_real).map_err(|err| about(src, err)))
        .collect()
}

/// Moves items into `dest_dir`, renaming on clashes ("a 2.txt"). Returns the new paths.
/// An item already in `dest_dir` stays where it is.
pub fn move_into(srcs: &[PathBuf], dest_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let dest_real = fs::canonicalize(dest_dir)?;
    srcs.iter()
        .map(|src| move_one(src, dest_dir, &dest_real).map_err(|err| about(src, err)))
        .collect()
}

/// Renames in place. Refuses to overwrite an existing item.
pub fn rename(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    if new_name.is_empty() || new_name == "." || new_name == ".." || new_name.contains(['/', '\0'])
    {
        return Err(invalid("That name can't be used."));
    }
    let Some(old_name) = path.file_name() else {
        return Err(invalid("This item can't be renamed."));
    };
    if old_name == new_name {
        return Ok(path.to_path_buf());
    }
    let dest = path.with_file_name(new_name);
    match rename_noreplace(path, &dest) {
        // On some case-insensitive volumes (exFAT, FAT) "a.txt" -> "A.txt"
        // clashes with itself. That is a rename, not an overwrite.
        Err(err)
            if err.kind() == io::ErrorKind::AlreadyExists
                && old_name.to_string_lossy().to_lowercase() == new_name.to_lowercase()
                && same_file(path, &dest) =>
        {
            fs::rename(path, &dest)?
        }
        result => result?,
    }
    Ok(dest)
}

/// Creates "untitled folder" (or "untitled folder 2", ...) in `dir`.
pub fn new_folder(dir: &Path) -> io::Result<PathBuf> {
    first_free(
        dir,
        |n| numbered(b"untitled folder", b"", n),
        |dest| fs::create_dir(dest),
    )
}

/// Opens with the default app, detached and silent.
pub fn open(path: &Path) -> io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    // Absolute, so a name starting with "-" is never read as an option.
    launch(Command::new(opener).arg(std::path::absolute(path)?))
}

/// Shows the item selected in Finder (macOS) or the system file manager (Linux).
pub fn reveal(path: &Path) -> io::Result<()> {
    let path = std::path::absolute(path)?;
    #[cfg(target_os = "macos")]
    {
        launch(Command::new("open").arg("-R").arg(path))
    }
    #[cfg(not(target_os = "macos"))]
    {
        reveal_freedesktop(&path)
    }
}

fn copy_one(src: &Path, dest_dir: &Path, dest_real: &Path) -> io::Result<PathBuf> {
    let (src_real, name) = locate(src)?;
    let meta = fs::symlink_metadata(src)?;
    if meta.is_dir() && dest_real.starts_with(&src_real) {
        return Err(invalid("A folder can't be copied into itself."));
    }
    let (stem, ext) = split_name(name, meta.is_dir());
    let copy = |dest: &Path| copy_item(src, dest, &meta);
    if src_real.parent() == Some(dest_real) {
        first_free(dest_dir, |n| copy_name(stem, ext, n), copy)
    } else {
        first_free(dest_dir, |n| numbered(stem, ext, n), copy)
    }
}

fn move_one(src: &Path, dest_dir: &Path, dest_real: &Path) -> io::Result<PathBuf> {
    let (src_real, name) = locate(src)?;
    // Like dropping an item on the folder it is already in: nothing to do.
    if src_real.parent() == Some(dest_real) {
        return Ok(src.to_path_buf());
    }
    let meta = fs::symlink_metadata(src)?;
    if meta.is_dir() && dest_real.starts_with(&src_real) {
        return Err(invalid("A folder can't be moved into itself."));
    }
    let (stem, ext) = split_name(name, meta.is_dir());
    first_free(
        dest_dir,
        |n| numbered(stem, ext, n),
        |dest| match rename_noreplace(src, dest) {
            Err(err) if err.kind() == io::ErrorKind::CrossesDevices => {
                copy_item(src, dest, &meta)?;
                if meta.is_dir() {
                    fs::remove_dir_all(src)
                } else {
                    fs::remove_file(src)
                }
            }
            result => result,
        },
    )
}

/// The item's real path (its folder resolved, the item itself not followed,
/// so a symlink stays a symlink) and its name.
fn locate(path: &Path) -> io::Result<(PathBuf, &OsStr)> {
    let name = path
        .file_name()
        .ok_or_else(|| invalid("This item can't be moved or copied."))?;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    Ok((fs::canonicalize(parent)?.join(name), name))
}

/// Tries `dir/name(1)`, `dir/name(2)`, ... until `create` succeeds. `create`
/// must fail with `AlreadyExists` instead of replacing anything, which makes
/// choosing the name and creating the item a single atomic step.
fn first_free(
    dir: &Path,
    name: impl Fn(u32) -> OsString,
    mut create: impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    let mut n = 1;
    loop {
        let dest = dir.join(name(n));
        match create(&dest) {
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => n += 1,
            result => return result.map(|()| dest),
        }
    }
}

/// Splits "photo.jpg" into ("photo", ".jpg"). Folders keep their whole name,
/// since a dot in a folder name ("v1.2", "node-18.0") is rarely an extension.
fn split_name(name: &OsStr, is_dir: bool) -> (&[u8], &[u8]) {
    let bytes = name.as_bytes();
    match bytes.iter().rposition(|&b| b == b'.') {
        // A leading dot (".bashrc") starts the name, not an extension.
        Some(dot) if dot > 0 && !is_dir => bytes.split_at(dot),
        _ => (bytes, &[]),
    }
}

/// "a.txt", "a 2.txt", "a 3.txt", ...
fn numbered(stem: &[u8], ext: &[u8], n: u32) -> OsString {
    let mut name = stem.to_vec();
    if n > 1 {
        name.extend_from_slice(format!(" {n}").as_bytes());
    }
    name.extend_from_slice(ext);
    OsString::from_vec(name)
}

/// "a copy.txt", "a copy 2.txt", ... Copying "a copy.txt" continues the
/// sequence ("a copy 2.txt") instead of giving "a copy copy.txt", like Finder.
fn copy_name(stem: &[u8], ext: &[u8], n: u32) -> OsString {
    let base = strip_copy_suffix(stem);
    let mut name = base.to_vec();
    name.extend_from_slice(b" copy");
    numbered(&name, ext, n)
}

/// "a copy" and "a copy 7" -> "a"; anything else is returned unchanged.
fn strip_copy_suffix(stem: &[u8]) -> &[u8] {
    let digits = stem.iter().rev().take_while(|b| b.is_ascii_digit()).count();
    let rest = &stem[..stem.len() - digits];
    let rest = if digits > 0 {
        match rest.strip_suffix(b" ") {
            Some(rest) => rest,
            None => return stem,
        }
    } else {
        rest
    };
    match rest.strip_suffix(b" copy") {
        Some(base) if !base.is_empty() => base,
        _ => stem,
    }
}

/// Copies `src` to `dest`, which must not exist yet; if it does, fails with
/// `AlreadyExists` and leaves it alone. A folder that fails partway is removed.
fn copy_item(src: &Path, dest: &Path, meta: &Metadata) -> io::Result<()> {
    if !meta.is_dir() {
        return copy_leaf(src, dest, meta);
    }
    fs::create_dir(dest)?;
    copy_dir(src, dest, meta).inspect_err(|_| {
        let _ = fs::remove_dir_all(dest);
    })
}

/// Fills the new, empty folder `dest` with the contents of `src`.
fn copy_dir(src: &Path, dest: &Path, meta: &Metadata) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let (from, to) = (entry.path(), dest.join(entry.file_name()));
        let kind = meta.file_type();
        if kind.is_dir() {
            fs::create_dir(&to)?;
            copy_dir(&from, &to, &meta)?;
        } else if kind.is_file() || kind.is_symlink() {
            copy_leaf(&from, &to, &meta)?;
        }
        // Sockets, pipes and devices are skipped: they are endpoints, not
        // content, and reading one could block forever.
    }
    // Last, so a read-only folder can still be filled, and the writes above
    // don't bump its modified time. One handle for both, because the new
    // permissions may not let us open it again.
    let dir = File::open(dest)?;
    dir.set_times(times(meta)?)?;
    dir.set_permissions(meta.permissions())
}

/// Copies a file, or a symlink as a link, to `dest`, which must not exist.
fn copy_leaf(src: &Path, dest: &Path, meta: &Metadata) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    if clone_file(src, dest)? {
        return Ok(());
    }
    let kind = meta.file_type();
    if kind.is_symlink() {
        return symlink(fs::read_link(src)?, dest);
    }
    if !kind.is_file() {
        return Err(invalid("Special files can't be copied."));
    }
    let mut from = File::open(src)?;
    let mut to = File::options()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dest)?;
    // On Linux this is copy_file_range, which shares blocks on Btrfs and XFS.
    io::copy(&mut from, &mut to)
        .and_then(|_| to.set_times(times(meta)?))
        .and_then(|()| to.set_permissions(meta.permissions()))
        .inspect_err(|_| {
            let _ = fs::remove_file(dest);
        })
}

/// Clones a file or symlink on APFS: instant, shares blocks, and keeps all
/// metadata. Returns false when the volume can't clone (not APFS, or `dest`
/// is on another volume). std::fs::copy also clones, but it would replace an
/// existing `dest`.
#[cfg(target_os = "macos")]
fn clone_file(src: &Path, dest: &Path) -> io::Result<bool> {
    const CLONE_NOFOLLOW: u32 = 0x0001;
    let (src, dest) = (c_path(src)?, c_path(dest)?);
    // SAFETY: both are valid NUL-terminated paths.
    if unsafe { libc::clonefile(src.as_ptr(), dest.as_ptr(), CLONE_NOFOLLOW) } == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ENOTSUP | libc::EXDEV) => Ok(false),
        _ => Err(err),
    }
}

fn times(meta: &Metadata) -> io::Result<FileTimes> {
    Ok(FileTimes::new()
        .set_accessed(meta.accessed()?)
        .set_modified(meta.modified()?))
}

/// Renames without ever replacing `to`; fails with `AlreadyExists` if it exists.
#[cfg(target_os = "macos")]
fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let (from_c, to_c) = (c_path(from)?, c_path(to)?);
    // SAFETY: both are valid NUL-terminated paths.
    if unsafe { libc::renamex_np(from_c.as_ptr(), to_c.as_ptr(), libc::RENAME_EXCL) } == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        // Some network and FAT volumes can't do an exclusive rename.
        Some(libc::ENOTSUP) => rename_checked(from, to),
        _ => Err(err),
    }
}

/// Renames without ever replacing `to`; fails with `AlreadyExists` if it exists.
#[cfg(not(target_os = "macos"))]
fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let (from_c, to_c) = (c_path(from)?, c_path(to)?);
    // The raw syscall, since musl (which the release builds link) has no
    // renameat2 wrapper, and neither does glibc before 2.28.
    // SAFETY: both are valid NUL-terminated paths, relative to the working directory.
    let status = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if status == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        // EINVAL: the file system doesn't support the flag (some network and
        // FUSE ones). ENOSYS: a kernel older than 3.15.
        Some(libc::EINVAL | libc::ENOSYS) => rename_checked(from, to),
        _ => Err(err),
    }
}

/// For volumes without an atomic no-replace rename: check, then rename. Another
/// process could create `to` in between, but this is the best those volumes allow.
fn rename_checked(from: &Path, to: &Path) -> io::Result<()> {
    match fs::symlink_metadata(to) {
        Ok(_) => Err(io::Error::from_raw_os_error(libc::EEXIST)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => fs::rename(from, to),
        Err(err) => Err(err),
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Starts a helper without blocking and without touching the terminal, which
/// the browser owns: no stdio, and its own process group so keys typed in the
/// terminal (Ctrl-C) never reach it. A thread reaps it so it can't linger as
/// a zombie.
fn launch(cmd: &mut Command) -> io::Result<()> {
    let mut child = quiet(cmd).spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
}

/// Asks the file manager over D-Bus to show the item selected. The call can
/// wait on a file manager starting up, so it runs on its own thread, which
/// opens the parent folder instead if nobody answers.
#[cfg(not(target_os = "macos"))]
fn reveal_freedesktop(path: &Path) -> io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("/")).to_path_buf();
    let uri = format!("file://{}", percent_encode(path.as_os_str().as_bytes()));
    let mut cmd = if has_command("gdbus") {
        let mut cmd = Command::new("gdbus");
        cmd.args([
            "call",
            "--session",
            "--dest=org.freedesktop.FileManager1",
            "--object-path=/org/freedesktop/FileManager1",
            "--method=org.freedesktop.FileManager1.ShowItems",
            // The URI is percent-encoded, so it can't contain a quote.
            &format!("['{uri}']"),
            "",
        ]);
        cmd
    } else if has_command("dbus-send") {
        let mut cmd = Command::new("dbus-send");
        cmd.args([
            "--session",
            "--print-reply",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
            &format!("array:string:{uri}"),
            "string:",
        ]);
        cmd
    } else {
        return open(&parent);
    };
    let mut child = quiet(&mut cmd).spawn()?;
    std::thread::spawn(move || {
        if !child.wait().is_ok_and(|status| status.success()) {
            let _ = open(&parent);
        }
    });
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn has_command(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

/// URL-style escaping for file URIs and trashinfo paths: every byte except
/// letters, digits, "-._~" and "/" becomes %XX.
#[cfg(any(not(target_os = "macos"), test))]
fn percent_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| invalid("The name contains a NUL byte."))
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

/// Prefixes an error with the item's name, so a failure in a batch says which
/// item it was.
fn about(path: &Path, err: io::Error) -> io::Error {
    let name = path.file_name().unwrap_or(path.as_os_str());
    io::Error::new(err.kind(), format!("{}: {err}", name.to_string_lossy()))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, SystemTime};

    use super::*;

    /// A fresh folder under the system temp folder, removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> TempDir {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("lsd-test-{}-{n}", std::process::id()));
            fs::create_dir(&path).unwrap();
            TempDir(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // Tests make read-only folders, which would stop remove_dir_all.
            fn writable(dir: &Path) {
                let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
                for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                    if entry.file_type().is_ok_and(|t| t.is_dir()) {
                        writable(&entry.path());
                    }
                }
            }
            writable(&self.0);
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn duplicates_get_finder_names() {
        let tmp = TempDir::new();
        let dir = tmp.path();
        let file = dir.join("a.txt");
        fs::write(&file, "x").unwrap();
        fs::create_dir(dir.join("pics.2024")).unwrap();

        let first = copy_into(std::slice::from_ref(&file), dir).unwrap();
        let second = copy_into(std::slice::from_ref(&file), dir).unwrap();
        assert_eq!(first, [dir.join("a copy.txt")]);
        assert_eq!(second, [dir.join("a copy 2.txt")]);
        // Copying a copy continues the sequence.
        let third = copy_into(&[dir.join("a copy.txt")], dir).unwrap();
        assert_eq!(third, [dir.join("a copy 3.txt")]);
        // A dot in a folder name is not an extension.
        let folder = copy_into(&[dir.join("pics.2024")], dir).unwrap();
        assert_eq!(folder, [dir.join("pics.2024 copy")]);
        assert_eq!(fs::read_to_string(dir.join("a copy 2.txt")).unwrap(), "x");
    }

    #[test]
    fn clash_in_another_folder_is_numbered() {
        let tmp = TempDir::new();
        let (src, dest) = (tmp.path().join("src"), tmp.path().join("dest"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(src.join(".env"), "new").unwrap();
        fs::write(dest.join(".env"), "old").unwrap();

        let copied = copy_into(&[src.join(".env")], &dest).unwrap();
        assert_eq!(copied, [dest.join(".env 2")]);
        assert_eq!(fs::read_to_string(dest.join(".env")).unwrap(), "old");
        assert_eq!(fs::read_to_string(dest.join(".env 2")).unwrap(), "new");
    }

    #[test]
    fn new_folders_are_numbered() {
        let tmp = TempDir::new();
        assert_eq!(
            new_folder(tmp.path()).unwrap(),
            tmp.path().join("untitled folder")
        );
        assert_eq!(
            new_folder(tmp.path()).unwrap(),
            tmp.path().join("untitled folder 2")
        );
    }

    #[test]
    fn rename_never_overwrites() {
        let tmp = TempDir::new();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let err = rename(&a, "b").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&a).unwrap(), "a");
        assert_eq!(fs::read_to_string(&b).unwrap(), "b");

        for bad in ["", ".", "..", "x/y"] {
            let err = rename(&a, bad).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{bad:?}");
        }
        assert_eq!(rename(&a, "c").unwrap(), tmp.path().join("c"));
        assert_eq!(names(tmp.path()), ["b", "c"]);
    }

    #[test]
    fn rename_can_change_only_case() {
        let tmp = TempDir::new();
        let file = tmp.path().join("photo.jpg");
        fs::write(&file, "x").unwrap();
        assert_eq!(
            rename(&file, "Photo.jpg").unwrap(),
            tmp.path().join("Photo.jpg")
        );
        assert_eq!(names(tmp.path()), ["Photo.jpg"]);
    }

    #[test]
    fn copy_keeps_metadata_and_links() {
        let tmp = TempDir::new();
        let src = tmp.path().join("project");
        let dest = tmp.path().join("backup");
        fs::create_dir_all(src.join("locked")).unwrap();
        fs::create_dir(&dest).unwrap();
        let script = src.join("run.sh");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o751)).unwrap();
        fs::write(src.join("locked/notes.txt"), "kept").unwrap();
        symlink("run.sh", src.join("link")).unwrap();
        symlink("missing", src.join("dangling")).unwrap();

        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let set_old = |path: &Path| {
            File::open(path)
                .unwrap()
                .set_times(FileTimes::new().set_accessed(old).set_modified(old))
                .unwrap()
        };
        set_old(&script);
        set_old(&src.join("locked"));
        fs::set_permissions(src.join("locked"), fs::Permissions::from_mode(0o555)).unwrap();
        set_old(&src);

        let copied = copy_into(std::slice::from_ref(&src), &dest).unwrap();
        let copy = dest.join("project");
        assert_eq!(copied, std::slice::from_ref(&copy));

        let meta = fs::metadata(copy.join("run.sh")).unwrap();
        assert_eq!(meta.mode() & 0o7777, 0o751);
        assert_eq!(meta.modified().unwrap(), old);
        let locked = fs::metadata(copy.join("locked")).unwrap();
        assert_eq!(locked.mode() & 0o7777, 0o555);
        assert_eq!(locked.modified().unwrap(), old);
        assert_eq!(fs::metadata(&copy).unwrap().modified().unwrap(), old);
        assert_eq!(
            fs::read_to_string(copy.join("locked/notes.txt")).unwrap(),
            "kept"
        );
        for (link, target) in [("link", "run.sh"), ("dangling", "missing")] {
            let path = copy.join(link);
            assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
            assert_eq!(fs::read_link(&path).unwrap(), Path::new(target));
        }
    }

    #[test]
    fn copy_or_move_into_itself_is_refused() {
        let tmp = TempDir::new();
        let folder = tmp.path().join("folder");
        fs::create_dir_all(folder.join("inner")).unwrap();
        for dest in [folder.clone(), folder.join("inner")] {
            let err = copy_into(std::slice::from_ref(&folder), &dest).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            let err = move_into(std::slice::from_ref(&folder), &dest).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }
        assert_eq!(names(&folder), ["inner"]);
        assert!(names(&folder.join("inner")).is_empty());
    }

    #[test]
    fn move_renames_on_clash_and_keeps_the_original() {
        let tmp = TempDir::new();
        let (src, dest) = (tmp.path().join("src"), tmp.path().join("dest"));
        fs::create_dir_all(src.join("album")).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(src.join("song.mp3"), "new").unwrap();
        fs::write(src.join("album/track.mp3"), "t").unwrap();
        fs::write(dest.join("song.mp3"), "old").unwrap();

        let moved = move_into(&[src.join("song.mp3"), src.join("album")], &dest).unwrap();
        assert_eq!(moved, [dest.join("song 2.mp3"), dest.join("album")]);
        assert!(names(&src).is_empty());
        assert_eq!(fs::read_to_string(dest.join("song.mp3")).unwrap(), "old");
        assert_eq!(fs::read_to_string(dest.join("song 2.mp3")).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(dest.join("album/track.mp3")).unwrap(),
            "t"
        );

        // Moving into the folder it is already in changes nothing.
        let same = move_into(&[dest.join("song.mp3")], &dest).unwrap();
        assert_eq!(same, [dest.join("song.mp3")]);
        assert_eq!(names(&dest), ["album", "song 2.mp3", "song.mp3"]);
    }

    /// Needs a second file system. Docker and most Linux systems have a tmpfs
    /// at /dev/shm; elsewhere the test has nothing to check.
    #[test]
    fn move_across_devices_copies_then_removes() {
        let tmp = TempDir::new();
        let other = Path::new("/dev/shm");
        let temp_dev = fs::metadata(tmp.path()).unwrap().dev();
        if !fs::metadata(other).is_ok_and(|m| m.dev() != temp_dev) {
            eprintln!("skipped: no second file system at /dev/shm");
            return;
        }
        let dest = other.join(format!("lsd-test-{}", std::process::id()));
        fs::create_dir(&dest).unwrap();
        let folder = tmp.path().join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("f"), "data").unwrap();
        symlink("f", folder.join("l")).unwrap();

        let moved = move_into(std::slice::from_ref(&folder), &dest);
        let data = fs::read_to_string(dest.join("folder/f"));
        let link = fs::read_link(dest.join("folder/l"));
        fs::remove_dir_all(&dest).unwrap();

        assert_eq!(moved.unwrap(), [dest.join("folder")]);
        assert_eq!(data.unwrap(), "data");
        assert_eq!(link.unwrap(), Path::new("f"));
        assert!(!folder.exists());
    }

    /// Moves one throwaway file to the real Trash, then deletes it from there.
    /// Ignored by default so `cargo test` never touches the user's Trash; run
    /// with `cargo test -- --ignored`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn trashes_into_the_real_trash() {
        let tmp = TempDir::new();
        let file = tmp
            .path()
            .join(format!("lsd-trash-test-{}.txt", std::process::id()));
        fs::write(&file, "throwaway").unwrap();
        let trashed = trash::trash_item(&file).unwrap();
        let in_trash = fs::read_to_string(&trashed);
        let _ = fs::remove_file(&trashed);
        assert!(!file.exists());
        assert_eq!(in_trash.unwrap(), "throwaway");
        assert!(trashed.starts_with(std::env::var("HOME").unwrap()));
    }
}
