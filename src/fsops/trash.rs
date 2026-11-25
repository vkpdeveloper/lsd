//! Moving one item to the Trash.

use std::io;
use std::path::{Path, PathBuf};

/// Moves `path` to the Trash and returns where it ended up. Goes through
/// NSFileManager, like Finder, so Put Back works. Foundation is loaded here,
/// not linked, so runs that trash nothing don't pay for it.
#[cfg(target_os = "macos")]
pub fn trash_item(path: &Path) -> io::Result<PathBuf> {
    use std::ffi::{CStr, c_char};
    use std::os::unix::ffi::OsStrExt;
    use std::ptr::null_mut;

    use crate::thumb::objc::{self, Id, Pool, msg};

    let path = std::path::absolute(path)?;
    // For a proper NotFound instead of a Foundation message.
    let meta = std::fs::symlink_metadata(&path)?;
    if objc::dlopen(c"/System/Library/Frameworks/Foundation.framework/Foundation").is_null() {
        return Err(io::Error::other("Foundation could not be loaded."));
    }
    let _pool = Pool::new();
    // SAFETY: Foundation is loaded, and every message matches its documented
    // signature. Results are autoreleased and drained with the pool.
    unsafe {
        let url = objc::file_url(&path, meta.is_dir());
        if url.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Not a valid path.",
            ));
        }
        let manager = msg!(objc::class(c"NSFileManager"), c"defaultManager"; Id);
        let mut trashed: Id = null_mut();
        let mut error: Id = null_mut();
        let ok = msg!(manager, c"trashItemAtURL:resultingItemURL:error:",
            url => Id, &mut trashed => *mut Id, &mut error => *mut Id; bool);
        if !ok {
            let message = objc::string(msg!(error, c"localizedDescription"; Id));
            return Err(io::Error::other(
                message.unwrap_or_else(|| "Couldn't move it to the Trash.".into()),
            ));
        }
        let repr = if trashed.is_null() {
            std::ptr::null()
        } else {
            msg!(trashed, c"fileSystemRepresentation"; *const c_char)
        };
        if repr.is_null() {
            return Err(io::Error::other(
                "The Trash did not say where the item went.",
            ));
        }
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(
            CStr::from_ptr(repr).to_bytes(),
        )))
    }
}

/// Moves `path` to the Trash and returns where it ended up.
#[cfg(not(target_os = "macos"))]
pub fn trash_item(path: &Path) -> io::Result<PathBuf> {
    freedesktop::trash(path, &freedesktop::data_home()?)
}

/// The freedesktop.org Trash spec, version 1.0:
/// <https://specifications.freedesktop.org/trash-spec/1.0/>
///
/// Built on macOS too when testing, so the format is checked everywhere.
#[cfg(any(not(target_os = "macos"), test))]
mod freedesktop {
    use std::fs::{self, DirBuilder, File};
    use std::io::{self, Write};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    use std::path::{Path, PathBuf};

    use super::super::{first_free, numbered, percent_encode, rename_noreplace, split_name};

