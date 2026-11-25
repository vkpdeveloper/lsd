use std::cmp::Ordering;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One file system item, with the metadata every view needs. Built from a
/// single `lstat` (plus a `stat` for symlinks) so listing stays cheap.
#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    /// True for directories and symlinks that point at directories.
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub mode: u32,
}

impl Entry {
    pub fn from_path(path: &Path) -> io::Result<Entry> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let lmeta = fs::symlink_metadata(path)?;
        Ok(Entry::from_metadata(name, path.to_path_buf(), &lmeta))
    }

    fn from_metadata(name: String, path: PathBuf, lmeta: &Metadata) -> Entry {
        let is_symlink = lmeta.file_type().is_symlink();
        // Follow symlinks for type and size, but keep the link itself when the target is gone.
        let followed = if is_symlink {
            fs::metadata(&path).ok()
        } else {
            None
        };
        let meta = followed.as_ref().unwrap_or(lmeta);
        Entry {
            name,
            path,
            is_dir: meta.is_dir(),
            is_symlink,
            size: meta.len(),
            modified: meta.modified().ok(),
            created: meta.created().ok(),
            mode: mode_of(meta),
        }
    }

    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }

    /// Lowercased extension without the dot, or "" when there is none.
    pub fn ext(&self) -> String {
        if self.is_dir {
            return String::new();
        }
        Path::new(&self.name)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    }
}

#[cfg(unix)]
fn mode_of(meta: &Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode()
}

#[cfg(not(unix))]
fn mode_of(_: &Metadata) -> u32 {
    0
}

/// Reads a directory. Entries that vanish between readdir and stat are skipped.
pub fn read_dir(dir: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let Ok(lmeta) = item.metadata() else { continue };
        entries.push(Entry::from_metadata(name, item.path(), &lmeta));
    }
    Ok(entries)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum SortKey {
    #[default]
    Name,
    Kind,
    Date,
    Size,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Sort {
    pub key: SortKey,
    pub reverse: bool,
    pub dirs_first: bool,
}

pub fn sort(entries: &mut [Entry], sort: Sort) {
    entries.sort_by(|a, b| {
        if sort.dirs_first && a.is_dir != b.is_dir {
            return b.is_dir.cmp(&a.is_dir);
        }
        let ord = match sort.key {
            SortKey::Name => Ordering::Equal,
            SortKey::Kind => a.ext().cmp(&b.ext()),
            // Newest and largest first, like Finder and `ls -t` / `ls -S`.
            SortKey::Date => b.modified.cmp(&a.modified),
            SortKey::Size => b.size.cmp(&a.size),
        }
        .then_with(|| natural_cmp(&a.name, &b.name));
        if sort.reverse { ord.reverse() } else { ord }
    });
}

/// Finder-style name order: case-insensitive, and digit runs compare by value,
/// so "file 2" sorts before "file 10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na = take_number(&mut a);
                let nb = take_number(&mut b);
                let ord = na
                    .trim_start_matches('0')
                    .len()
                    .cmp(&nb.trim_start_matches('0').len())
                    .then_with(|| na.trim_start_matches('0').cmp(nb.trim_start_matches('0')));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn take_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut digits = String::new();
    while let Some(c) = chars.peek().copied().filter(char::is_ascii_digit) {
        digits.push(c);
        chars.next();
    }
    digits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_matches_finder() {
        let mut names = vec!["file10", "File2", "file1", "alpha", "file02b"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["alpha", "file1", "File2", "file02b", "file10"]);
    }
}
