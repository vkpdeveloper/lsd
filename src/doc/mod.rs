//! Quick Look for text: JSON and source code laid out as styled lines in the
//! terminal's own palette, so every theme works. Markdown, PDFs and books
//! open in Leaf instead (see `leaf`); the browser's previews show Markdown
//! laid out (see [`markdown`]) and tables as colored source.
//!
//! Lines use ratatui styles so the browser can show them as they are, and the
//! inline views turn them into escapes with [`push_ansi`].

pub mod code;
pub mod env;
pub mod json;
pub mod markdown;

use std::path::Path;
use std::rc::Rc;

use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthChar;

use crate::entry::Entry;
use crate::glyph::{self, Class};
use crate::{fmt, halfblock};

/// Largest file shown in full. Quick Look stops early too.
pub const MAX_BYTES: usize = 1 << 20;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    pub link: Option<Rc<str>>,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Span {
        Span {
            text: text.into(),
            style,
            link: None,
        }
    }

    pub fn width(&self) -> usize {
        fmt::width(&self.text)
    }
}

pub type Line = Vec<Span>;

pub enum Kind {
    Json,
    Code,
}

/// Which renderer suits `entry`, if any, with the text and whether it was cut
/// short. Reads at most [`MAX_BYTES`] to tell text from binary. Documents Leaf
/// reads are left to it, and environment files show with their values hidden.
pub fn load(entry: &Entry) -> Option<(Kind, String, bool)> {
    if crate::leaf::reads(entry) {
        return None;
    }
    read(entry, MAX_BYTES, false)
}

/// How much of a file a preview reads. JSON cut short here can't be laid out
/// pretty, so it shows as colored source instead.
const PREVIEW_BYTES: usize = 256 << 10;

/// The first `rows` lines of a text file laid out `width` wide, for the
/// browser's previews. None when it is not text, or empty. Environment files
/// show their values only with `reveal`.
pub fn preview(entry: &Entry, width: usize, rows: usize, reveal: bool) -> Option<Vec<Line>> {
    if !entry.is_dir && matches!(entry.ext().as_str(), "md" | "markdown" | "mdown" | "mkd") {
        let (text, _) = read_text(&entry.path, PREVIEW_BYTES)?;
        let lines = markdown::render(&text, width, rows);
        return (!lines.is_empty()).then_some(lines);
    }
    let (kind, text, cut) = read(entry, PREVIEW_BYTES, reveal)?;
    let json = match kind {
        Kind::Json if !cut => json::render(&text, width),
        _ => None,
    };
    let lines = match json {
        Some(mut lines) => {
            lines.truncate(rows);
            lines
        }
        None => {
            // Nothing past a screenful of characters and line breaks can
            // show, and laying out one long line in full is the slow part.
            let end = text
                .char_indices()
                .nth(rows * (width + 1))
                .map_or(text.len(), |(i, _)| i);
            code::render(&text[..end], &entry.path, width)
                .take(rows)
                .collect()
        }
    };
    (!lines.is_empty()).then_some(lines)
}

fn read(entry: &Entry, max: usize, reveal: bool) -> Option<(Kind, String, bool)> {
    if entry.is_dir {
        return None;
    }
    let kind = match entry.ext().as_str() {
        "json" | "geojson" | "jsonc" | "json5" => Kind::Json,
        _ => match glyph::class(entry) {
            Class::Code | Class::Text | Class::Other => Kind::Code,
            _ => return None,
        },
    };
    let (text, cut) = read_text(&entry.path, max)?;
    if !reveal && env::is_env(&entry.name) {
        return Some((kind, env::hide(&text), cut));
    }
    Some((kind, text, cut))
}

/// The first `max` bytes of the file as UTF-8, if it looks like text. The
/// flag says it was cut short.
fn read_text(path: &Path, max: usize) -> Option<(String, bool)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?.take(max as u64 + 1);
    let mut buf = Vec::new();
    // Binary files show a NUL early, so look before reading the rest.
    (&mut file).take(8192).read_to_end(&mut buf).ok()?;
    if buf.contains(&0) {
        return None;
    }
    file.read_to_end(&mut buf).ok()?;
    let cut = buf.len() > max;
    buf.truncate(max);
    let text = match String::from_utf8(buf) {
        Ok(s) => s,
        // A cut can land inside a character; anything else is not text.
        Err(e) if cut && e.utf8_error().error_len().is_none() => {
            let valid = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).ok()?
        }
        Err(_) => return None,
    };
    Some((text, cut))
}

