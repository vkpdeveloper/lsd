//! Linux and other freedesktop systems: images through the `image` crate,
//! video through ffmpegthumbnailer or ffmpeg, PDF through pdftoppm, each only
//! when installed. Thumbnails another app already made in the freedesktop
//! cache (~/.cache/thumbnails) are reused when their Thumb::MTime matches.
//!
//! Folder icons come from the icon theme, picked the way Nautilus does: home
//! and the XDG user dirs get their own icon names, the rest get `folder`.
//! Current Adwaita ships those as SVG only (its PNGs stop at 16 px), so they
//! are drawn with rsvg-convert when it is installed, like the other tools.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use image::{DynamicImage, ImageDecoder, ImageReader};

use super::{
    Class, Icon, Job, What, book_thumb, icon, png_size, save_png, save_scaled, tmp_path,
    write_atomic,
};

/// A stuck tool is killed after this long.
const TIMEOUT: Duration = Duration::from_secs(5);

pub fn renders(class: Class, ext: &str) -> bool {
    match class {
        Class::Image => {
            image::ImageFormat::from_extension(ext).is_some_and(|f| f.reading_enabled())
        }
        Class::Video => tools().ffmpegthumbnailer.is_some() || tools().ffmpeg.is_some(),
        Class::Pdf => tools().pdftoppm.is_some(),
        Class::Book => true,
        Class::Other => false,
    }
}

