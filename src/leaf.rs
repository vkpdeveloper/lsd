//! Documents open in Leaf, a terminal reader by Deepanshu Mishra
//! (https://github.com/DeepanshuMishraa/leaf, MIT). lsd draws the listing and
//! the previews; opening a PDF, book, Markdown or office file hands the
//! terminal to `leaf open` until the reader quits with Esc or q, then lsd
//! takes the screen back.
//!
//! The installer puts Leaf in ~/.local/bin, which is not always on PATH, so
//! the usual install folders are searched too.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::entry::Entry;

/// Whether Leaf is the reader for this file. Pictures stay with lsd.
pub fn reads(entry: &Entry) -> bool {
    !entry.is_dir
        && matches!(
            entry.ext().as_str(),
            "pdf"
                | "epub"
                | "md"
                | "markdown"
                | "mdown"
                | "mkd"
                | "docx"
                | "pptx"
                | "xlsx"
                | "ods"
                | "csv"
                | "tsv"
        )
}

/// The `leaf` command, if it is installed.
pub fn command() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let fallbacks = [
        home.as_ref().map(|h| h.join(".local/bin")),
        home.as_ref().map(|h| h.join(".cargo/bin")),
        Some(PathBuf::from("/opt/homebrew/bin")),
        Some(PathBuf::from("/usr/local/bin")),
    ];
    std::env::split_paths(&path)
        .chain(fallbacks.into_iter().flatten())
        .map(|dir| dir.join("leaf"))
        .find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Runs `leaf open <path>` in this terminal and waits for it to quit. The
/// caller must have given up the terminal first: no raw mode, and nothing
/// else reading input, or Leaf's own terminal queries go astray. Leaf clears
/// every kitty image when it quits, so pictures must be sent again afterwards.
///
/// Leaf draws on stdout; its stderr is kept so a failure can be shown in lsd
/// instead of vanishing when lsd redraws.
pub fn open(leaf: &Path, path: &Path) -> Result<(), String> {
    let output = Command::new(leaf)
        .arg("open")
        .arg(path)
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|child| child.wait_with_output())
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr.lines().map(str::trim).find(|l| !l.is_empty());
    Err(reason.map_or_else(
        || output.status.to_string(),
        |l| l.strip_prefix("leaf: ").unwrap_or(l).to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_reads_documents_but_not_pictures() {
        let entry = |name: &str| Entry {
            name: name.into(),
            path: name.into(),
            ..Entry::default()
        };
        assert!(reads(&entry("paper.PDF")));
        assert!(reads(&entry("book.epub")));
        assert!(reads(&entry("README.md")));
        assert!(!reads(&entry("photo.jpg")));
        assert!(!reads(&entry("data.json")));
    }
}