/// Lays out styled text into lines no wider than `width`, breaking at spaces.
/// Continuation lines start with `rest` instead of `first`.
pub struct Wrap {
    width: usize,
    rest: Line,
    lines: Vec<Line>,
    cur: Line,
    cur_w: usize,
    /// Only the prefix is on the current line so far.
    empty: bool,
    space: Option<Style>,
}

impl Wrap {
    pub fn new(width: usize, first: Line, rest: Line) -> Wrap {
        let cur_w = first.iter().map(Span::width).sum();
        Wrap {
            width: width.max(cur_w + 8),
            rest,
            lines: Vec::new(),
            cur: first,
            cur_w,
            empty: true,
            space: None,
        }
    }

    /// Adds text that may break at its spaces.
    pub fn text(&mut self, text: &str, style: Style, link: Option<&Rc<str>>) {
        for (i, word) in text.split([' ', '\n', '\t']).enumerate() {
            if i > 0 {
                self.space = Some(style);
            }
            if !word.is_empty() {
                self.word(word, style, link);
            }
        }
    }

    /// Adds text that never breaks at spaces, only when it must.
    pub fn word(&mut self, word: &str, style: Style, link: Option<&Rc<str>>) {
        let w = fmt::width(word);
        let space = usize::from(self.space.is_some() && !self.empty);
        // A word too long for any line starts right here, wasting no room.
        let fits_a_line = line_width(&self.rest) + w <= self.width;
        if !self.empty && fits_a_line && self.cur_w + space + w > self.width {
            self.newline();
        } else if let Some(style) = self.space.take().filter(|_| !self.empty) {
            self.push(" ", style, None);
        }
        self.space = None;
        if self.cur_w + w <= self.width {
            self.push(word, style, link);
            return;
        }
        // Longer than a whole line: split it by characters.
        let mut chunk = String::new();
        for c in word.chars() {
            let cw = c.width().unwrap_or(0);
            if self.cur_w + fmt::width(&chunk) + cw > self.width {
                self.push(&chunk, style, link);
                chunk.clear();
                self.newline();
            }
            chunk.push(c);
        }
        self.push(&chunk, style, link);
    }

    /// Adds spans that must stay together on one line, like `12,`.
    pub fn group(&mut self, spans: &[Span]) {
        let w = line_width(spans);
        let space = usize::from(self.space.is_some() && !self.empty);
        if !self.empty && self.cur_w + space + w > self.width {
            self.newline();
        }
        for span in spans {
            self.word(&span.text, span.style, span.link.as_ref());
        }
    }

    pub fn newline(&mut self) {
        let line = std::mem::replace(&mut self.cur, self.rest.clone());
        self.lines.push(line);
        self.cur_w = self.rest.iter().map(Span::width).sum();
        self.empty = true;
        self.space = None;
    }

    fn push(&mut self, text: &str, style: Style, link: Option<&Rc<str>>) {
        if text.is_empty() {
            return;
        }
        self.cur_w += fmt::width(text);
        self.empty = false;
        if let Some(last) = self.cur.last_mut()
            && last.style == style
            && last.link.as_ref() == link
        {
            last.text.push_str(text);
            return;
        }
        self.cur.push(Span {
            text: text.into(),
            style,
            link: link.cloned(),
        });
    }

    pub fn finish(mut self) -> Vec<Line> {
        if !self.empty {
            self.lines.push(self.cur);
        }
        self.lines
    }
}

pub fn line_width(line: &[Span]) -> usize {
    line.iter().map(Span::width).sum()
}

/// Appends a line as SGR and OSC 8 escapes, ending with everything reset.
pub fn push_ansi(s: &mut String, line: &[Span], truecolor: bool) {
    for span in line {
        if let Some(url) = &span.link {
            s.push_str(&fmt::link_start(url));
        }
        push_sgr(s, span.style, truecolor);
        s.push_str(&span.text);
        s.push_str("\x1b[0m");
        if span.link.is_some() {
            s.push_str(fmt::LINK_END);
        }
    }
}