    /// $XDG_DATA_HOME, or ~/.local/share. Tests pass their own folder.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub fn data_home() -> io::Result<PathBuf> {
        if let Some(dir) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
            && dir.is_absolute()
        {
            return Ok(dir);
        }
        let home = std::env::var_os("HOME")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set."))?;
        Ok(Path::new(&home).join(".local/share"))
    }

    /// Now, in local time, as the spec's `DeletionDate` wants it.
    fn deletion_date() -> io::Result<String> {
        let t = crate::fmt::Local::at(std::time::SystemTime::now())
            .ok_or_else(|| io::Error::other("The local time is unknown."))?;
        Ok(format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            t.year, t.month, t.day, t.hour, t.minute, t.second
        ))
    }

    /// Moves `path` into the right trash, writing its .trashinfo file first.
    pub fn trash(path: &Path, data_home: &Path) -> io::Result<PathBuf> {
        let meta = fs::symlink_metadata(path)?;
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "This item can't be trashed.")
        })?;
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        let original = fs::canonicalize(parent)?.join(name);
        let (trash_dir, base) = trash_dir_for(&original, meta.dev(), data_home)?;

        let (files, info) = (trash_dir.join("files"), trash_dir.join("info"));
        for dir in [&files, &info] {
            DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
        // Relative to the volume for a volume's own trash, so the item can
        // still be put back when the volume is mounted somewhere else.
        let stored = base.map_or(original.as_path(), |base| {
            original.strip_prefix(base).unwrap_or(&original)
        });
        let entry = format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            percent_encode(stored.as_os_str().as_bytes()),
            deletion_date()?,
        );

        let (stem, ext) = split_name(name, meta.is_dir());
        first_free(
            &files,
            |n| numbered(stem, ext, n),
            |dest| {
                // The spec reserves a name by creating its info file
                // exclusively, so two programs never pick the same one.
                let mut info_name = dest.file_name().unwrap_or_default().to_os_string();
                info_name.push(".trashinfo");
                let info_path = info.join(info_name);
                let mut file = File::options()
                    .write(true)
                    .create_new(true)
                    .open(&info_path)?;
                file.write_all(entry.as_bytes())
                    .and_then(|()| rename_noreplace(path, dest))
                    .inspect_err(|_| {
                        let _ = fs::remove_file(&info_path);
                    })
            },
        )
    }

    /// Picks the trash for an item on device `dev`: the home trash when it is
    /// on the same device, otherwise one at the top of the item's volume.
    /// Also returns the folder `Path=` is stored relative to (none for the
    /// home trash, which stores absolute paths).
    fn trash_dir_for(
        item: &Path,
        dev: u64,
        data_home: &Path,
    ) -> io::Result<(PathBuf, Option<PathBuf>)> {
        let home_trash = data_home.join("Trash");
        let home_dev = home_trash
            .ancestors()
            .find_map(|dir| fs::metadata(dir).ok())
            .map(|meta| meta.dev());
        if home_dev == Some(dev) {
            return Ok((home_trash, None));
        }

        let top = volume_top(item, dev);
        // SAFETY: getuid has no preconditions and can't fail.
        let uid = unsafe { libc::getuid() };
        // An admin-made $top/.Trash must be a real folder with the sticky bit,
        // so users can't tamper with each other's trash.
        let shared = top.join(".Trash");
        if fs::symlink_metadata(&shared).is_ok_and(|m| m.is_dir() && m.mode() & 0o1000 != 0) {
            let dir = shared.join(uid.to_string());
            if private_dir(&dir, uid).is_ok() {
                return Ok((dir, Some(top)));
            }
        }
        let dir = top.join(format!(".Trash-{uid}"));
        private_dir(&dir, uid).map_err(|_| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "Items on this disk can't be moved to the Trash.",
            )
        })?;
        Ok((dir, Some(top)))
    }

    /// The highest folder above `item` that is still on device `dev`.
    fn volume_top(item: &Path, dev: u64) -> PathBuf {
        let mut top = item;
        while let Some(up) = top.parent() {
            if !fs::metadata(up).is_ok_and(|meta| meta.dev() == dev) {
                break;
            }
            top = up;
        }
        top.to_path_buf()
    }

    /// Makes sure `dir` is a folder of ours, creating it as private if needed.
    fn private_dir(dir: &Path, uid: u32) -> io::Result<()> {
        match DirBuilder::new().mode(0o700).create(dir) {
            Err(err) if err.kind() != io::ErrorKind::AlreadyExists => return Err(err),
            _ => {}
        }
        let meta = fs::symlink_metadata(dir)?;
        if meta.is_dir() && meta.uid() == uid {
            Ok(())
        } else {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        }
    }

    #[cfg(test)]
    mod tests {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        use super::*;

        #[test]
        fn writes_trashinfo_and_numbers_clashes() {
            let tmp = crate::fsops::tests::TempDir::new();
            let data_home = tmp.path().join("data");
            let item = tmp.path().join("old notes.txt");
            fs::write(&item, "first").unwrap();

            let trashed = trash(&item, &data_home).unwrap();
            let trash_dir = data_home.join("Trash");
            assert_eq!(trashed, trash_dir.join("files/old notes.txt"));
            assert!(!item.exists());
            assert_eq!(fs::read_to_string(&trashed).unwrap(), "first");

            let info = fs::read_to_string(trash_dir.join("info/old notes.txt.trashinfo")).unwrap();
            let lines: Vec<_> = info.lines().collect();
            let original = fs::canonicalize(tmp.path()).unwrap().join("old notes.txt");
            let expected_path = format!("Path={}", percent_encode(original.as_os_str().as_bytes()));
            assert_eq!(lines[0], "[Trash Info]");
            assert_eq!(lines[1], expected_path);
            assert!(lines[1].ends_with("/old%20notes.txt"));
            let date = lines[2].strip_prefix("DeletionDate=").unwrap();
            let shape = date
                .bytes()
                .map(|b| if b.is_ascii_digit() { b'0' } else { b });
            assert!(shape.eq(*b"0000-00-00T00:00:00"), "{date}");

            // A second item with the same name gets the next free name.
            fs::write(&item, "second").unwrap();
            let trashed = trash(&item, &data_home).unwrap();
            assert_eq!(trashed, trash_dir.join("files/old notes 2.txt"));
            assert!(trash_dir.join("info/old notes 2.txt.trashinfo").exists());
            assert_eq!(fs::read_to_string(&trashed).unwrap(), "second");
        }

        #[test]
        fn skips_names_already_in_the_trash() {
            let tmp = crate::fsops::tests::TempDir::new();
            let data_home = tmp.path().join("data");
            let files = data_home.join("Trash/files");
            fs::create_dir_all(&files).unwrap();
            // Left behind without an info file, for example by a crash.
            fs::write(files.join("a"), "stale").unwrap();
            let item = tmp.path().join("a");
            fs::create_dir(&item).unwrap();

            let trashed = trash(&item, &data_home).unwrap();
            assert_eq!(trashed, files.join("a 2"));
            assert_eq!(fs::read_to_string(files.join("a")).unwrap(), "stale");
            assert!(!data_home.join("Trash/info/a.trashinfo").exists());
        }

        #[test]
        fn encodes_paths_like_urls() {
            let name = OsString::from_vec(b"/x/caf\xc3\xa9 #1%.txt".to_vec());
            assert_eq!(
                percent_encode(name.as_bytes()),
                "/x/caf%C3%A9%20%231%25.txt"
            );
        }
    }
}
