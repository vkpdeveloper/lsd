//! Styles that fit the user's terminal theme. Text keeps the terminal's own
//! colors and the accent is the theme's ANSI blue. When the terminal tells us
//! its background, surfaces (bars, sidebar, inactive selection) are mixed from
//! it; otherwise everything falls back to attributes that need no RGB.

use std::time::Duration;

use ratatui::style::{Color, Modifier, Style};

use crate::halfblock;
use crate::term::{self, Colors, Rgb};

pub struct Theme {
    /// Toolbar, sidebar and path bar.
    pub chrome: Style,
    /// Inactive selection and the surface behind a selected icon.
    pub soft: Style,
    /// Every other row in the list view.
    pub stripe: Style,
    /// The selection: accent background with readable text.
    pub sel: Style,
    /// Accent text, for marks.
    pub accent: Style,
    /// Secondary text.
    pub dim: Style,
    /// Separators and placeholders.
    pub faint: Style,
    pub error: Style,
}

impl Theme {
    /// Asks the terminal for its colors. Must run before anything reads input.
    pub fn detect(truecolor: bool) -> Theme {
        let colors = term::query_colors(Duration::from_millis(100));
        // Only worth a second round trip when the terminal answered the first.
        let accent = colors.and_then(|_| query_palette(4, Duration::from_millis(100)));
        Theme::new(colors, accent, truecolor)
    }

    pub fn new(colors: Option<Colors>, accent: Option<Rgb>, truecolor: bool) -> Theme {
        let Some(c) = colors else {
            return Theme::plain();
        };
        let rgb = |v: Rgb| {
            if truecolor {
                Color::Rgb(v.0, v.1, v.2)
            } else {
                Color::Indexed(halfblock::ansi256(v))
            }
        };
        let mix = |amount| rgb(c.mix(amount));
        // Dark themes tend to have a light, pastel blue and light themes a deep
        // one, so the background color reads best on the accent in both. When we
        // know the actual blue, pick whichever of fg and bg contrasts more.
        let sel_fg = match accent {
            Some(a) if contrast(a, c.fg) > contrast(a, c.bg) => c.fg,
            _ => c.bg,
        };
        Theme {
            chrome: Style::new().bg(mix(0.06)),
            soft: Style::new().bg(mix(if c.is_dark() { 0.16 } else { 0.12 })),
            stripe: Style::new().bg(mix(0.025)),
            sel: Style::new().bg(Color::Blue).fg(rgb(sel_fg)),
            accent: Style::new().fg(Color::Blue),
            dim: Style::new().fg(mix(0.55)),
            faint: Style::new().fg(mix(0.3)),
            error: Style::new().fg(Color::Red),
        }
    }

    fn plain() -> Theme {
        Theme {
            chrome: Style::new(),
            soft: Style::new().add_modifier(Modifier::BOLD),
            stripe: Style::new(),
            // Reversed blue: blue background, text in the terminal's background color.
            sel: Style::new()
                .fg(Color::Blue)
                .add_modifier(Modifier::REVERSED),
            accent: Style::new().fg(Color::Blue),
            dim: Style::new().add_modifier(Modifier::DIM),
            faint: Style::new().add_modifier(Modifier::DIM),
            error: Style::new().fg(Color::Red),
        }
    }

    /// Background color of a style, for pictures drawn on a surface.
    pub fn bg_of(style: Style) -> Color {
        style.bg.unwrap_or(Color::Reset)
    }
}

/// WCAG contrast ratio between two colors.
fn contrast(a: Rgb, b: Rgb) -> f32 {
    let lum = |(r, g, b): Rgb| {
        let lin = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    };
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Asks for one ANSI palette entry with OSC 4, using DA1 as a sentinel like
/// `term::query_colors` so silent terminals answer at once.
// TODO(term): this duplicates the tty plumbing in term.rs; `query_colors` could
// return the accent too and this would go away.
fn query_palette(index: u8, timeout: Duration) -> Option<Rgb> {
    let request = format!("\x1b]4;{index};?\x1b\\\x1b[c");
    let reply = query_tty(request.as_bytes(), timeout)?;
    parse_rgb(&reply, &format!("]4;{index};rgb:"))
}

fn query_tty(request: &[u8], timeout: Duration) -> Option<String> {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::time::Instant;

    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let fd = tty.as_raw_fd();
    // SAFETY: plain termios calls on a descriptor we own for this scope.
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
        return None;
    }
    let mut raw = saved;
    unsafe { libc::cfmakeraw(&mut raw) };
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) };

    let mut reply = Vec::new();
    if tty.write_all(request).and_then(|_| tty.flush()).is_ok() {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd.
            if unsafe { libc::poll(&mut pfd, 1, left.as_millis().max(1) as i32) } <= 0 {
                break;
            }
            let mut buf = [0u8; 256];
            let Ok(n) = tty.read(&mut buf) else { break };
            reply.extend_from_slice(&buf[..n]);
            let da1 = reply
                .windows(3)
                .rposition(|w| w == b"\x1b[?")
                .is_some_and(|at| reply[at..].contains(&b'c'));
            if da1 {
                break;
            }
        }
    }
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &saved) };
    Some(String::from_utf8_lossy(&reply).into_owned())
}

/// Reads `rgb:RRRR/GGGG/BBBB` right after `prefix`.
fn parse_rgb(reply: &str, prefix: &str) -> Option<Rgb> {
    let rest = &reply[reply.find(prefix)? + prefix.len()..];
    let end = rest.find(['\x1b', '\x07']).unwrap_or(rest.len());
    let mut parts = rest[..end].split('/');
    let mut channel = || {
        let hex = parts.next()?;
        let v = u32::from_str_radix(hex, 16).ok()?;
        let max = (1u32 << (4 * hex.len().min(4) as u32)) - 1;
        Some((v * 255 / max.max(1)) as u8)
    };
    Some((channel()?, channel()?, channel()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_text_contrasts_with_the_accent() {
        let dark = Colors {
            fg: (205, 214, 244),
            bg: (30, 30, 46),
        };
        // Pastel blue on a dark theme: dark text.
        let t = Theme::new(Some(dark), Some((137, 180, 250)), true);
        assert_eq!(t.sel.fg, Some(Color::Rgb(30, 30, 46)));
        // Deep blue on a dark theme: light text.
        let t = Theme::new(Some(dark), Some((0, 0, 200)), true);
        assert_eq!(t.sel.fg, Some(Color::Rgb(205, 214, 244)));
    }

    #[test]
    fn parses_palette_reply() {
        let reply = "\x1b]4;4;rgb:8989/b4b4/fafa\x1b\\\x1b[?62c";
        assert_eq!(parse_rgb(reply, "]4;4;rgb:"), Some((137, 180, 250)));
    }
}