/// Renders every job on a pool of threads, calling `done` once per job index
/// (on the calling thread) as results arrive.
pub fn generate(jobs: &[Job], done: &mut dyn FnMut(usize, Option<(u32, u32)>)) {
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(jobs.len());
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|s| {
        for _ in 0..workers {
            let tx = tx.clone();
            let next = &next;
            s.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else { break };
                    if tx.send((i, render(job))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        for (i, dims) in rx {
            done(i, dims);
        }
    });
}

/// Cache name prefix for a type icon. Folder icons follow the icon theme, so
/// theirs changes with the GNOME settings; the rest are drawn.
pub fn icon_kind(icon: &Icon) -> String {
    match icon {
        Icon::Folder | Icon::Place(_) => format!("theme-{:x}-v", settings_stamp()),
        _ => "draw".into(),
    }
}

fn render(job: &Job) -> Option<(u32, u32)> {
    match &job.what {
        What::Icon(icon) => {
            let themed = match icon {
                Icon::Folder => theme_icon("folder", job),
                Icon::Place(name) => theme_icon(name, job).or_else(|| theme_icon("folder", job)),
                _ => None,
            };
            // A folder without a usable theme icon is drawn and cached under
            // the theme name too, so later runs skip the theme lookup.
            themed.or_else(|| save_png(&job.out, job.px, job.px, &icon::draw(icon, job.px)))
        }
        What::Content(class) => freedesktop(job).or_else(|| match class {
            Class::Image => image_thumb(&job.src, job.px, &job.out),
            Class::Video => video_thumb(job),
            Class::Pdf => pdf_thumb(job),
            Class::Book => book_thumb(job),
            Class::Other => None,
        }),
    }
}

pub(super) fn image_thumb(src: &Path, px: u32, out: &Path) -> Option<(u32, u32)> {
    let mut decoder = ImageReader::open(src)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    save_scaled(img, px, out)
}

fn video_thumb(job: &Job) -> Option<(u32, u32)> {
    let tools = tools();
    let tmp = tmp_path(&job.out, "png");
    let px = job.px.to_string();
    let ok = match (&tools.ffmpegthumbnailer, &tools.ffmpeg) {
        (Some(ft), _) => run(Command::new(ft)
            .arg("-i")
            .arg(&job.src)
            .arg("-o")
            .arg(&tmp)
            .args(["-s", &px, "-c", "png"])),
        (None, Some(ffmpeg)) => {
            // A frame a little way in is more telling than the first, which is
            // often black. Short clips without a frame there fall back to 0.
            let seek = crate::meta::movie(&job.src)
                .and_then(|m| m.duration)
                .map_or(1.0, |d| (d.as_secs_f64() * 0.1).min(30.0));
            [seek, 0.0].into_iter().any(|t| {
                let scale = format!("scale={px}:{px}:force_original_aspect_ratio=decrease");
                run(Command::new(ffmpeg)
                    .args(["-v", "error", "-nostdin", "-y", "-ss", &t.to_string(), "-i"])
                    .arg(&job.src)
                    .args([
                        "-frames:v",
                        "1",
                        "-vf",
                        &scale,
                        "-f",
                        "image2",
                        "-c:v",
                        "png",
                    ])
                    .arg(&tmp))
                    && tmp.exists()
            })
        }
        (None, None) => false,
    };
    finish(ok, &tmp, &job.out)
}

fn pdf_thumb(job: &Job) -> Option<(u32, u32)> {
    let pdftoppm = tools().pdftoppm.as_ref()?;
    // pdftoppm appends ".png" to the prefix it is given.
    let tmp = tmp_path(&job.out, "png");
    let ok = run(Command::new(pdftoppm)
        .args(["-png", "-f", "1", "-l", "1", "-singlefile", "-scale-to"])
        .arg(job.px.to_string())
        .arg(&job.src)
        .arg(tmp.with_extension("")));
    finish(ok, &tmp, &job.out)
}

/// Moves a tool's output into place.
fn finish(ok: bool, tmp: &Path, out: &Path) -> Option<(u32, u32)> {
    if ok && png_size(tmp).is_some() && std::fs::rename(tmp, out).is_ok() {
        return png_size(out);
    }
    let _ = std::fs::remove_file(tmp);
    None
}

/// Runs a tool quietly, killing it if it takes longer than [`TIMEOUT`].
fn run(cmd: &mut Command) -> bool {
    let Ok(mut child) = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

struct Tools {
    ffmpegthumbnailer: Option<PathBuf>,
    ffmpeg: Option<PathBuf>,
    pdftoppm: Option<PathBuf>,
    rsvg_convert: Option<PathBuf>,
}

fn tools() -> &'static Tools {
    static TOOLS: OnceLock<Tools> = OnceLock::new();
    TOOLS.get_or_init(|| Tools {
        ffmpegthumbnailer: which("ffmpegthumbnailer"),
        ffmpeg: which("ffmpeg"),
        pdftoppm: which("pdftoppm"),
        rsvg_convert: which("rsvg-convert"),
    })
}

pub(crate) fn which(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// The icon name Nautilus gives a special folder: home and the XDG user dirs
/// from `user-dirs.dirs`.
pub(super) fn place(path: &Path) -> Option<&'static str> {
    static PLACES: OnceLock<Vec<(PathBuf, &'static str)>> = OnceLock::new();
    PLACES
        .get_or_init(|| {
            let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
                return Vec::new();
            };
            let file = config_home(&home).join("user-dirs.dirs");
            let mut places = user_dirs(&std::fs::read_to_string(file).unwrap_or_default(), &home);
            places.push((home, "user-home"));
            places
        })
        .iter()
        .find(|(p, _)| p == path)
        .map(|&(_, name)| name)
}

/// Reads `XDG_DOWNLOAD_DIR="$HOME/Downloads"` lines. A dir set to home itself
/// is turned off, per the spec.
fn user_dirs(text: &str, home: &Path) -> Vec<(PathBuf, &'static str)> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once('=')?;
            let name = match key.strip_prefix("XDG_")?.strip_suffix("_DIR")? {
                "DESKTOP" => "user-desktop",
                "DOWNLOAD" => "folder-download",
                "TEMPLATES" => "folder-templates",
                "PUBLICSHARE" => "folder-publicshare",
                "DOCUMENTS" => "folder-documents",
                "MUSIC" => "folder-music",
                "PICTURES" => "folder-pictures",
                "VIDEOS" => "folder-videos",
                _ => return None,
            };
            let value = value.trim().strip_prefix('"')?.strip_suffix('"')?;
            let path = match value.strip_prefix("$HOME") {
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None => PathBuf::from(value),
            };
            (path.is_absolute() && path != home).then_some((path, name))
        })
        .collect()
}

fn config_home(home: &Path) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
}

/// Changes whenever a GNOME setting does, icon theme included. Asking
/// gsettings for the theme takes about 8 ms, too slow for every listing; this
/// is one stat.
fn settings_stamp() -> u64 {
    static STAMP: OnceLock<u64> = OnceLock::new();
    *STAMP.get_or_init(|| {
        std::env::var_os("HOME")
            .and_then(|home| {
                config_home(Path::new(&home))
                    .join("dconf/user")
                    .metadata()
                    .ok()
            })
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs())
    })
}

