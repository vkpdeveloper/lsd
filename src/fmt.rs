//! Finder-style text: sizes, dates, two-line icon labels, middle ellipsis, and
//! OSC 8 links. Shared by the inline views and the browser.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

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
    let (Some(t), Some(now)) = (Local::at(t), Local::at(SystemTime::now())) else {
        return "--".into();
    };
    label(t, now)
}

fn label(t: Local, now: Local) -> String {
    let hour = match t.hour % 12 {
        0 => 12,
        h => h,
    };
    let half = if t.hour < 12 { "AM" } else { "PM" };
    let time = format!("{hour}:{:02} {half}", t.minute);
    match now.day_number() - t.day_number() {
        0 => format!("Today, {time}"),
        1 => format!("Yesterday, {time}"),
        _ => {
            const MONTHS: [&str; 12] = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            let month = MONTHS[usize::from(t.month - 1)];
            format!("{} {month} {}, {time}", t.day, t.year)
        }
    }
}

/// Wall-clock time in the local time zone, from the C library. chrono's
/// time zone lookup links CoreFoundation on macOS, which costs every run
/// about 1 ms before lsd draws anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    pub year: i64,
    /// 1 to 12.
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl Local {
    pub fn at(t: SystemTime) -> Option<Local> {
        let secs = match t.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => d.as_secs() as libc::time_t,
            Err(e) => -(e.duration().as_secs() as libc::time_t),
        };
        // SAFETY: tm is plain old data, and localtime_r only writes into it.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
            return None;
        }
        Some(Local {
            year: i64::from(tm.tm_year) + 1900,
            month: (tm.tm_mon + 1) as u8,
            day: tm.tm_mday as u8,
            hour: tm.tm_hour as u8,
            minute: tm.tm_min as u8,
            second: tm.tm_sec as u8,
        })
    }

    /// Days since 1970-01-01 on the calendar (Howard Hinnant's
    /// days_from_civil), so "yesterday" is right across DST changes.
    fn day_number(self) -> i64 {
        let (m, d) = (i64::from(self.month), i64::from(self.day));
        let y = if m <= 2 { self.year - 1 } else { self.year };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
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

/// `file://host/path` with percent-encoding, for OSC 8 links. Relative paths
/// are made absolute, or clicking `./notes.txt` would open nothing.
pub fn file_url(path: &Path) -> String {
    static HOST: LazyLock<String> = LazyLock::new(hostname);
    static CWD: LazyLock<Option<PathBuf>> = LazyLock::new(|| std::env::current_dir().ok());
    let joined;
    let path = match &*CWD {
        Some(cwd) if path.is_relative() => {
            joined = cwd.join(path.strip_prefix(".").unwrap_or(path));
            &joined
        }
        _ => path,
    };
    let mut url = String::from("file://");
    url.push_str(&HOST);
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
    fn links_point_at_absolute_paths() {
        let cwd = std::env::current_dir().unwrap();
        let url = file_url(Path::new("./my notes.txt"));
        assert!(
            url.ends_with(&format!("{}/my%20notes.txt", cwd.display())),
            "{url}"
        );
        assert!(file_url(Path::new("/tmp/x")).ends_with("/tmp/x"));
    }

    #[test]
    fn dates_read_like_finder() {
        let at = |year, month, day, hour, minute| Local {
            year,
            month,
            day,
            hour,
            minute,
            second: 0,
        };
        let now = at(2026, 3, 1, 9, 0);
        assert_eq!(label(at(2026, 3, 1, 0, 5), now), "Today, 12:05 AM");
        // Across a month end, in a leap-free year.
        assert_eq!(label(at(2026, 2, 28, 12, 30), now), "Yesterday, 12:30 PM");
        assert_eq!(
            label(at(2025, 12, 31, 23, 59), now),
            "31 Dec 2025, 11:59 PM"
        );
        assert_eq!(
            label(at(2024, 2, 29, 9, 0), at(2024, 3, 1, 9, 0)),
            "Yesterday, 9:00 AM"
        );
    }

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
