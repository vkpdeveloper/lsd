//! Finder-style text: sizes, dates, two-line icon labels, middle ellipsis, and
//! OSC 8 links. Shared by the inline views and the browser.

use std::path::Path;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Finder uses decimal units: 1 KB = 1000 bytes.
pub fn size(bytes: u64) -> String {
    match bytes {
        0 => "Zero bytes".into(),
        1 => "1 byte".into(),
        b if b < 1000 => format!("{b} bytes"),
        b if b < 1_000_000 => format!("{} KB", (b as f64 / 1e3).round() as u64),
        b if b < 1_000_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        b if b < 1_000_000_000_000 => format!("{:.2} GB", b as f64 / 1e9),
        b => format!("{:.2} TB", b as f64 / 1e12),
    }
}

/// "Today, 10:41 PM", "Yesterday, 9:03 AM", otherwise "26 Nov 2026, 10:41 PM".
pub fn date(t: SystemTime) -> String {
    let t: DateTime<Local> = t.into();
    let today = Local::now().date_naive();
    let day = t.date_naive();
    let time = t.format("%-I:%M %p");
    if day == today {
        format!("Today, {time}")
    } else if today.pred_opt() == Some(day) {
        format!("Yesterday, {time}")
    } else {
        format!("{}, {time}", t.format("%-d %b %Y"))
    }
}

/// "00:06", or "1:02:03" past an hour.
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Keeps the start and the end (so the extension survives):
/// "openai-matrix-multiplication-2026-11-24.pdf" -> "openai-matri…6-11-24.pdf".
pub fn ellipsize_middle(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".chars().take(max).collect();
    }
    let budget = max - 1;
    let tail_budget = budget * 2 / 5;
    let head = take_width(s.chars(), budget - tail_budget);
    let tail: String = take_width(s.chars().rev(), tail_budget)
        .chars()
        .rev()
        .collect();
    format!("{head}…{tail}")
}

/// Cuts from the end: "a very long name" -> "a very lo…".
pub fn ellipsize_end(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    format!("{}…", take_width(s.chars(), max - 1))
}

fn take_width(chars: impl Iterator<Item = char>, max: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in chars {
        let w = c.width().unwrap_or(0);
        if used + w > max {
            break;
        }
        used += w;
        out.push(c);
    }
    out
}

/// Finder icon labels: up to two lines, breaking after `-`, `_`, `.` or a space
/// when possible, with the second line middle-truncated.
pub fn label_lines(name: &str, max: usize) -> Vec<String> {
    if width(name) <= max {
        return vec![name.to_string()];
    }
    let chars: Vec<char> = name.chars().collect();
    // Longest prefix that fits, then back off to the last break character in it.
    let mut fit = 0;
    let mut used = 0;
    for (i, c) in chars.iter().enumerate() {
        let w = c.width().unwrap_or(0);
        if used + w > max {
            break;
        }
        used += w;
        fit = i + 1;
    }
    let brk = chars[..fit]
        .iter()
        .rposition(|c| matches!(c, '-' | '_' | ' ' | '.'))
        .map(|i| i + 1)
        .filter(|&i| i * 2 >= fit) // a break in the first half wastes too much of line one
        .unwrap_or(fit);
    let first: String = chars[..brk].iter().collect();
    let rest: String = chars[brk..].iter().collect();
    vec![
        first.trim_end().to_string(),
        ellipsize_middle(rest.trim_start(), max),
    ]
}

/// Pads `s` to `w` columns, centered.
pub fn center(s: &str, w: usize) -> String {
    let sw = width(s);
    if sw >= w {
        return s.to_string();
    }
    let left = (w - sw) / 2;
    format!("{}{s}{}", " ".repeat(left), " ".repeat(w - sw - left))
}

/// `file://host/path` with percent-encoding, for OSC 8 links.
pub fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    url.push_str(&hostname());
    for b in path.as_os_str().as_encoded_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                url.push(*b as char)
            }
            _ => url.push_str(&format!("%{b:02X}")),
        }
    }
    url
}

/// Opening half of an OSC 8 hyperlink. Close it with [`LINK_END`].
pub fn link_start(url: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\")
}

pub const LINK_END: &str = "\x1b]8;;\x1b\\";

fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its full length and gethostname NUL-terminates on success.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    if !ok {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_wrap_like_finder() {
        assert_eq!(label_lines("bitcoin.pdf", 18), ["bitcoin.pdf"]);
        assert_eq!(
            label_lines("fx-pulse-fullscreen.mp4", 18),
            ["fx-pulse-", "fullscreen.mp4"]
        );
        let long = label_lines("openai-matrix-multiplication-2026-11-24.pdf", 18);
        assert_eq!(long[0], "openai-matrix-");
        assert!(width(&long[1]) <= 18 && long[1].ends_with(".pdf") && long[1].contains('…'));
    }

    #[test]
    fn finder_sizes() {
        assert_eq!(size(184_321), "184 KB");
        assert_eq!(size(2_300_000), "2.3 MB");
        assert_eq!(size(999), "999 bytes");
    }
}
