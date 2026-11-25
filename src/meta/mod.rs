//! File kinds ("PDF document") and the details shown under "Information".
//!
//! `kind` is a table lookup on the extension. `info` asks the platform
//! (Spotlight and LaunchServices on macOS, ffprobe and pdfinfo on Linux) and
//! falls back to reading file headers, which also covers folders Spotlight
//! does not index, such as /tmp.

#[cfg(not(target_os = "macos"))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(not(target_os = "macos"))]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
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
    let ext = Path::new(&entry.name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if entry.is_dir {
        return match ext.as_str() {
            "app" => "Application",
            "bundle" | "plugin" | "appex" => "Bundle",
            "framework" => "Framework",
            "xcodeproj" => "Xcode Project",
            "xcworkspace" => "Xcode Workspace",
            "photoslibrary" => "Photos Library",
            "rtfd" => "RTF with attachments",
            "pages" => "Pages document",
            "numbers" => "Numbers Document",
            "key" => "Keynote document",
            _ => "Folder",
        }
        .into();
    }
    if let Some(known) = known_kind(&ext) {
        return known.into();
    }
    if ext.is_empty() {
        if entry.mode & 0o111 != 0 {
            "Unix executable".into()
        } else {
            "Document".into()
        }
    } else {
        format!("{} file", ext.to_uppercase())
    }
}

