//! Thumbnails, cached on disk as PNG.
//!
//! macOS asks QuickLook in process, so pictures and icons match Finder. Linux
//! decodes images itself, uses ffmpeg and pdftoppm when they are installed,
//! reuses the freedesktop thumbnail cache, and draws folder and file icons.
//!
//! Which picture an entry gets (see [`content`]):
//! - Content thumbnails are cached per file, keyed by path, size, mtime, pixel
//!   size and mode: images, video, PDF, and other formats QuickLook renders
//!   (office and iWork documents, fonts, audio, ...), plus `.app` bundles and
//!   the special folders (Downloads, Applications, ...) whose icons differ.
//! - Everything else shares one icon per type: folders, each extension,
//!   extensionless files, executables. A 5000-entry node_modules costs a
//!   handful of renders, not 5000.
//! - At 64 px and below (list and tree rows) only images, video and PDF get
//!   content thumbnails; office documents and the like use the type icon.
//! - Preview mode shows one file at a time, so on macOS every file gets its
//!   content (text included).
//!
//! When content cannot be rendered the entry falls back to its type icon, and
//! if even that fails, to a drawn icon, so `get` only returns None when the
//! cache cannot be written.

mod icon;
#[cfg(any(not(target_os = "macos"), test))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) mod objc;

#[cfg(not(target_os = "macos"))]
use linux as backend;
#[cfg(target_os = "macos")]
use macos as backend;

use std::collections::HashMap;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use crate::entry::Entry;

/// Bump when the look of generated thumbnails changes, to start a fresh cache.
const VERSION: u32 = 1;

/// Largest size that counts as a list or tree row.
const SMALL: u32 = 64;

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

/// Formats with a real content thumbnail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Image,
    Video,
    Pdf,
    /// Anything else QuickLook draws per file: documents, fonts, audio, apps.
    Other,
}

/// A picture shared by every entry of one type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Icon {
    Folder,
    /// A folder that macOS shows as a file, like `.xcodeproj` or `.bundle`.
    Package(String),
    Ext(String),
    /// No extension.
    Plain,
    /// No extension and executable.
    Exec,
}

/// QuickLook picks its own generator and icon, so only the Linux backend
/// reads the payloads.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) enum What {
    Content(Class),
    Icon(Icon),
}

/// One picture for a backend to render into `out`.
pub(crate) struct Job {
    /// Absolute path of the file to render.
    pub src: PathBuf,
    pub out: PathBuf,
    pub px: u32,
    /// Only QuickLook draws icon decorations (page curl, movie frame).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub mode: Mode,
    pub what: What,
}

/// Cheap to clone and safe to share across threads.
#[derive(Clone)]
pub struct Thumbnailer(Arc<Inner>);

struct Inner {
    dir: PathBuf,
    cwd: PathBuf,
    /// Sizes of cache files already seen, so repeated `cached` calls skip the
    /// disk.
    known: Mutex<HashMap<PathBuf, (u32, u32)>>,
}

impl Default for Thumbnailer {
    fn default() -> Thumbnailer {
        Thumbnailer::new()
    }
}

impl Thumbnailer {
    pub fn new() -> Thumbnailer {
        let dir = cache_dir().join("thumbs");
        let _ = std::fs::create_dir_all(&dir);
        Thumbnailer(Arc::new(Inner {
            dir,
            cwd: std::env::current_dir().unwrap_or_default(),
            known: Mutex::default(),
        }))
    }

    /// Returns a cached thumbnail without generating anything. Fast enough to call
    /// on every frame.
    pub fn cached(&self, entry: &Entry, px: u32, mode: Mode) -> Option<Thumb> {
        self.lookup(&self.plan(entry, px, mode, false).out)
    }

    /// Returns the thumbnail, generating it if needed. May block for a while
    /// (hundreds of ms for a large video), so call it off the UI thread.
    /// `px` is the longest side wanted, in pixels.
    pub fn get(&self, entry: &Entry, px: u32, mode: Mode) -> Option<Thumb> {
        let mut thumb = None;
        self.run(std::slice::from_ref(entry), px, mode, &mut |_, t| thumb = t);
        thumb
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
        let (this, entries) = (self.clone(), entries.to_vec());
        std::thread::spawn(move || {
            this.run(&entries, px, mode, &mut |i, t| {
                let _ = tx.send((i, t));
            });
        });
        rx
    }

    fn run(
        &self,
        entries: &[Entry],
        px: u32,
        mode: Mode,
        emit: &mut dyn FnMut(usize, Option<Thumb>),
    ) {
        let all: Vec<usize> = (0..entries.len()).collect();
        let (content, mut icons): (Vec<_>, Vec<_>) = self
            .round(entries, &all, px, mode, false, emit)
            .into_iter()
            .partition(|&(_, content)| content);
        // Content that could not be rendered gets its type icon instead.
        let content: Vec<usize> = content.into_iter().map(|(i, _)| i).collect();
        icons.extend(self.round(entries, &content, px, mode, true, emit));
        for (i, _) in icons {
            emit(i, self.drawn(&entries[i], px));
        }
    }

