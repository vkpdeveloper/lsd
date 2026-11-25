//! What the terminal we are attached to can do. Detection never blocks on a
//! terminal round trip: it reads the environment, and inside tmux it asks the
//! tmux server once (a few milliseconds) for the outer terminal and options.

use std::env;
use std::io::IsTerminal;
use std::process::Command;

use crate::kitty::{Graphics, IdMode, Transfer};

#[derive(Clone, Debug)]
pub struct Term {
    pub cols: u16,
    pub rows: u16,
    /// Pixel size of one cell. Guessed when the terminal does not report it.
    pub cell: (u16, u16),
    pub graphics: Option<Graphics>,
    /// Set when the outer terminal could show images but tmux is blocking them.
    pub tmux_hint: bool,
    pub tty: bool,
}

const FALLBACK_CELL: (u16, u16) = (10, 20);

impl Term {
    pub fn detect() -> Term {
        let tty = std::io::stdout().is_terminal();
        let (cols, rows, cell) = size();
        let mut term = Term {
            cols,
            rows,
            cell,
            graphics: None,
            tmux_hint: false,
            tty,
        };
        if !tty {
            return term;
        }
        match env::var("LSD_GRAPHICS").as_deref() {
            Ok("off" | "none" | "0") => return term,
            Ok("kitty" | "on" | "1") => {
                let tmux = env::var_os("TMUX").is_some();
                term.graphics = Some(Graphics {
                    tmux,
                    ids: IdMode::Truecolor,
                    transfer: transfer(),
                });
                return term;
            }
            _ => {}
        }
        if env::var_os("TMUX").is_some() {
            detect_tmux(&mut term);
        } else if outer_supports_kitty() {
            term.graphics = Some(Graphics {
                tmux: false,
                ids: IdMode::Truecolor,
                transfer: transfer(),
            });
        }
        term
    }
}

/// Terminals known to implement kitty graphics *with* Unicode placeholders.
fn is_kitty_term(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("kitty") || name.contains("ghostty")
}

fn outer_supports_kitty() -> bool {
    let var = |k: &str| env::var(k).unwrap_or_default();
    is_kitty_term(&var("TERM"))
        || is_kitty_term(&var("TERM_PROGRAM"))
        || env::var_os("KITTY_WINDOW_ID").is_some()
        || env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
}

fn detect_tmux(term: &mut Term) {
    let mut cmd = Command::new("tmux");
    cmd.args(["display-message", "-p"]);
    if let Some(pane) = env::var_os("TMUX_PANE") {
        cmd.arg("-t").arg(pane);
    }
    cmd.arg("#{client_termname}\t#{client_termfeatures}\t#{allow-passthrough}");
    let Ok(out) = cmd.output() else { return };
    let line = String::from_utf8_lossy(&out.stdout);
    let mut parts = line.trim_end().split('\t');
    let termname = parts.next().unwrap_or_default();
    let features = parts.next().unwrap_or_default();
    let passthrough = parts.next().unwrap_or_default();

    // tmux reports its own TERM for detached or odd clients; fall back to env.
    let outer = is_kitty_term(termname)
        || env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || env::var_os("KITTY_WINDOW_ID").is_some();
    if !outer {
        return;
    }
    if !matches!(passthrough, "on" | "all") {
        term.tmux_hint = true;
        return;
    }
    // Without RGB tmux downsamples truecolor, which would scramble image ids.
    let ids = if features.split(',').any(|f| f == "RGB") {
        IdMode::Truecolor
    } else {
        IdMode::Indexed
    };
    term.graphics = Some(Graphics {
        tmux: true,
        ids,
        transfer: transfer(),
    });
}

/// Files only work when the terminal shares our file system. A pane inside a
/// tmux server started over SSH inherits these variables too.
fn transfer() -> Transfer {
    if env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some() {
        Transfer::Direct
    } else {
        Transfer::File
    }
}

fn size() -> (u16, u16, (u16, u16)) {
    // SAFETY: TIOCGWINSZ fills a plain struct; a zeroed winsize is a valid input.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let fds = [libc::STDOUT_FILENO, libc::STDIN_FILENO, libc::STDERR_FILENO];
    let ok = fds
        .iter()
        .any(|&fd| unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0);
    if !ok {
        let cols = env::var("COLUMNS")
            .ok()
            .and_then(|c| c.parse().ok())
            .unwrap_or(80);
        return (cols, 24, FALLBACK_CELL);
    }
    let cell = if ws.ws_xpixel > 0 && ws.ws_ypixel > 0 {
        (ws.ws_xpixel / ws.ws_col, ws.ws_ypixel / ws.ws_row.max(1))
    } else {
        FALLBACK_CELL
    };
    (ws.ws_col, ws.ws_row, cell)
}

pub const TMUX_HINT: &str = "lsd: tmux is blocking images. Add `set -g allow-passthrough on` to ~/.tmux.conf, then run `tmux source ~/.tmux.conf`.";