/// Renders the icon theme's `name` into `job.out`, or None when no theme has
/// a usable one.
fn theme_icon(name: &str, job: &Job) -> Option<(u32, u32)> {
    // Like GTK, the first theme that has the icon decides, even when lsd cannot
    // draw what it has. A PNG much smaller than wanted is skipped too: lsd's
    // drawn folder looks better than a 16 px icon blown up.
    let files = themes()
        .iter()
        .map(|theme| files_in(theme, name))
        .find(|files| !files.is_empty())?;
    let svg_ok = tools().rsvg_convert.is_some();
    let (_, svg, file) = files
        .into_iter()
        .filter(|&(size, svg, _)| if svg { svg_ok } else { size >= job.px.min(48) })
        .max_by_key(|&(size, svg, _)| (svg, size))?;
    if !svg {
        return save_scaled(image::open(&file).ok()?, job.px, &job.out);
    }
    let rsvg = tools().rsvg_convert.as_ref()?;
    let tmp = tmp_path(&job.out, "png");
    let px = job.px.to_string();
    let ok = run(Command::new(rsvg)
        .args(["-w", &px, "-h", &px, "-a", "-o"])
        .arg(&tmp)
        .arg(&file));
    finish(ok, &tmp, &job.out)
}

/// Theme directories in lookup order: the GNOME icon theme and what it
/// inherits, then Adwaita and hicolor.
fn themes() -> &'static [PathBuf] {
    static THEMES: OnceLock<Vec<PathBuf>> = OnceLock::new();
    THEMES.get_or_init(|| {
        let bases = icon_bases();
        let mut names: Vec<String> = gsettings_theme().into_iter().collect();
        names.extend(["Adwaita".into(), "hicolor".into()]);
        let mut dirs = Vec::new();
        let mut i = 0;
        while i < names.len() {
            let found: Vec<PathBuf> = bases
                .iter()
                .map(|b| b.join(&names[i]))
                .filter(|d| d.is_dir())
                .collect();
            let index = found
                .iter()
                .find_map(|d| std::fs::read_to_string(d.join("index.theme")).ok())
                .unwrap_or_default();
            let parents = index
                .lines()
                .find_map(|l| l.strip_prefix("Inherits="))
                .unwrap_or_default();
            // Parents come right after their child, before the fallbacks.
            let mut at = i + 1;
            for parent in parents.split(',').map(str::trim) {
                if !parent.is_empty() && !names.iter().any(|n| n == parent) {
                    names.insert(at, parent.into());
                    at += 1;
                }
            }
            dirs.extend(found);
            i += 1;
        }
        dirs
    })
}

/// Where icon themes live: the user's data dir, `~/.icons`, then the system
/// data dirs.
fn icon_bases() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    data_home
        .into_iter()
        .map(|d| d.join("icons"))
        .chain(home.map(|h| h.join(".icons")))
        .chain(std::env::split_paths(&data_dirs).map(|d| d.join("icons")))
        .collect()
}

fn gsettings_theme() -> Option<String> {
    let out = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let name = String::from_utf8(out.stdout).ok()?;
    let name = name.trim().trim_matches('\'');
    (out.status.success() && !name.is_empty()).then(|| name.into())
}

/// Every file for `name` in one theme's places: its size, whether it is an
/// SVG, and its path.
fn files_in(theme: &Path, name: &str) -> Vec<(u32, bool, PathBuf)> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(theme).into_iter().flatten().flatten() {
        let top = entry.file_name().to_string_lossy().into_owned();
        // Adwaita nests `48x48/places`, Breeze `places/48`.
        let dirs: Vec<(String, PathBuf)> = if top == "places" {
            std::fs::read_dir(entry.path())
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                .collect()
        } else {
            vec![(top, entry.path().join("places"))]
        };
        for (size_dir, dir) in dirs {
            let Some(size) = dir_size(&size_dir) else {
                continue;
            };
            for (ext, svg) in [("svg", true), ("png", false)] {
                let file = dir.join(format!("{name}.{ext}"));
                if file.is_file() {
                    files.push((size, svg, file));
                }
            }
        }
    }
    files
}

/// Pixel size from a theme directory name: `48x48`, `48`, `48x48@2`, `48@2x`
/// or `scalable`.
fn dir_size(name: &str) -> Option<u32> {
    if name == "scalable" {
        return Some(512);
    }
    let (size, scale) = name.split_once('@').unwrap_or((name, "1"));
    let scale: u32 = scale.trim_end_matches('x').parse().ok()?;
    let size = size.split_once('x').map_or(size, |(w, _)| w);
    Some(size.parse::<u32>().ok()? * scale)
}

