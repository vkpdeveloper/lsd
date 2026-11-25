//! Image headers through the `image` crate, MP4 boxes, and ffprobe and
//! pdfinfo when they are installed.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{Info, movie};
use crate::entry::Entry;
use crate::thumb::{Class, class_of};

pub fn fill(entry: &Entry, info: &mut Info) {
    if entry.is_dir {
        return;
    }
    match class_of(&entry.ext()) {
        Some(Class::Image) => info.dimensions = image::image_dimensions(&entry.path).ok(),
        Some(Class::Video) => {
            if let Some(m) = movie(&entry.path) {
                info.duration = m.duration;
                info.dimensions = m.dimensions;
            }
            if info.duration.is_none() || info.dimensions.is_none() {
                ffprobe(&entry.path, info);
            }
        }
        Some(Class::Pdf) => {
            let out = output(Command::new("pdfinfo").arg(&entry.path));
            info.pages = out
                .as_deref()
                .and_then(|o| field(o, "Pages:")?.parse().ok());
        }
        _ => {}
    }
}

fn ffprobe(path: &Path, info: &mut Info) {
    let Some(out) = output(
        Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0"])
            .args(["-show_entries", "stream=width,height:format=duration"])
            .args(["-of", "default=noprint_wrappers=1"])
            .arg(path),
    ) else {
        return;
    };
    let num = |key| field(&out, key).and_then(|v| v.parse::<f64>().ok());
    if let (Some(w), Some(h)) = (num("width="), num("height=")) {
        info.dimensions = info.dimensions.or(Some((w as u32, h as u32)));
    }
    if let Some(d) = num("duration=").and_then(|s| Duration::try_from_secs_f64(s).ok()) {
        info.duration = info.duration.or(Some(d));
    }
}

/// The trimmed value after `key` on the first line that starts with it.
fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|l| l.strip_prefix(key))
        .map(str::trim)
}

/// Standard output of a tool, or None if it is missing, fails, or takes more
/// than five seconds.
fn output(cmd: &mut Command) -> Option<String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut text = String::new();
    child.stdout.take()?.read_to_string(&mut text).ok()?;
    Some(text)
}