    /// Emits what is cached right away and renders the rest, once per output
    /// file. Returns the indices that failed, each with whether it wanted
    /// content (rather than a type icon).
    fn round(
        &self,
        entries: &[Entry],
        indices: &[usize],
        px: u32,
        mode: Mode,
        icon_only: bool,
        emit: &mut dyn FnMut(usize, Option<Thumb>),
    ) -> Vec<(usize, bool)> {
        let mut jobs: Vec<Job> = Vec::new();
        let mut waiting: Vec<Vec<usize>> = Vec::new();
        let mut by_out: HashMap<PathBuf, usize> = HashMap::new();
        for &i in indices {
            let job = self.plan(&entries[i], px, mode, icon_only);
            if let Some(t) = self.lookup(&job.out) {
                emit(i, Some(t));
            } else if let Some(&j) = by_out.get(&job.out) {
                waiting[j].push(i);
            } else {
                by_out.insert(job.out.clone(), jobs.len());
                jobs.push(job);
                waiting.push(vec![i]);
            }
        }
        let mut failed = Vec::new();
        if jobs.is_empty() {
            return failed;
        }
        backend::generate(&jobs, &mut |j, dims| {
            let job = &jobs[j];
            let Some((width, height)) = dims else {
                let content = matches!(job.what, What::Content(_));
                failed.extend(waiting[j].iter().map(|&i| (i, content)));
                return;
            };
            self.remember(&job.out, (width, height));
            for &i in &waiting[j] {
                let path = job.out.clone();
                emit(
                    i,
                    Some(Thumb {
                        path,
                        width,
                        height,
                    }),
                );
            }
        });
        failed
    }

    /// Decides what to show for `entry` and where it is cached.
    fn plan(&self, entry: &Entry, px: u32, mode: Mode, icon_only: bool) -> Job {
        // Normalizes `./x` so `lsd`, `lsd .` and `lsd /abs` share cache entries.
        let src: PathBuf = self.0.cwd.join(&entry.path).components().collect();
        let ext = ext_of(entry);
        let class = if icon_only {
            None
        } else {
            content(entry, &ext, px, mode)
        };
        if let Some(class) = class {
            let key = content_key(&src, entry, px, mode);
            return Job {
                out: self.0.dir.join(format!("{key:016x}.png")),
                src,
                px,
                mode,
                what: What::Content(class),
            };
        }
        let icon = Icon::of(entry, &ext);
        // The generic folder comes from a folder lsd owns, so a custom icon on
        // whichever user folder is rendered first cannot leak into all folders.
        let src = if icon == Icon::Folder {
            self.0.dir.clone()
        } else {
            src
        };
        Job {
            out: self.icon_path(&icon, px, false),
            src,
            px,
            mode: Mode::Icon,
            what: What::Icon(icon),
        }
    }

    /// Type icons come from the platform; `drawn` ones are lsd's own. Linux
    /// only has the drawn kind.
    fn icon_path(&self, icon: &Icon, px: u32, drawn: bool) -> PathBuf {
        let kind = if drawn || !cfg!(target_os = "macos") {
            "draw"
        } else {
            "type"
        };
        let name = match icon {
            Icon::Folder => "folder".into(),
            Icon::Package(ext) => format!("pkg-{ext}"),
            Icon::Ext(ext) => format!("ext-{ext}"),
            Icon::Plain => "file".into(),
            Icon::Exec => "exec".into(),
        };
        self.0.dir.join(format!("{kind}{VERSION}-{name}-{px}.png"))
    }

    /// Last resort: a drawn icon, which needs nothing but a writable cache.
    fn drawn(&self, entry: &Entry, px: u32) -> Option<Thumb> {
        let icon = Icon::of(entry, &ext_of(entry));
        let out = self.icon_path(&icon, px, true);
        if let Some(t) = self.lookup(&out) {
            return Some(t);
        }
        let (width, height) = save_png(&out, px, px, &icon::draw(&icon, px))?;
        self.remember(&out, (width, height));
        Some(Thumb {
            path: out,
            width,
            height,
        })
    }

    fn lookup(&self, out: &Path) -> Option<Thumb> {
        let known = self.0.known.lock().ok()?.get(out).copied();
        let (width, height) = match known {
            Some(dims) => dims,
            None => {
                let dims = png_size(out)?;
                self.remember(out, dims);
                dims
            }
        };
        Some(Thumb {
            path: out.to_path_buf(),
            width,
            height,
        })
    }

