//! Text icons for terminals without images, and for rows too small for one.

use crate::entry::Entry;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Icons {
    /// Emoji, which every modern terminal font stack can show.
    #[default]
    Emoji,
    /// Nerd Font glyphs, for fonts patched with them.
    Nerd,
    /// Names only.
    None,
}

impl Icons {
    /// `--icons`, else `LSD_ICONS`, else emoji.
    pub fn resolve(flag: Option<Icons>) -> Icons {
        if let Some(icons) = flag {
            return icons;
        }
        match std::env::var("LSD_ICONS").as_deref() {
            Ok("nerd") => Icons::Nerd,
            Ok("none" | "off") => Icons::None,
            _ => Icons::Emoji,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Folder,
    App,
    Image,
    Video,
    Audio,
    Pdf,
    Archive,
    Code,
    Text,
    Other,
}

pub fn class(entry: &Entry) -> Class {
    if entry.is_dir {
        return if entry.name.ends_with(".app") {
            Class::App
        } else {
            Class::Folder
        };
    }
    match entry.ext().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "tif" | "tiff" | "bmp"
        | "svg" | "ico" | "avif" | "raw" | "cr2" | "nef" | "arw" | "dng" | "psd" => Class::Image,
        "mp4" | "mov" | "m4v" | "mkv" | "webm" | "avi" | "wmv" | "flv" => Class::Video,
        "mp3" | "m4a" | "aac" | "flac" | "wav" | "ogg" | "opus" | "aiff" => Class::Audio,
        "pdf" => Class::Pdf,
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "dmg" | "iso"
        | "pkg" | "deb" | "rpm" => Class::Archive,
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "c" | "h" | "cpp" | "hpp" | "swift"
        | "kt" | "java" | "rb" | "php" | "sh" | "zsh" | "fish" | "lua" | "zig" | "json"
        | "toml" | "yaml" | "yml" | "html" | "css" | "scss" | "sql" => Class::Code,
        "txt" | "md" | "markdown" | "rtf" | "doc" | "docx" | "pages" | "csv" | "log" => Class::Text,
        _ if entry.mode & 0o111 != 0 => Class::App,
        _ => Class::Other,
    }
}

/// The icon followed by a space, or "" with `Icons::None`. Emoji take two
/// columns; Nerd Font glyphs take one.
pub fn icon(entry: &Entry, icons: Icons) -> &'static str {
    let c = class(entry);
    match icons {
        Icons::None => "",
        Icons::Emoji => match c {
            Class::Folder => "📁 ",
            Class::App => "🚀 ",
            Class::Image => "🌄 ",
            Class::Video => "🎬 ",
            Class::Audio => "🎵 ",
            Class::Pdf => "📕 ",
            Class::Archive => "📦 ",
            Class::Code => "📜 ",
            Class::Text => "📝 ",
            Class::Other => "📄 ",
        },
        Icons::Nerd => match c {
            Class::Folder => "\u{f07b} ",
            Class::App => "\u{f135} ",
            Class::Image => "\u{f1c5} ",
            Class::Video => "\u{f1c8} ",
            Class::Audio => "\u{f1c7} ",
            Class::Pdf => "\u{f1c1} ",
            Class::Archive => "\u{f1c6} ",
            Class::Code => "\u{f1c9} ",
            Class::Text => "\u{f15c} ",
            Class::Other => "\u{f15b} ",
        },
    }
}

/// SGR for a name, using the terminal's own palette so every theme fits.
pub fn name_style(entry: &Entry) -> &'static str {
    match class(entry) {
        Class::Folder => "\x1b[1;34m",
        Class::App => "\x1b[1;32m",
        Class::Image => "\x1b[35m",
        Class::Video => "\x1b[35m",
        Class::Audio => "\x1b[36m",
        Class::Archive => "\x1b[31m",
        _ => "",
    }
}
