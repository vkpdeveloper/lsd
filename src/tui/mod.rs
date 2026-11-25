//! The interactive browser (`lsd -i`).
//!
//! PLACEHOLDER: the real browser replaces this module and keeps `run` and
//! `Options` unchanged.

use std::path::PathBuf;

use crate::entry::Sort;
use crate::term::Term;

pub struct Options {
    pub start: PathBuf,
    pub show_hidden: bool,
    pub sort: Sort,
    /// When set, the directory the user ended in is written here on quit, so a
    /// shell function can `cd` to it.
    pub cwd_file: Option<PathBuf>,
}

pub fn run(_term: &Term, _opts: Options) -> anyhow::Result<()> {
    anyhow::bail!("the interactive browser is not built yet")
}