fn push_sgr(s: &mut String, style: Style, truecolor: bool) {
    let mut codes: Vec<String> = Vec::new();
    let m = style.add_modifier;
    for (flag, code) in [
        (Modifier::BOLD, "1"),
        (Modifier::DIM, "2"),
        (Modifier::ITALIC, "3"),
        (Modifier::UNDERLINED, "4"),
        (Modifier::REVERSED, "7"),
        (Modifier::CROSSED_OUT, "9"),
    ] {
        if m.contains(flag) {
            codes.push(code.into());
        }
    }
    if let Some(c) = style.fg.and_then(|c| color(c, 30, truecolor)) {
        codes.push(c);
    }
    if let Some(c) = style.bg.and_then(|c| color(c, 40, truecolor)) {
        codes.push(c);
    }
    if !codes.is_empty() {
        s.push_str("\x1b[");
        s.push_str(&codes.join(";"));
        s.push('m');
    }
}

/// SGR parameters for `c`. `base` is 30 for foreground, 40 for background.
fn color(c: Color, base: u8, truecolor: bool) -> Option<String> {
    let indexed = |i: u8| match i {
        0..=7 => format!("{}", base + i),
        8..=15 => format!("{}", base + 60 + i - 8),
        _ => format!("{};5;{i}", base + 8),
    };
    Some(match c {
        Color::Reset => return None,
        Color::Black => indexed(0),
        Color::Red => indexed(1),
        Color::Green => indexed(2),
        Color::Yellow => indexed(3),
        Color::Blue => indexed(4),
        Color::Magenta => indexed(5),
        Color::Cyan => indexed(6),
        Color::Gray => indexed(7),
        Color::DarkGray => indexed(8),
        Color::LightRed => indexed(9),
        Color::LightGreen => indexed(10),
        Color::LightYellow => indexed(11),
        Color::LightBlue => indexed(12),
        Color::LightMagenta => indexed(13),
        Color::LightCyan => indexed(14),
        Color::White => indexed(15),
        Color::Indexed(i) => indexed(i),
        Color::Rgb(r, g, b) if truecolor => format!("{};2;{r};{g};{b}", base + 8),
        Color::Rgb(r, g, b) => indexed(halfblock::ansi256((r, g, b))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn wraps_at_spaces_with_hanging_indent() {
        let mut w = Wrap::new(
            12,
            vec![Span::new("- ", Style::new())],
            vec![Span::new("  ", Style::new())],
        );
        w.text("one two three four", Style::new(), None);
        assert_eq!(text(&w.finish()), ["- one two", "  three four"]);
    }

    #[test]
    fn splits_words_longer_than_a_line() {
        let mut w = Wrap::new(8, vec![], vec![]);
        w.text("abcdefghijkl", Style::new(), None);
        assert_eq!(text(&w.finish()), ["abcdefgh", "ijkl"]);

        // It starts on the line it is on instead of leaving a gap.
        let mut w = Wrap::new(8, vec![], vec![]);
        w.text("ab cdefghijkl", Style::new(), None);
        assert_eq!(text(&w.finish()), ["ab cdefg", "hijkl"]);
    }

    #[test]
    fn keeps_styles_apart() {
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let mut w = Wrap::new(40, vec![], vec![]);
        w.text("plain ", Style::new(), None);
        w.text("bold", bold, None);
        let lines = w.finish();
        assert_eq!(lines[0].len(), 2);
        assert_eq!(lines[0][1].text, "bold");
    }

    #[test]
    fn previews_the_start_of_text_files() {
        let tmp = crate::fsops::tests::TempDir::new();
        let preview = |name: &str, bytes: &[u8]| {
            let path = tmp.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            super::preview(&Entry::from_path(&path).unwrap(), 40, 3, false).map(|l| text(&l))
        };

        let pretty = preview("small.json", br#"{"a":[1,2],"b":{"c":"d","e":"f"}}"#).unwrap();
        assert_eq!(pretty[0].trim(), "{");
        assert_eq!(pretty.len(), 3);

        // Too long to parse for a preview, so it shows as written.
        let mut big = br#"{"k":"#.to_vec();
        big.extend(std::iter::repeat_n(b'1', PREVIEW_BYTES));
        big.push(b'}');
        let source = preview("big.json", &big).unwrap();
        assert!(source[0].starts_with(r#"1  {"k":111"#), "{source:?}");

        assert_eq!(preview("blob", b"\x7fELF\0\0\x01"), None);
        // Shows its icon instead of a blank page.
        assert_eq!(preview("empty.txt", b""), None);
    }
}