    fn remember(&self, out: &Path, dims: (u32, u32)) {
        if let Ok(mut known) = self.0.known.lock() {
            known.insert(out.to_path_buf(), dims);
        }
    }
}

/// Folders whose Finder icon differs from the generic one. Matched by name
/// only: a plain folder that happens to share a name costs one extra render.
const SPECIAL_FOLDERS: &[&str] = &[
    "Applications",
    "Desktop",
    "Developer",
    "Documents",
    "Downloads",
    "Library",
    "Movies",
    "Music",
    "Pictures",
    "Public",
    "Sites",
    "System",
    "Users",
    "Utilities",
];

/// Folder extensions macOS shows as a single item with its own icon.
const PACKAGES: &[&str] = &[
    "appex",
    "bundle",
    "dsym",
    "framework",
    "kext",
    "mdimporter",
    "photoslibrary",
    "playground",
    "plugin",
    "prefpane",
    "qlgenerator",
    "saver",
    "scptd",
    "sparsebundle",
    "systemextension",
    "xcarchive",
    "xcodeproj",
    "xcworkspace",
    "xpc",
];

/// The rule from the module docs: Some when `entry` gets its own thumbnail.
fn content(entry: &Entry, ext: &str, px: u32, mode: Mode) -> Option<Class> {
    let class = class_of(ext);
    if entry.is_dir {
        if cfg!(target_os = "macos")
            && (ext == "app" || SPECIAL_FOLDERS.contains(&entry.name.as_str()))
        {
            return Some(Class::Other);
        }
        // Document packages such as `.pages` or `.rtfd` follow the file rules.
        if class != Some(Class::Other) {
            return None;
        }
    }
    let class = match class {
        Some(class) => class,
        None if mode == Mode::Preview && cfg!(target_os = "macos") => Class::Other,
        None => return None,
    };
    if !backend::renders(class, ext) {
        return None;
    }
    match (mode, class) {
        (Mode::Preview, _) | (_, Class::Image | Class::Video | Class::Pdf) => Some(class),
        (Mode::Icon, Class::Other) => (px > SMALL).then_some(class),
    }
}

pub(crate) fn class_of(ext: &str) -> Option<Class> {
    Some(match ext {
        "png" | "jpg" | "jpeg" | "jpe" | "jfif" | "gif" | "webp" | "bmp" | "tif" | "tiff"
        | "heic" | "heif" | "avif" | "jxl" | "ico" | "icns" | "tga" | "psd" | "svg" | "exr"
        | "hdr" | "dng" | "cr2" | "cr3" | "nef" | "arw" | "raf" | "orf" | "rw2" => Class::Image,
        "mp4" | "m4v" | "mov" | "qt" | "mkv" | "webm" | "avi" | "wmv" | "flv" | "mpg" | "mpeg"
        | "m2ts" | "3gp" | "ogv" => Class::Video,
        "pdf" => Class::Pdf,
        "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp" | "rtf"
        | "rtfd" | "pages" | "numbers" | "key" | "epub" | "ai" | "eps" | "ps" | "sketch"
        | "ttf" | "otf" | "ttc" | "woff" | "woff2" | "mp3" | "m4a" | "aac" | "flac" | "aiff"
        | "aif" | "usdz" | "usd" | "reality" | "obj" | "stl" | "glb" | "gltf" | "dae" | "scn" => {
            Class::Other
        }
        _ => return None,
    })
}

impl Icon {
    fn of(entry: &Entry, ext: &str) -> Icon {
        // Odd extensions (spaces, version numbers, unicode) are not worth an icon each.
        let simple = !ext.is_empty()
            && ext.len() <= 16
            && ext.bytes().any(|b| b.is_ascii_alphabetic())
            && ext
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+-_".contains(&b));
        if entry.is_dir {
            if cfg!(target_os = "macos") && simple && PACKAGES.contains(&ext) {
                return Icon::Package(ext.into());
            }
            return Icon::Folder;
        }
        if simple {
            Icon::Ext(ext.into())
        } else if ext.is_empty() && entry.mode & 0o111 != 0 {
            Icon::Exec
        } else {
            Icon::Plain
        }
    }
}

