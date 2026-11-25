//! File operations used by the browser, with Finder semantics.
//!
//! PLACEHOLDER: signatures only. The real implementation replaces this file and
//! keeps the public API unchanged.

#![allow(dead_code)]

use std::io;
use std::path::{Path, PathBuf};

fn unsupported<T>() -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "not implemented yet",
    ))
}

/// Moves items to the Trash (macOS) or the freedesktop trash (Linux).
pub fn trash(_paths: &[PathBuf]) -> io::Result<()> {
    unsupported()
}

/// Copies items into `dest_dir`. Name clashes get Finder names ("a copy.txt",
/// "a copy 2.txt"). Returns the new paths.
pub fn copy_into(_srcs: &[PathBuf], _dest_dir: &Path) -> io::Result<Vec<PathBuf>> {
    unsupported()
}

/// Moves items into `dest_dir`, renaming on clashes ("a 2.txt"). Returns the new paths.
pub fn move_into(_srcs: &[PathBuf], _dest_dir: &Path) -> io::Result<Vec<PathBuf>> {
    unsupported()
}

/// Renames in place. Refuses to overwrite an existing item.
pub fn rename(_path: &Path, _new_name: &str) -> io::Result<PathBuf> {
    unsupported()
}

/// Creates "untitled folder" (or "untitled folder 2", ...) in `dir`.
pub fn new_folder(_dir: &Path) -> io::Result<PathBuf> {
    unsupported()
}

/// Opens with the default app, detached and silent.
pub fn open(_path: &Path) -> io::Result<()> {
    unsupported()
}

/// Shows the item selected in Finder (macOS) or the system file manager (Linux).
pub fn reveal(_path: &Path) -> io::Result<()> {
    unsupported()
}
