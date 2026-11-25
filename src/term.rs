//! What the terminal we are attached to can do. Detection never waits on the
//! terminal: it reads the environment, and inside tmux it asks the tmux server
//! once (a few milliseconds) about the outer terminal.

use std::env;
use std::io::IsTerminal;
use std::process::Command;
use std::time::Duration;

use crate::kitty::{Graphics, IdMode, Transfer};

/// How pictures reach the screen.
#[derive(Clone, Copy, Debug)]
pub enum Protocol {
    /// kitty graphics with Unicode placeholders: kitty and Ghostty. Images are
    /// text, so they scroll, clear and redraw like text, including in tmux.
    Kitty(Graphics),
    /// iTerm2 inline images: iTerm2 and WezTerm. Images sit at the cursor.
    Iterm { tmux: bool },
    /// No pictures (Alacritty, Terminal.app, ...): icons are glyphs and large
    /// previews are drawn with colored half blocks.
    Text,
}

impl Protocol {
    pub fn has_images(self) -> bool {
        !matches!(self, Protocol::Text)
    }
}

#[derive(Clone, Debug)]
pub struct Term {
    pub cols: u16,
    pub rows: u16,
    /// Pixel size of one cell. Guessed when the terminal does not report it.
    pub cell: (u16, u16),
    pub protocol: Protocol,
    /// 24-bit color works. Otherwise colors are mapped to the 256-color palette.
    pub truecolor: bool,
    /// Set when the outer terminal could show images but tmux is blocking them.
    pub tmux_hint: bool,
    pub tty: bool,
}

const FALLBACK_CELL: (u16, u16) = (10, 20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outer {
    Kitty,
    Iterm,
    Other,
}

impl Term {
    pub fn detect() -> Term {
        let tty = std::io::stdout().is_terminal();
        let (cols, rows, cell) = size();
        let mut term = Term {
            cols,
            rows,
            cell,
            protocol: Protocol::Text,
            truecolor: false,
            tmux_hint: false,
            tty,
        };
        if !tty {
            return term;
        }
        let tmux = env::var_os("TMUX").is_some();
        let (outer, ids) = if tmux {
            detect_tmux(&mut term)
        } else {
            (outer_from_env(), IdMode::Truecolor)
        };
        term.truecolor = if tmux {
            ids == IdMode::Truecolor
        } else {
            matches!(outer, Outer::Kitty | Outer::Iterm)
                || colorterm_truecolor()
                || known_truecolor()
        };

        let forced = env::var("LSD_IMAGES").ok();
        let choice = match forced.as_deref() {
            Some("kitty") => Outer::Kitty,
            Some("iterm") => Outer::Iterm,
            Some("text" | "off" | "none") => Outer::Other,
            _ if term.tmux_hint => Outer::Other,
            _ => outer,
        };
        term.protocol = match choice {
            Outer::Kitty => Protocol::Kitty(Graphics {
                tmux,
                ids,
                transfer: transfer(),
            }),
            Outer::Iterm => Protocol::Iterm { tmux },
            Outer::Other => Protocol::Text,
        };
        term
    }
}

fn var(k: &str) -> String {
    env::var(k).unwrap_or_default()
}

fn classify(name: &str) -> Outer {
    let name = name.to_ascii_lowercase();
    if name.contains("kitty") || name.contains("ghostty") {
        Outer::Kitty
    } else if name.contains("iterm") || name.contains("wezterm") {
        Outer::Iterm
    } else {
        Outer::Other
    }
}

fn outer_from_env() -> Outer {
    for name in [var("TERM"), var("TERM_PROGRAM"), var("LC_TERMINAL")] {
        let found = classify(&name);
        if found != Outer::Other {
            return found;
        }
    }
    if env::var_os("KITTY_WINDOW_ID").is_some() || env::var_os("GHOSTTY_RESOURCES_DIR").is_some() {
        Outer::Kitty
    } else if env::var_os("WEZTERM_EXECUTABLE").is_some()
        || env::var_os("ITERM_SESSION_ID").is_some()
    {
        Outer::Iterm
    } else {
        Outer::Other
    }
}

/// Asks tmux which terminal the current client runs in and whether escapes may
/// pass through. `client_termtype` is the terminal's own XTVERSION answer, which
/// identifies WezTerm and iTerm2 even though they report TERM=xterm-256color.
fn detect_tmux(term: &mut Term) -> (Outer, IdMode) {
    let mut cmd = Command::new("tmux");
    cmd.args(["display-message", "-p"]);
    if let Some(pane) = env::var_os("TMUX_PANE") {
        cmd.arg("-t").arg(pane);
    }
    cmd.arg("#{client_termname}\t#{client_termtype}\t#{client_termfeatures}\t#{allow-passthrough}");
    let Ok(out) = cmd.output() else {
        return (Outer::Other, IdMode::Indexed);
    };
    let line = String::from_utf8_lossy(&out.stdout);
    let mut parts = line.trim_end().split('\t');
    let termname = parts.next().unwrap_or_default();
    let termtype = parts.next().unwrap_or_default();
    let features = parts.next().unwrap_or_default();
    let passthrough = parts.next().unwrap_or_default();

    let mut outer = classify(termname);
    if outer == Outer::Other {
        outer = classify(termtype);
    }
    if outer == Outer::Other {
        // Variables from the shell that started the tmux server.
        outer = outer_from_env();
    }
    if outer != Outer::Other && !matches!(passthrough, "on" | "all") {
        term.tmux_hint = true;
    }
    // Without RGB tmux downsamples truecolor, which would scramble kitty image ids.
    let rgb = features.split(',').any(|f| f == "RGB");
    let ids = if rgb {
        IdMode::Truecolor
    } else {
        IdMode::Indexed
    };
    (outer, ids)
}

fn colorterm_truecolor() -> bool {
    matches!(var("COLORTERM").as_str(), "truecolor" | "24bit")
}

fn known_truecolor() -> bool {
    let t = var("TERM");
    t.contains("alacritty") || t.contains("direct") || t == "foot" || t.starts_with("foot-")
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

pub type Rgb = (u8, u8, u8);

/// The terminal's actual foreground and background, so the UI can derive
/// surfaces (sidebar, selection, bars) that match any theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colors {
    pub fg: Rgb,
    pub bg: Rgb,
}

impl Colors {
    pub fn is_dark(&self) -> bool {
        luma(self.bg) < luma(self.fg)
    }

    /// `amount` (0..=1) of the way from background to foreground. Use small
    /// amounts for surfaces and larger ones for secondary text.
    pub fn mix(&self, amount: f32) -> Rgb {
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
        (
            m(self.bg.0, self.fg.0),
            m(self.bg.1, self.fg.1),
            m(self.bg.2, self.fg.2),
        )
    }
}

fn luma((r, g, b): Rgb) -> f32 {
    0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32
}

/// Asks the terminal for its colors with OSC 10/11, using a DA1 request as a
/// sentinel so terminals that ignore the question answer right away. Must be
/// called before anything else reads stdin. Returns None if the terminal does
/// not answer within `timeout`.
pub fn query_colors(timeout: Duration) -> Option<Colors> {
    let reply = query_tty(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[c", timeout)?;
    let fg = parse_osc_color(&reply, "10;")?;
    let bg = parse_osc_color(&reply, "11;")?;
    Some(Colors { fg, bg })
}

/// Writes `request` to the controlling terminal and collects the reply until a
/// DA1 answer (`ESC [ ? ... c`) arrives or `timeout` passes.
fn query_tty(request: &[u8], timeout: Duration) -> Option<String> {
    use std::io::Write;
    use std::os::fd::AsRawFd;

    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let fd = tty.as_raw_fd();
    // SAFETY: plain termios calls on a file descriptor we own for this scope.
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
        return None;
    }
    let mut raw = saved;
    unsafe { libc::cfmakeraw(&mut raw) };
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) };

    let mut reply = Vec::new();
    let sent = tty.write_all(request).and_then(|_| tty.flush()).is_ok();
    let deadline = std::time::Instant::now() + timeout;
    if sent {
        read_until_da1(&mut tty, &mut reply, deadline);
    }
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &saved) };
    Some(String::from_utf8_lossy(&reply).into_owned())
}

