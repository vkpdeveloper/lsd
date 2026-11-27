mod diacritics;
mod doc;
mod entry;
mod epub;
mod fmt;
mod fsops;
mod glyph;
mod halfblock;
mod iterm;
mod kitty;
mod leaf;
mod meta;
mod places;
mod setup;
mod term;
mod thumb;
mod tui;
mod view;

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use entry::{Entry, Sort, SortKey};
use glyph::Icons;
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

    /// Tree view: folders expanded recursively.
    #[arg(short = 'T', long, group = "view")]
    tree: bool,

    /// Gallery view: a large preview of each item with its details.
    #[arg(short, long, group = "view")]
    gallery: bool,

    /// One name per line, no images.
    #[arg(short = '1', group = "view")]
    one: bool,

    /// With --tree: how many levels deep to go.
    #[arg(short = 'L', long, value_name = "N", value_hint = clap::ValueHint::Other)]
    depth: Option<usize>,

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

    /// Text icons for terminals without images [default: emoji, or $LSD_ICONS].
    #[arg(long, value_enum)]
    icons: Option<Icons>,

    /// With -i: write the final folder to this file on quit (for a `cd` wrapper).
    #[arg(long, value_name = "FILE")]
    cwd_file: Option<PathBuf>,

    /// Set up tab completion for this shell. With SHELL, print its script.
    #[arg(long, value_enum, value_name = "SHELL", num_args = 0..=1, exclusive = true)]
    completions: Option<Option<Shell>>,

    /// Update lsd to the latest release.
    #[arg(long, exclusive = true)]
    update: bool,

    /// Remove lsd and its thumbnail cache from this machine.
    #[arg(long, exclusive = true)]
    uninstall: bool,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum Shell {
    Bash,
    Zsh,
    Fish,
    Elvish,
    Powershell,
    Nushell,
}

/// Set by the elvish and PowerShell scripts when they ask lsd what Tab offers.
const COMPLETE_VAR: &str = "LSD_COMPLETE";

/// The tab completion script for `shell`. Built in memory so printing it can
/// stop early (`| head`) without a panic: the generators panic when a write
/// fails.
fn completions(shell: Shell) -> io::Result<Vec<u8>> {
    use clap_complete::env::{self, EnvCompleter};
    use clap_complete::{Shell as Std, generate};
    let mut cmd = Cli::command();
    let mut script = Vec::new();
    match shell {
        Shell::Bash => {
            generate(Std::Bash, &mut cmd, "lsd", &mut script);
            script.extend_from_slice(BASH_FILES_FIRST.as_bytes());
        }
        Shell::Zsh => generate(Std::Zsh, &mut cmd, "lsd", &mut script),
        Shell::Fish => generate(Std::Fish, &mut cmd, "lsd", &mut script),
        Shell::Nushell => generate(clap_complete_nushell::Nushell, &mut cmd, "lsd", &mut script),
        // clap's fixed scripts for these offer flags only, so these ask lsd on
        // each Tab instead, and it offers values and paths too.
        Shell::Elvish => {
            env::Elvish.write_registration(COMPLETE_VAR, "lsd", "lsd", "lsd", &mut script)?
        }
        Shell::Powershell => {
            env::Powershell.write_registration(COMPLETE_VAR, "lsd", "lsd", "lsd", &mut script)?
        }
    }
    Ok(script)
}

/// The generated bash function offers every flag for an empty word. lsd's
/// words are nearly always paths, so like `ls` it offers files there, and
/// flags once the word starts with a dash.
const BASH_FILES_FIRST: &str = r#"
_lsd_files_first() {
    _lsd "$@"
    if [[ "${COMP_WORDS[COMP_CWORD]}" != -* && "${COMPREPLY[0]}" == -* ]]; then
        COMPREPLY=()
    fi
}
complete -F _lsd_files_first -o bashdefault -o default lsd
"#;

fn print_completions(shell: Shell) -> ExitCode {
    match completions(shell).and_then(|script| io::stdout().write_all(&script)) {
        Err(err) if err.kind() != io::ErrorKind::BrokenPipe => {
            eprintln!("lsd: {err}");
            ExitCode::FAILURE
        }
        _ => ExitCode::SUCCESS,
    }
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
    // Answers an elvish or PowerShell Tab, then exits. Otherwise does nothing.
    clap_complete::CompleteEnv::with_factory(Cli::command)
        .var(COMPLETE_VAR)
        .complete();
    let cli = Cli::parse();
    // Before anything looks at the terminal: shells run this at startup.
    match cli.completions {
        Some(Some(shell)) => return print_completions(shell),
        Some(None) => return setup::completions(),
        None => {}
    }
    if cli.update {
        return setup::update();
    }
    if cli.uninstall {
        return setup::uninstall();
    }
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
                eprintln!("lsd: {}: {}", path.display(), fsops::describe(&err));
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

    let single_file = paths.len() == 1 && files.len() == 1;
    let ctx = Ctx {
        term,
        thumbs: Thumbnailer::new(),
        size: cli.size,
        icons: Icons::resolve(cli.icons),
    };
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    if cli.tree && ctx.term.tty {
        let mut roots = files;
        roots.extend(dirs);
        let opts = view::tree::TreeOpts {
            show_hidden: cli.all,
            sort,
            depth: cli.depth,
        };
        view::tree::render(&ctx, &roots, &opts, &mut out)?;
        out.flush()?;
        return Ok(ok);
    }
    // `lsd data.json` shows the file itself, like Quick Look. Other files,
    // or any file with -g, show their card: a bigger picture with details.
    if single_file && ctx.term.tty && !(cli.list || cli.one || cli.tree) {
        if cli.gallery || !view::quicklook::render(&ctx, &files[0], &mut out)? {
            view::gallery::render(&ctx, &files, &mut out)?;
        }
        out.flush()?;
        return Ok(ok);
    }
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
                eprintln!("lsd: {}: {}", dir.path.display(), fsops::describe(&err));
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
        view::plain::lines(entries, out)
    } else if cli.list {
        view::list::render(ctx, entries, out)
    } else if cli.gallery {
        view::gallery::render(ctx, entries, out)
    } else {
        view::icons::render(ctx, entries, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn bash_wrapper_calls_the_generated_function() {
        // A rename in clap_complete would leave the wrapper calling nothing.
        let script = String::from_utf8(completions(Shell::Bash).unwrap()).unwrap();
        assert!(script.contains("_lsd() {"));
    }
}