fn known_kind(ext: &str) -> Option<&'static str> {
    Some(match ext {
        // Documents
        "pdf" => "PDF document",
        "txt" | "text" => "Plain Text Document",
        "md" | "markdown" => "Markdown Document",
        "rtf" => "Rich Text Document",
        "doc" => "Microsoft Word 97-2004 document",
        "docx" => "Microsoft Word document",
        "xls" => "Microsoft Excel 97-2004 workbook",
        "xlsx" => "Microsoft Excel workbook",
        "ppt" => "PowerPoint 97-2004 presentation",
        "pptx" => "PowerPoint presentation",
        "pages" => "Pages document",
        "numbers" => "Numbers Document",
        "key" => "Keynote document",
        "odt" => "OpenDocument text",
        "ods" => "OpenDocument spreadsheet",
        "odp" => "OpenDocument presentation",
        "epub" => "Electronic Publication (EPUB)",
        "csv" => "Comma Separated Values",
        "tsv" => "TSV document",
        "log" => "Log File",
        "ics" => "ICS file",
        "vcf" => "vCard",
        "eml" => "Email message",
        "webloc" => "Web site location",
        // Images
        "png" => "PNG image",
        "jpg" | "jpeg" | "jpe" | "jfif" => "JPEG image",
        "gif" => "GIF image",
        "heic" | "heif" => "HEIF Image",
        "avif" => "AV1 Image File Format",
        "webp" => "WebP Image",
        "tif" | "tiff" => "TIFF image",
        "bmp" => "Windows BMP image",
        "ico" => "Windows icon image",
        "icns" => "Apple icon image",
        "svg" => "SVG document",
        "psd" => "Adobe Photoshop document",
        "ai" => "Adobe Illustrator file",
        "eps" => "EPS document",
        "tga" => "TGA image",
        "exr" => "OpenEXR image",
        "jxl" => "JPEG XL image",
        "dng" => "DNG raw image",
        "cr2" | "cr3" => "Canon raw image",
        "nef" => "Nikon raw image",
        "arw" => "Sony raw image",
        "raf" => "Fujifilm raw image",
        "sketch" => "Sketch document",
        "fig" => "Figma document",
        // Video
        "mp4" => "MPEG-4 movie",
        "m4v" => "Apple MPEG-4 movie",
        "mov" | "qt" => "QuickTime movie",
        "mkv" => "Matroska video",
        "webm" => "WebM Media",
        "avi" => "AVI movie",
        "wmv" => "Windows Media video",
        "flv" => "Flash video",
        "mpg" | "mpeg" => "MPEG movie",
        "m2ts" => "AVCHD video",
        "3gp" => "3GPP movie",
        // Audio
        "mp3" => "MP3 audio",
        "m4a" => "Apple MPEG-4 audio",
        "aac" => "AAC audio",
        "wav" => "Waveform audio",
        "aif" | "aiff" => "AIFF audio",
        "flac" => "FLAC audio",
        "ogg" | "oga" => "Ogg Audio",
        "opus" => "Opus audio",
        "mid" | "midi" => "MIDI audio",
        // Archives and disk images
        "zip" => "ZIP archive",
        "tar" => "tar archive",
        "gz" => "gzip compressed archive",
        "tgz" => "gzip compressed tar archive",
        "bz2" => "bzip2 compressed archive",
        "xz" => "xz compressed archive",
        "zst" => "Zstandard compressed archive",
        "7z" => "7-Zip Archive",
        "rar" => "RAR archive",
        "dmg" => "Disk Image",
        "iso" => "ISO Disk Image",
        "pkg" => "Installer flat package",
        "deb" => "Debian package",
        "rpm" => "RPM package",
        "jar" => "Java JAR file",
        "apk" => "Android package",
        // Fonts
        "ttf" => "TrueType® font",
        "otf" => "OpenType® font",
        "ttc" => "TrueType font collection",
        "woff" | "woff2" => "Web font",
        // Code and data
        "rs" => "Rust source code",
        "c" => "C Source",
        "h" => "C Header Source",
        "cc" | "cpp" | "cxx" => "C++ Source",
        "hpp" | "hh" | "hxx" => "C++ header",
        "m" => "Objective-C source",
        "mm" => "Objective-C++ source",
        "swift" => "Swift Source",
        "go" => "Go source code",
        "py" => "Python Script",
        "rb" => "Ruby Source",
        "js" | "mjs" | "cjs" => "JavaScript script",
        "ts" | "mts" | "cts" => "TypeScript source",
        "jsx" => "JavaScript JSX source",
        "tsx" => "TypeScript JSX source",
        "java" => "Java source",
        "kt" | "kts" => "Kotlin source",
        "scala" => "Scala source",
        "cs" => "C# source",
        "php" => "PHP Script",
        "lua" => "Lua script",
        "pl" => "Perl script",
        "r" => "R script",
        "dart" => "Dart source",
        "zig" => "Zig source",
        "ex" | "exs" => "Elixir source",
        "hs" => "Haskell source",
        "sql" => "SQL File",
        "sh" | "bash" | "zsh" | "fish" => "Shell Script",
        "html" | "htm" => "HTML document",
        "css" => "CSS style sheet",
        "scss" => "Sass style sheet",
        "json" => "JSON Document",
        "yaml" | "yml" => "YAML Document",
        "toml" => "TOML document",
        "xml" => "XML Document",
        "plist" => "Property List",
        "ini" | "cfg" | "conf" => "Configuration file",
        "lock" => "Lock file",
        "patch" | "diff" => "Patch file",
        "ipynb" => "Jupyter notebook",
        "wasm" => "WebAssembly module",
        "o" => "Object code",
        "a" => "Static library",
        "dylib" => "Dynamic library",
        "so" => "Shared library",
        _ => return None,
    })
}

/// Full details. May spawn helper processes, so call it off the UI thread.
pub fn info(entry: &Entry) -> Info {
    let mut info = Info {
        kind: kind(entry),
        ..Info::default()
    };
    if entry.is_dir && info.kind == "Folder" {
        // Finder counts what it shows, so hidden files are left out.
        info.items = std::fs::read_dir(&entry.path).ok().map(|d| {
            d.filter(|e| {
                e.as_ref()
                    .is_ok_and(|e| !e.file_name().as_encoded_bytes().starts_with(b"."))
            })
            .count()
        });
    }
    platform::fill(entry, &mut info);
    if !entry.is_dir
        && (info.duration.is_none() || info.dimensions.is_none())
        && let Some(movie) = movie(&entry.path)
    {
        info.duration = info.duration.or(movie.duration);
        info.dimensions = info.dimensions.or(movie.dimensions);
    }
    info
}

pub(crate) struct Movie {
    pub duration: Option<Duration>,
    pub dimensions: Option<(u32, u32)>,
}