/// Lowercased extension of the name, for folders too (unlike `Entry::ext`).
fn ext_of(entry: &Entry) -> String {
    Path::new(&entry.name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Stable across runs and Rust versions, unlike `DefaultHasher`.
fn content_key(src: &Path, entry: &Entry, px: u32, mode: Mode) -> u64 {
    let mut h = Fnv::default();
    h.write_u32(VERSION);
    h.write(src.as_os_str().as_encoded_bytes());
    h.write_u8(0);
    h.write_u64(entry.size);
    let mtime = entry
        .modified
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .unwrap_or_default();
    h.write_u64(mtime.as_secs());
    h.write_u32(mtime.subsec_nanos());
    h.write_u32(px);
    h.write_u8(mode as u8);
    h.finish()
}

/// FNV-1a, 64 bit.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// Writes straight RGBA as a fast-compressed PNG. Atomic: a reader never sees
/// a partial file.
pub(crate) fn save_png(out: &Path, width: u32, height: u32, rgba: &[u8]) -> Option<(u32, u32)> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    write_atomic(out, &png)?;
    Some((width, height))
}

pub(crate) fn write_atomic(out: &Path, data: &[u8]) -> Option<()> {
    let tmp = tmp_path(out, "tmp");
    if std::fs::write(&tmp, data)
        .and_then(|_| std::fs::rename(&tmp, out))
        .is_err()
    {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(())
}

/// A sibling of `out` that no other writer, in this process or another, uses.
pub(crate) fn tmp_path(out: &Path, ext: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    out.with_extension(format!("{}-{n}.{ext}", std::process::id()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from("/data").join(name),
            is_dir,
            is_symlink: false,
            size: 1000,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            created: None,
            accessed: None,
            mode: 0o644,
        }
    }

    fn thumbs() -> Thumbnailer {
        Thumbnailer(Arc::new(Inner {
            dir: PathBuf::from("/cache"),
            cwd: PathBuf::from("/data"),
            known: Mutex::default(),
        }))
    }

    fn wants_content(t: &Thumbnailer, name: &str, is_dir: bool, px: u32, mode: Mode) -> bool {
        matches!(
            t.plan(&entry(name, is_dir), px, mode, false).what,
            What::Content(_)
        )
    }

    #[test]
    fn plain_folders_and_code_share_one_icon_per_type() {
        let t = thumbs();
        let a = t.plan(&entry("node_modules", true), 128, Mode::Icon, false);
        let b = t.plan(&entry("src", true), 128, Mode::Icon, false);
        assert_eq!(a.out, b.out);
        // Dotted folder names are not packages, so they share it too.
        let c = t.plan(&entry("com.apple.Safari", true), 128, Mode::Icon, false);
        assert_eq!(a.out, c.out);
        let js = t.plan(&entry("index.js", false), 256, Mode::Icon, false);
        let js2 = t.plan(&entry("util.JS", false), 256, Mode::Icon, false);
        assert_eq!(js.out, js2.out);
        assert_ne!(js.out, a.out);
    }

    #[test]
    fn content_rule_depends_on_size_and_mode() {
        let t = thumbs();
        for name in ["photo.HEIC", "clip.mov", "paper.pdf"] {
            assert!(wants_content(&t, name, false, 64, Mode::Icon), "{name}");
        }
        assert!(!wants_content(&t, "report.docx", false, 64, Mode::Icon));
        assert!(!wants_content(&t, "main.ts", false, 512, Mode::Icon));
        assert!(!wants_content(&t, "notes", false, 512, Mode::Icon));
        if cfg!(target_os = "macos") {
            assert!(wants_content(&t, "report.docx", false, 128, Mode::Icon));
            assert!(wants_content(&t, "Safari.app", true, 64, Mode::Icon));
            assert!(wants_content(&t, "Downloads", true, 64, Mode::Icon));
            assert!(wants_content(&t, "main.ts", false, 512, Mode::Preview));
        }
        assert!(!wants_content(&t, "src", true, 512, Mode::Preview));
    }

    #[test]
    fn content_key_tracks_the_file_and_the_request() {
        let t = thumbs();
        let base = entry("a.png", false);
        let key = |e: &Entry, px, mode| t.plan(e, px, mode, false).out;
        let k = key(&base, 128, Mode::Icon);

        // Relative and absolute spellings of one file share an entry.
        let mut rel = base.clone();
        rel.path = PathBuf::from("./a.png");
        assert_eq!(key(&rel, 128, Mode::Icon), k);

        let mut touched = base.clone();
        touched.modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_001));
        let mut grown = base.clone();
        grown.size += 1;
        for other in [
            key(&touched, 128, Mode::Icon),
            key(&grown, 128, Mode::Icon),
            key(&base, 256, Mode::Icon),
            key(&base, 128, Mode::Preview),
        ] {
            assert_ne!(other, k);
        }
        // Stable across runs and toolchains: this name is on disk in users' caches.
        assert_eq!(k.file_name().unwrap(), "a9693c76d25a351e.png");
    }

    #[test]
    fn falls_back_to_type_icon_when_forced() {
        let t = thumbs();
        let job = t.plan(&entry("clip.mov", false), 128, Mode::Icon, true);
        assert!(matches!(job.what, What::Icon(Icon::Ext(ref e)) if e == "mov"));
    }
}
