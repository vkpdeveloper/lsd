//! Book covers, for thumbnails where the system has no EPUB thumbnailer (most
//! Linux desktops). An EPUB is a zip: META-INF/container.xml names the package
//! file, and the package names the cover image.

use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use zip::ZipArchive;

/// Bigger covers are skipped instead of read.
const MAX_COVER: u64 = 20 << 20;
/// container.xml and the package file are a few KB; this only stops a hostile zip.
const MAX_XML: u64 = 4 << 20;

/// The cover image's bytes (JPEG, PNG...), if the book has one.
pub fn cover(path: &Path) -> Option<Vec<u8>> {
    cover_from(File::open(path).ok()?)
}

fn cover_from(reader: impl Read + Seek) -> Option<Vec<u8>> {
    let mut zip = ZipArchive::new(reader).ok()?;
    let container = read_text(&mut zip, "META-INF/container.xml")?;
    let package_path = tags(&container, "rootfile").find_map(|t| attr(t, "full-path"))?;
    let package = read_text(&mut zip, &package_path)?;
    let href = cover_href(&package)?;
    // Hrefs are relative to the package file.
    let dir = package_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let mut file = zip.by_name(&resolve(dir, &href)).ok()?;
    if file.size() > MAX_COVER {
        return None;
    }
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn read_text(zip: &mut ZipArchive<impl Read + Seek>, name: &str) -> Option<String> {
    let mut text = String::new();
    zip.by_name(name)
        .ok()?
        .take(MAX_XML)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// EPUB 3 marks the cover item; EPUB 2 points at it from a `<meta>`. Older
/// books often do neither, so an image named like a cover is the fallback.
fn cover_href(package: &str) -> Option<String> {
    let images: Vec<&str> = tags(package, "item")
        .filter(|t| attr(t, "media-type").is_some_and(|m| m.starts_with("image/")))
        .collect();
    let marked = images.iter().find(|t| {
        attr(t, "properties").is_some_and(|p| p.split_whitespace().any(|p| p == "cover-image"))
    });
    let named = || {
        let id = tags(package, "meta")
            .find(|t| attr(t, "name").as_deref() == Some("cover"))
            .and_then(|t| attr(t, "content"))?;
        images.iter().find(|t| attr(t, "id") == Some(id.clone()))
    };
    let guessed = || {
        images.iter().find(|t| {
            let id = attr(t, "id").unwrap_or_default();
            let href = attr(t, "href").unwrap_or_default();
            id.to_ascii_lowercase().contains("cover") || href.to_ascii_lowercase().contains("cover")
        })
    };
    attr(marked.or_else(named).or_else(guessed)?, "href")
}

/// The inside of each `<name ...>` tag, with any namespace prefix (`opf:item`)
/// ignored. Good enough for package files, which are small and machine-written.
fn tags<'a>(xml: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    xml.split('<').skip(1).filter_map(move |t| {
        let t = t.split('>').next()?;
        let tag = t.split(|c: char| c.is_whitespace() || c == '/').next()?;
        (tag.rsplit(':').next()? == name).then_some(t)
    })
}

/// An attribute's value, unescaped. `id` does not match `xml:id`.
fn attr(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(i) = rest.find(name) {
        let starts_word = rest[..i].ends_with(char::is_whitespace);
        rest = &rest[i + name.len()..];
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        if !starts_word {
            continue;
        }
        let value = value.trim_start();
        let quote = value.chars().next().filter(|&c| c == '"' || c == '\'')?;
        let value = &value[1..];
        return Some(unescape(&value[..value.find(quote)?]));
    }
    None
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The zip entry an href points at: `../Images/my%20cover.jpg` from `OEBPS/Text`
/// is `OEBPS/Images/my cover.jpg`.
fn resolve(dir: &str, href: &str) -> String {
    let href = percent_decode(href.split('#').next().unwrap_or(href));
    let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for segment in href.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(b) if bytes[i] == b'%' => {
                out.push(b);
                i += 3;
            }
            _ => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::{SimpleFileOptions, ZipWriter};

    fn book(files: &[(&str, &[u8])]) -> Cursor<Vec<u8>> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in files {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        let mut out = zip.finish().unwrap();
        out.set_position(0);
        out
    }

    const CONTAINER: &[u8] = br#"<?xml version="1.0"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

    #[test]
    fn finds_an_epub2_cover_through_its_meta() {
        let package = br#"<opf:package xmlns:opf="http://www.idpf.org/2007/opf">
  <opf:metadata><opf:meta name="cover" content="art"/></opf:metadata>
  <opf:manifest>
    <opf:item id="other" href="Images/map.png" media-type="image/png"/>
    <opf:item id="art" href="Images/front%20art.jpg" media-type="image/jpeg"/>
  </opf:manifest>
</opf:package>"#;
        let epub = book(&[
            ("META-INF/container.xml", CONTAINER),
            ("OEBPS/content.opf", package),
            ("OEBPS/Images/map.png", b"map"),
            ("OEBPS/Images/front art.jpg", b"cover"),
        ]);
        assert_eq!(cover_from(epub).as_deref(), Some(&b"cover"[..]));
    }

    #[test]
    fn prefers_the_epub3_cover_image_property() {
        let package = br#"<package version="3.0"><manifest>
  <item id="cover" href="xhtml/cover.xhtml" media-type="application/xhtml+xml"/>
  <item id="i1" href="../art/front.png" properties="svg cover-image" media-type="image/png"/>
</manifest></package>"#;
        let epub = book(&[
            ("META-INF/container.xml", CONTAINER),
            ("OEBPS/content.opf", package),
            ("art/front.png", b"front"),
        ]);
        assert_eq!(cover_from(epub).as_deref(), Some(&b"front"[..]));
    }

    #[test]
    fn has_no_cover_when_the_book_names_none() {
        let package = br#"<package><manifest>
  <item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
  <item id="fig" href="fig1.png" media-type="image/png"/>
</manifest></package>"#;
        let epub = book(&[
            ("META-INF/container.xml", CONTAINER),
            ("OEBPS/content.opf", package),
        ]);
        assert_eq!(cover_from(epub), None);
    }
}