/// Copies a matching thumbnail from the shared freedesktop cache.
fn freedesktop(job: &Job) -> Option<(u32, u32)> {
    let mtime = job.src.metadata().ok()?.modified().ok()?;
    let mtime = mtime.duration_since(UNIX_EPOCH).ok()?.as_secs().to_string();
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))?
        .join("thumbnails");
    let name = format!("{:x}.png", md5::compute(file_uri(&job.src)));
    let sizes = [
        ("normal", 128),
        ("large", 256),
        ("x-large", 512),
        ("xx-large", 1024),
    ];
    for (dir, size) in sizes {
        if size < job.px.min(1024) {
            continue;
        }
        let Ok(png) = std::fs::read(base.join(dir).join(&name)) else {
            continue;
        };
        if png_text(&png, "Thumb::MTime") == Some(mtime.as_bytes()) {
            write_atomic(&job.out, &png)?;
            return png_size(&job.out);
        }
    }
    None
}

/// `file://` URI the way GLib's `g_filename_to_uri` writes it, which is what
/// the freedesktop cache hashes.
pub(super) fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &b in OsStr::as_encoded_bytes(path.as_os_str()) {
        if b.is_ascii_alphanumeric() || b"!$&'()*+,-./:=@_~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// The value of a `tEXt` chunk.
pub(super) fn png_text<'a>(png: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let mut rest = png.strip_prefix(b"\x89PNG\r\n\x1a\n")?;
    while rest.len() >= 12 {
        let len = u32::from_be_bytes(rest[..4].try_into().ok()?) as usize;
        let kind = &rest[4..8];
        let data = rest.get(8..8 + len)?;
        if kind == b"tEXt"
            && let Some(value) = data
                .strip_prefix(key.as_bytes())
                .and_then(|d| d.strip_prefix(b"\0"))
        {
            return Some(value);
        }
        if kind == b"IEND" {
            break;
        }
        rest = rest.get(12 + len..)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freedesktop_name_matches_spec_example() {
        // From the freedesktop thumbnail spec.
        let uri = file_uri(Path::new("/home/jens/photos/me.png"));
        assert_eq!(uri, "file:///home/jens/photos/me.png");
        assert_eq!(
            format!("{:x}", md5::compute(uri)),
            "c6ee772d9e49320e97ec29a7eb5b1697"
        );
    }

    #[test]
    fn uri_escapes_like_glib() {
        let path = Path::new("/tmp/a b;c#d/caf\u{e9}+x(1).png");
        assert_eq!(
            file_uri(path),
            "file:///tmp/a%20b%3Bc%23d/caf%C3%A9+x(1).png"
        );
    }

    #[test]
    fn maps_xdg_user_dirs_to_icon_names() {
        let home = Path::new("/home/ana");
        let text = r#"
# comment
XDG_DESKTOP_DIR="$HOME/Desktop"
XDG_DOWNLOAD_DIR="/data/dl"
XDG_MUSIC_DIR="$HOME"
XDG_VIDEOS_DIR="$HOME/Movies"
XDG_UNKNOWN_DIR="$HOME/x"
"#;
        assert_eq!(
            user_dirs(text, home),
            [
                (PathBuf::from("/home/ana/Desktop"), "user-desktop"),
                (PathBuf::from("/data/dl"), "folder-download"),
                (PathBuf::from("/home/ana/Movies"), "folder-videos"),
            ]
        );
    }

    #[test]
    fn reads_theme_dir_sizes() {
        for (name, size) in [
            ("48x48", Some(48)),
            ("48", Some(48)),
            ("24@2x", Some(48)),
            ("16x16@2", Some(32)),
            ("scalable", Some(512)),
            ("symbolic", None),
            ("cursors", None),
        ] {
            assert_eq!(dir_size(name), size, "{name}");
        }
    }

    #[test]
    fn reads_text_chunks() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |kind: &[u8], data: &[u8]| {
            png.extend((data.len() as u32).to_be_bytes());
            png.extend(kind);
            png.extend(data);
            png.extend([0; 4]);
        };
        chunk(b"IHDR", &[0; 13]);
        chunk(b"tEXt", b"Thumb::URI\0file:///x");
        chunk(b"tEXt", b"Thumb::MTime\x001700000000");
        chunk(b"IEND", b"");
        assert_eq!(png_text(&png, "Thumb::MTime"), Some(&b"1700000000"[..]));
        assert_eq!(png_text(&png, "Thumb::Size"), None);
    }
}
