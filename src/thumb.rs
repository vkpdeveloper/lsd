//! Thumbnails, cached on disk as PNG.
//!
//! PLACEHOLDER BACKEND: this only handles common image formats through the
//! `image` crate. The real backend (QuickLook on macOS, native tools on Linux,
//! generic type icons for everything else) replaces this file and keeps the
//! public API below unchanged.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use crate::entry::Entry;

/// How the thumbnail is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Finder icon look: page curl on documents, frame on movies, real icons for
    /// folders and apps.
    Icon,
    /// Clean content only, for the preview pane and Quick Look.
    Preview,
}

/// A PNG in the cache directory.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Thumb {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

/// Cheap to clone and safe to share across threads.
#[derive(Clone)]
pub struct Thumbnailer {
    dir: Arc<PathBuf>,
}

impl Thumbnailer {
    pub fn new() -> Thumbnailer {
        let dir = cache_dir().join("thumbs");
        let _ = std::fs::create_dir_all(&dir);
        Thumbnailer { dir: Arc::new(dir) }
    }

    /// Returns a cached thumbnail without generating anything. Fast enough to call
    /// on every frame.
    pub fn cached(&self, entry: &Entry, px: u32, mode: Mode) -> Option<Thumb> {
        let path = self.key_path(entry, px, mode);
        let (width, height) = png_size(&path)?;
        Some(Thumb {
            path,
            width,
            height,
        })
    }

    /// Returns the thumbnail, generating it if needed. May block for a while
    /// (hundreds of ms for a large video), so call it off the UI thread.
    /// `px` is the longest side wanted, in pixels.
    pub fn get(&self, entry: &Entry, px: u32, mode: Mode) -> Option<Thumb> {
        if let Some(t) = self.cached(entry, px, mode) {
            return Some(t);
        }
        if entry.is_dir {
            return None;
        }
        let img = image::open(&entry.path).ok()?;
        let img = img.thumbnail(px, px);
        let path = self.key_path(entry, px, mode);
        let tmp = path.with_extension("tmp");
        img.save_with_format(&tmp, image::ImageFormat::Png).ok()?;
        std::fs::rename(&tmp, &path).ok()?;
        Some(Thumb {
            path,
            width: img.width(),
            height: img.height(),
        })
    }

    /// Generates thumbnails for many entries in parallel. Results arrive as they
    /// finish, tagged with the entry's index; every index is sent exactly once.
    pub fn batch(
        &self,
        entries: &[Entry],
        px: u32,
        mode: Mode,
    ) -> Receiver<(usize, Option<Thumb>)> {
        let (tx, rx) = mpsc::channel();
        let jobs = Arc::new(Mutex::new(
            entries
                .iter()
                .cloned()
                .enumerate()
                .collect::<Vec<_>>()
                .into_iter(),
        ));
        let workers = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .min(entries.len().max(1));
        for _ in 0..workers {
            let (tx, jobs, this) = (tx.clone(), jobs.clone(), self.clone());
            std::thread::spawn(move || {
                loop {
                    let job = jobs.lock().ok().and_then(|mut j| j.next());
                    let Some((i, entry)) = job else { break };
                    if tx.send((i, this.get(&entry, px, mode))).is_err() {
                        break;
                    }
                }
            });
        }
        rx
    }

    fn key_path(&self, entry: &Entry, px: u32, mode: Mode) -> PathBuf {
        let mut h = DefaultHasher::new();
        entry.path.hash(&mut h);
        entry.size.hash(&mut h);
        entry
            .modified
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .hash(&mut h);
        px.hash(&mut h);
        mode.hash(&mut h);
        self.dir.join(format!("{:016x}.png", h.finish()))
    }
}

/// Rounds a wanted pixel size up to a few fixed buckets so the cache holds at
/// most a handful of sizes per file.
pub fn bucket(px: u32) -> u32 {
    [64, 128, 256, 512, 1024, 2048]
        .into_iter()
        .find(|&b| b >= px)
        .unwrap_or(2048)
}

/// Width and height from a PNG's IHDR chunk, reading only the first 24 bytes.
pub fn png_size(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut head = [0u8; 24];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(head[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(head[20..24].try_into().ok()?);
    Some((w, h))
}

/// `~/Library/Caches/lsd` on macOS, `$XDG_CACHE_HOME/lsd` or `~/.cache/lsd` elsewhere.
pub fn cache_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    if cfg!(target_os = "macos") {
        return home.join("Library/Caches/lsd");
    }
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".cache"))
        .join("lsd")
}
