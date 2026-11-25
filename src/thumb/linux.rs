//! Linux and other freedesktop systems: images through the `image` crate,
//! video through ffmpegthumbnailer or ffmpeg, PDF through pdftoppm, each only
//! when installed. Thumbnails another app already made in the freedesktop
//! cache (~/.cache/thumbnails) are reused when their Thumb::MTime matches.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use image::{DynamicImage, ImageDecoder, ImageReader};

use super::{Class, Job, What, icon, png_size, save_png, tmp_path, write_atomic};

/// A stuck tool is killed after this long.
const TIMEOUT: Duration = Duration::from_secs(5);

pub fn renders(class: Class, ext: &str) -> bool {
    match class {
        Class::Image => {
            image::ImageFormat::from_extension(ext).is_some_and(|f| f.reading_enabled())
        }
        Class::Video => tools().ffmpegthumbnailer.is_some() || tools().ffmpeg.is_some(),
        Class::Pdf => tools().pdftoppm.is_some(),
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

fn render(job: &Job) -> Option<(u32, u32)> {
    match &job.what {
        What::Icon(icon) => save_png(&job.out, job.px, job.px, &icon::draw(icon, job.px)),
        What::Content(class) => freedesktop(job).or_else(|| match class {
            Class::Image => image_thumb(&job.src, job.px, &job.out),
            Class::Video => video_thumb(job),
            Class::Pdf => pdf_thumb(job),
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
    // Downscale only; the terminal scales small pictures up itself.
    if img.width() > px || img.height() > px {
        img = img.thumbnail(px, px);
    }
    let rgba = img.into_rgba8();
    save_png(out, rgba.width(), rgba.height(), &rgba)
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
}

fn tools() -> &'static Tools {
    static TOOLS: OnceLock<Tools> = OnceLock::new();
    TOOLS.get_or_init(|| Tools {
        ffmpegthumbnailer: which("ffmpegthumbnailer"),
        ffmpeg: which("ffmpeg"),
        pdftoppm: which("pdftoppm"),
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
