mod diacritics;
mod entry;
mod fmt;
mod fsops;
mod kitty;
mod meta;
mod places;
mod term;
mod thumb;
mod tui;
mod view;

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use entry::{Entry, Sort, SortKey};
use term::Term;
use thumb::Thumbnailer;
use view::{Ctx, IconSize};

/// ls + display: list files with real thumbnails, or browse them like Finder.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    /// Files or folders to show (default: the current folder).
    paths: Vec<PathBuf>,

    /// Browse interactively, with preview, Quick Look and file operations.
    #[arg(short, long)]
    interactive: bool,

    /// List view: small icons with Date Modified, Size and Kind columns.
    #[arg(short, long, group = "view")]
    list: bool,

    /// One name per line, no images.
    #[arg(short = '1', group = "view")]
    one: bool,

    /// Show hidden files.
    #[arg(short, long)]
    all: bool,

    /// Sort by date modified, newest first.
    #[arg(short = 't')]
    by_date: bool,

    /// Sort by size, largest first.
    #[arg(short = 'S')]
    by_size: bool,

    /// Sort by kind (extension).
    #[arg(short = 'X')]
    by_kind: bool,

    /// Sort order.
    #[arg(long, value_enum)]
    sort: Option<SortKey>,

    /// Reverse the sort order.
    #[arg(short, long)]
    reverse: bool,

    /// Put folders before files.
    #[arg(long)]
    dirs_first: bool,

    /// Icon size.
    #[arg(short, long, value_enum, default_value_t)]
    size: IconSize,

    /// With -i: write the final folder to this file on quit (for a `cd` wrapper).
    #[arg(long, value_name = "FILE")]
    cwd_file: Option<PathBuf>,
}

impl Cli {
    fn sort(&self) -> Sort {
        let key = self.sort.unwrap_or(if self.by_date {
            SortKey::Date
        } else if self.by_size {
            SortKey::Size
        } else if self.by_kind {
            SortKey::Kind
        } else {
            SortKey::Name
        });
        Sort {
            key,
            reverse: self.reverse,
            dirs_first: self.dirs_first,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let term = Term::detect();
    if term.tmux_hint {
        eprintln!("{}", term::TMUX_HINT);
    }

    if cli.interactive {
        let start = cli
            .paths
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."));
        let opts = tui::Options {
            start,
            show_hidden: cli.all,
            sort: cli.sort(),
            cwd_file: cli.cwd_file.clone(),
        };
        return match tui::run(&term, opts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("lsd: {err:#}");
                ExitCode::FAILURE
            }
        };
    }

    match list(&cli, &term) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        // A closed pipe (`lsd | head`) is not an error.
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("lsd: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Prints like `ls`: loose files first as one group, then each folder with a
/// heading when there is more than one thing to show. Returns false if any path
/// could not be read.
fn list(cli: &Cli, term: &Term) -> io::Result<bool> {
    let paths = if cli.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        cli.paths.clone()
    };
    let sort = cli.sort();
    let mut ok = true;
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for path in &paths {
        match Entry::from_path(path) {
            Ok(e) if e.is_dir => dirs.push(e),
            Ok(mut e) => {
                // Show the path as typed, like ls.
                e.name = path.to_string_lossy().into_owned();
                files.push(e);
            }
            Err(err) => {
                eprintln!("lsd: {}: {}", path.display(), err.kind());
                ok = false;
            }
        }
    }
    entry::sort(&mut files, sort);
    entry::sort(
        &mut dirs,
        Sort {
            key: SortKey::Name,
            ..sort
        },
    );

    let ctx = Ctx {
        term,
        thumbs: Thumbnailer::new(),
        size: cli.size,
    };
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let headings = paths.len() > 1;
    let mut first = true;

    if !files.is_empty() {
        show(cli, &ctx, &files, &mut out)?;
        first = false;
    }
    for dir in &dirs {
        let mut entries = match entry::read_dir(&dir.path, cli.all) {
            Ok(entries) => entries,
            Err(err) => {
                eprintln!("lsd: {}: {}", dir.path.display(), err.kind());
                ok = false;
                continue;
            }
        };
        entry::sort(&mut entries, sort);
        if headings {
            if !first {
                writeln!(out)?;
            }
            writeln!(out, "{}:", dir.path.display())?;
        }
        show(cli, &ctx, &entries, &mut out)?;
        first = false;
    }
    out.flush()?;
    Ok(ok)
}

fn show(cli: &Cli, ctx: &Ctx, entries: &[Entry], out: &mut impl Write) -> io::Result<()> {
    if cli.one || !ctx.term.tty {
        return view::plain::lines(entries, out);
    }
    view::icons::render(ctx, entries, out)
}