/// Duration and frame size of an MP4 or QuickTime file, read from its `moov`
/// box (no decoding, no helper tools).
pub(crate) fn movie(path: &Path) -> Option<Movie> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    if !matches!(
        ext.as_str(),
        "mp4" | "m4v" | "m4a" | "m4b" | "mov" | "qt" | "3gp" | "3g2"
    ) {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let moov = loop {
        let (kind, len) = box_header(&mut file)?;
        if &kind == b"moov" {
            // moov holds only tables; anything huge is not worth reading.
            if len > 64 << 20 {
                return None;
            }
            let mut moov = vec![0; len as usize];
            file.read_exact(&mut moov).ok()?;
            break moov;
        }
        file.seek(SeekFrom::Current(i64::try_from(len).ok()?))
            .ok()?;
    };
    let mut movie = Movie {
        duration: None,
        dimensions: None,
    };
    for (kind, body) in boxes(&moov) {
        match &kind {
            b"mvhd" => movie.duration = mvhd_duration(body),
            b"trak" if movie.dimensions.is_none() => {
                movie.dimensions = boxes(body)
                    .find(|(k, _)| k == b"tkhd")
                    .and_then(|(_, b)| tkhd_size(b));
            }
            _ => {}
        }
    }
    Some(movie)
}

/// Reads a box header and returns its type and payload length.
fn box_header(file: &mut File) -> Option<([u8; 4], u64)> {
    let mut head = [0u8; 8];
    file.read_exact(&mut head).ok()?;
    let kind = head[4..8].try_into().ok()?;
    let len = match u32::from_be_bytes(head[..4].try_into().ok()?) {
        1 => {
            let mut big = [0u8; 8];
            file.read_exact(&mut big).ok()?;
            u64::from_be_bytes(big).checked_sub(16)?
        }
        0 => return None,
        n => u64::from(n).checked_sub(8)?,
    };
    Some((kind, len))
}

/// Child boxes inside a box payload.
fn boxes(mut data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let len = u32::from_be_bytes(data.get(..4)?.try_into().ok()?) as usize;
        let kind = data.get(4..8)?.try_into().ok()?;
        let body = data.get(8..len.max(8))?;
        data = &data[len.max(8)..];
        Some((kind, body))
    })
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn mvhd_duration(b: &[u8]) -> Option<Duration> {
    let (scale, units) = match b.first()? {
        0 => (be32(b, 12)?, u64::from(be32(b, 16)?)),
        _ => (be32(b, 20)?, be64(b, 24)?),
    };
    // Checked: a zero scale or a corrupt length must not panic.
    Duration::try_from_secs_f64(units as f64 / scale as f64).ok()
}

/// Display size of a track, rotated as the player shows it. None for tracks
/// without pictures.
fn tkhd_size(b: &[u8]) -> Option<(u32, u32)> {
    let (matrix, size) = match b.first()? {
        0 => (40, 76),
        _ => (52, 88),
    };
    let (w, h) = (be32(b, size)? >> 16, be32(b, size + 4)? >> 16);
    // A non-zero second matrix entry means a quarter turn.
    let turned = be32(b, matrix + 4)? != 0;
    match (w, h) {
        (0, _) | (_, 0) => None,
        _ if turned => Some((h, w)),
        _ => Some((w, h)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn mp4_box(kind: &[u8], body: &[u8]) -> Vec<u8> {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend(kind);
        b.extend(body);
        b
    }

    #[test]
    fn reads_movie_duration_and_rotated_size() {
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&600u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&(600u32 * 75).to_be_bytes());
        let mut tkhd = vec![0u8; 84];
        tkhd[44..48].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        tkhd[76..80].copy_from_slice(&(1920u32 << 16).to_be_bytes());
        tkhd[80..84].copy_from_slice(&(1080u32 << 16).to_be_bytes());
        let trak = mp4_box(b"trak", &mp4_box(b"tkhd", &tkhd));
        let moov = mp4_box(b"moov", &[mp4_box(b"mvhd", &mvhd), trak].concat());
        let file = [mp4_box(b"ftyp", b"isom"), mp4_box(b"mdat", &[0; 32]), moov].concat();

        let dir = std::env::temp_dir().join(format!("lsd-meta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clip.mov");
        File::create(&path).unwrap().write_all(&file).unwrap();
        let movie = movie(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(movie.duration, Some(Duration::from_secs(75)));
        assert_eq!(movie.dimensions, Some((1080, 1920)));
    }
}
