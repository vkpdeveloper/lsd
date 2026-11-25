//! Source code with syntax colors from the terminal's palette.

use std::path::Path;
use std::sync::LazyLock;

use ratatui::style::{Color, Modifier, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    self as hl, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use super::{Line, Span, Wrap};

/// bat's syntax set: TypeScript, TOML, Dockerfile and friends included.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
static THEME: LazyLock<Theme> = LazyLock::new(palette_theme);

/// Past this many lines the rest is shown uncolored; highlighting is the slow
/// part of showing a big file.
const MAX_HIGHLIGHT_LINES: usize = 5000;
/// A line longer than this ends the coloring too. On one, as in minified code,
/// syntect can take minutes, and it would not fit on a screen anyway.
const MAX_HIGHLIGHT_LINE: usize = 4096;

/// syntect colors carry no palette, so the theme encodes one in `a`:
/// 0 means palette entry `r`, `DIM_ALPHA` means dimmed default text, anything
/// else the default color.
const DIM_ALPHA: u8 = 2;
/// Stands for "dimmed" in the rules table, outside the palette's 0..=15.
const DIMMED: u8 = 255;

/// The grammar for a file: by name (Makefile), extension, then first line.
pub fn syntax_for_path(path: &Path, first_line: &str) -> &'static SyntaxReference {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    SYNTAXES
        .find_syntax_by_extension(name)
        .or_else(|| SYNTAXES.find_syntax_by_extension(ext))
        .or_else(|| SYNTAXES.find_syntax_by_extension(&ext.to_ascii_lowercase()))
        .or_else(|| SYNTAXES.find_syntax_by_first_line(first_line))
        .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text())
}

/// Colors `src` line by line, as the lines are taken. Tabs become four spaces.
pub fn highlight<'a>(src: &'a str, syntax: &SyntaxReference) -> impl Iterator<Item = Line> + 'a {
    let mut h = HighlightLines::new(syntax, &THEME);
    let mut plain = false;
    LinesWithEndings::from(src)
        .enumerate()
        .map(move |(n, raw)| {
            let mut line = Line::new();
            if n >= MAX_HIGHLIGHT_LINES || raw.len() > MAX_HIGHLIGHT_LINE {
                plain = true;
            }
            let ranges = if plain {
                None
            } else {
                h.highlight_line(raw, &SYNTAXES).ok()
            };
            match ranges {
                Some(ranges) => {
                    for (style, text) in ranges {
                        push(&mut line, text, convert(style));
                    }
                }
                None => push(&mut line, raw, Style::new()),
            }
            line
        })
}

fn push(line: &mut Line, text: &str, style: Style) {
    let text = text.trim_end_matches(['\n', '\r']).replace('\t', "    ");
    if text.is_empty() {
        return;
    }
    match line.last_mut() {
        Some(last) if last.style == style => last.text.push_str(&text),
        _ => line.push(Span::new(text, style)),
    }
}

/// A file with line numbers, wrapped to `width`. Lines come out as they are
/// colored, so the first screen can show before a long file is done.
pub fn render<'a>(src: &'a str, path: &Path, width: usize) -> Box<dyn Iterator<Item = Line> + 'a> {
    let first = src.lines().next().unwrap_or("");
    let syntax = syntax_for_path(path, first);
    if syntax.name == "Plain Text" {
        return Box::new(src.lines().flat_map(move |raw| prose(raw, width)));
    }
    let digits = src.lines().count().max(1).to_string().len();
    let gutter = Style::new().add_modifier(Modifier::DIM);
    Box::new(
        highlight(src, syntax)
            .enumerate()
            .flat_map(move |(i, line)| {
                let number = vec![Span::new(format!("{:>digits$}  ", i + 1), gutter)];
                let blank = vec![Span::new(" ".repeat(digits + 2), Style::new())];
                let mut wrap = Wrap::new(width, number.clone(), blank);
                for span in &line {
                    wrap.word(&span.text, span.style, None);
                }
                let wrapped = wrap.finish();
                if wrapped.is_empty() {
                    vec![number]
                } else {
                    wrapped
                }
            }),
    )
}

/// One line of plain text, wrapped at spaces like a note, without a number.
fn prose(raw: &str, width: usize) -> Vec<Line> {
    let raw = raw.replace('\t', "    ");
    let body = raw.trim_start();
    let pad = vec![Span::new(" ".repeat(raw.len() - body.len()), Style::new())];
    let mut wrap = Wrap::new(width, pad.clone(), pad);
    wrap.text(body, Style::new(), None);
    let lines = wrap.finish();
    if lines.is_empty() {
        vec![Line::new()]
    } else {
        lines
    }
}

