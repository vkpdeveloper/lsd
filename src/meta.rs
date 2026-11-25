//! File kinds ("PDF document") and the details shown under "Information".
//!
//! PLACEHOLDER BACKEND: `kind` is a small extension table and `info` only reads
//! image dimensions. The real backend (Spotlight metadata on macOS, ffprobe and
//! friends on Linux) replaces this file and keeps the public API unchanged.

use std::time::{Duration, SystemTime};

use crate::entry::Entry;

/// Details that cost more than a `stat`. Every field is optional because the
/// platform may not know it.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub kind: String,
    pub dimensions: Option<(u32, u32)>,
    pub pages: Option<u32>,
    pub duration: Option<Duration>,
    pub last_opened: Option<SystemTime>,
    /// Number of items, for folders.
    pub items: Option<usize>,
}

/// Human file kind from the name alone. Cheap enough for every row of a listing.
pub fn kind(entry: &Entry) -> String {
    if entry.is_dir {
        return if entry.ext() == "app" || entry.name.ends_with(".app") {
            "Application"
        } else {
            "Folder"
        }
        .into();
    }
    let ext = entry.ext();
    let known = match ext.as_str() {
        "pdf" => "PDF document",
        "png" => "PNG image",
        "jpg" | "jpeg" => "JPEG image",
        "gif" => "GIF image",
        "mp4" => "MPEG-4 movie",
        "mov" => "QuickTime movie",
        "txt" => "Plain Text",
        "md" => "Markdown",
        "rs" => "Rust source",
        _ => "",
    };
    if !known.is_empty() {
        return known.into();
    }
    if ext.is_empty() {
        "Document".into()
    } else {
        format!("{} file", ext.to_uppercase())
    }
}

/// Full details. May spawn helper processes, so call it off the UI thread.
pub fn info(entry: &Entry) -> Info {
    let mut info = Info {
        kind: kind(entry),
        ..Info::default()
    };
    if entry.is_dir {
        info.items = std::fs::read_dir(&entry.path).ok().map(|d| d.count());
    } else {
        info.dimensions = image::image_dimensions(&entry.path).ok();
    }
    info
}