/// Reads terminal replies into `reply` until a DA1 answer, EOF or `deadline`.
fn read_until_da1(tty: &mut std::fs::File, reply: &mut Vec<u8>, deadline: std::time::Instant) {
    use std::io::Read;
    use std::os::fd::AsRawFd;

    let fd = tty.as_raw_fd();
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return;
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        if unsafe { libc::poll(&mut pfd, 1, left.as_millis().max(1) as i32) } <= 0 {
            return;
        }
        let mut buf = [0u8; 256];
        match tty.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => reply.extend_from_slice(&buf[..n]),
        }
        if da1_done(reply) {
            return;
        }
    }
}

fn da1_done(reply: &[u8]) -> bool {
    let Some(start) = reply.windows(3).rposition(|w| w == b"\x1b[?") else {
        return false;
    };
    reply[start..].contains(&b'c')
}

/// Finds `ESC ] 1x ; rgb:RRRR/GGGG/BBBB` in a reply.
fn parse_osc_color(reply: &str, code: &str) -> Option<Rgb> {
    let at = reply.find(&format!("]{code}rgb:"))? + code.len() + 5;
    let rest = &reply[at..];
    let end = rest.find(['\x1b', '\x07']).unwrap_or(rest.len());
    let mut parts = rest[..end].split('/');
    let mut channel = || {
        let hex = parts.next()?;
        let v = u32::from_str_radix(hex, 16).ok()?;
        // Scale 1 to 4 hex digits down to 8 bits.
        let max = (1u32 << (4 * hex.len() as u32)) - 1;
        Some((v * 255 / max.max(1)) as u8)
    };
    Some((channel()?, channel()?, channel()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_osc_color_replies() {
        let reply = "\x1b]10;rgb:ffff/ffff/ffff\x1b\\\x1b]11;rgb:1e1e/1e1e/2e2e\x07\x1b[?62;22c";
        assert_eq!(parse_osc_color(reply, "10;"), Some((255, 255, 255)));
        assert_eq!(parse_osc_color(reply, "11;"), Some((30, 30, 46)));
        assert!(da1_done(reply.as_bytes()));
    }

    #[test]
    fn classifies_terminals() {
        assert_eq!(classify("xterm-ghostty"), Outer::Kitty);
        assert_eq!(classify("WezTerm 20240203-110809-5046fc22"), Outer::Iterm);
        assert_eq!(classify("iTerm2 3.5.10"), Outer::Iterm);
        assert_eq!(classify("alacritty"), Outer::Other);
    }
}