fn convert(style: hl::Style) -> Style {
    let mut out = Style::new();
    let fg = style.foreground;
    if fg.a == 0 {
        out = out.fg(Color::Indexed(fg.r));
    } else if fg.a == DIM_ALPHA {
        out = out.add_modifier(Modifier::DIM);
    }
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}

/// Maps scopes to the eight base palette colors, like bat's `ansi` theme, so
/// code looks right in every terminal theme.
fn palette_theme() -> Theme {
    const RED: u8 = 1;
    const GREEN: u8 = 2;
    const YELLOW: u8 = 3;
    const BLUE: u8 = 4;
    const MAGENTA: u8 = 5;
    const CYAN: u8 = 6;
    let default = hl::Color {
        r: 0,
        g: 0,
        b: 0,
        a: 1,
    };
    let rules: &[(&str, Option<u8>, FontStyle)] = &[
        (
            "comment, punctuation.definition.comment",
            Some(DIMMED),
            FontStyle::ITALIC,
        ),
        (
            "string, punctuation.definition.string",
            Some(GREEN),
            FontStyle::empty(),
        ),
        (
            "constant.character.escape, constant.other.placeholder",
            Some(CYAN),
            FontStyle::empty(),
        ),
        (
            "constant.numeric, constant.language, constant.character",
            Some(YELLOW),
            FontStyle::empty(),
        ),
        (
            "keyword, storage, keyword.operator.word",
            Some(MAGENTA),
            FontStyle::empty(),
        ),
        (
            "keyword.operator, punctuation.separator",
            None,
            FontStyle::empty(),
        ),
        (
            "entity.name.function, support.function, variable.function",
            Some(BLUE),
            FontStyle::empty(),
        ),
        (
            "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.trait, \
             entity.other.inherited-class, support.type, support.class",
            Some(CYAN),
            FontStyle::empty(),
        ),
        (
            "variable.language, support.constant",
            Some(RED),
            FontStyle::empty(),
        ),
        (
            "entity.name.tag, meta.mapping.key string, support.type.property-name",
            Some(BLUE),
            FontStyle::empty(),
        ),
        (
            "entity.other.attribute-name",
            Some(YELLOW),
            FontStyle::empty(),
        ),
        (
            "entity.name.section, markup.heading",
            Some(BLUE),
            FontStyle::BOLD,
        ),
        ("markup.bold", None, FontStyle::BOLD),
        ("markup.italic", None, FontStyle::ITALIC),
        (
            "markup.raw, markup.inline.raw",
            Some(YELLOW),
            FontStyle::empty(),
        ),
        (
            "markup.underline.link, meta.link",
            Some(BLUE),
            FontStyle::UNDERLINE,
        ),
        ("markup.inserted", Some(GREEN), FontStyle::empty()),
        ("markup.deleted, invalid", Some(RED), FontStyle::empty()),
        (
            "meta.diff.range, meta.diff.header",
            Some(CYAN),
            FontStyle::empty(),
        ),
    ];
    let scopes = rules
        .iter()
        .filter_map(|&(scope, color, font_style)| {
            let foreground = color.map(|c| match c {
                DIMMED => hl::Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: DIM_ALPHA,
                },
                c => hl::Color {
                    r: c,
                    g: 0,
                    b: 0,
                    a: 0,
                },
            });
            Some(ThemeItem {
                scope: scope.parse::<ScopeSelectors>().ok()?,
                style: StyleModifier {
                    foreground: foreground.or(Some(default)),
                    background: None,
                    font_style: Some(font_style),
                },
            })
        })
        .collect();
    Theme {
        settings: ThemeSettings {
            foreground: Some(default),
            ..ThemeSettings::default()
        },
        scopes,
        ..Theme::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_rust_from_the_palette() {
        let src = "fn main() { let s = \"hi\"; } // done\n";
        let lines: Vec<Line> = highlight(src, syntax_for_path(Path::new("a.rs"), "")).collect();
        let find = |t: &str| {
            lines[0]
                .iter()
                .find(|s| s.text.contains(t))
                .map(|s| s.style)
                .unwrap()
        };
        assert_eq!(find("fn").fg, Some(Color::Indexed(5)));
        assert_eq!(find("\"hi").fg, Some(Color::Indexed(2)));
        assert!(find("done").add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn finds_grammars_by_name_extension_and_first_line() {
        assert_eq!(syntax_for_path(Path::new("Makefile"), "").name, "Makefile");
        assert_eq!(syntax_for_path(Path::new("app.ts"), "").name, "TypeScript");
        assert_eq!(
            syntax_for_path(Path::new("x"), "#!/bin/bash").name,
            "Bourne Again Shell (bash)"
        );
    }
}
