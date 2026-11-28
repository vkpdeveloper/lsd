//! Markdown for the browser's previews: headings, lists, quotes, code and
//! tables laid out as styled lines, so a glance shows the document rather than
//! its source. Opening the file still hands it to Leaf for real reading.
//!
//! Colors come from the terminal's palette, like code. Raw HTML is left out,
//! and images show as their alt text.

use std::rc::Rc;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};

use super::{Line, Span, Wrap, code, line_width};
use crate::fmt;

const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
const ACCENT: Color = Color::Indexed(4);
const CODE: Color = Color::Indexed(6);

/// The first `rows` lines of `src` laid out `width` wide. Parsing stops once
/// the screen is full.
pub fn render(src: &str, width: usize, rows: usize) -> Vec<Line> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut md = Md {
        width,
        ..Md::default()
    };
    for event in Parser::new_ext(src, options) {
        md.event(event);
        if md.lines.len() >= rows {
            break;
        }
    }
    md.flush();
    md.lines.truncate(rows);
    md.lines
}

#[derive(Default)]
struct Md {
    width: usize,
    lines: Vec<Line>,
    /// What starts every line inside the open quotes and list items.
    indent: Vec<Span>,
    /// The next number of each open list, None for bullets.
    lists: Vec<Option<u64>>,
    /// A list item's bullet, for the first line of its text.
    bullet: Option<Line>,
    /// The text block being laid out.
    wrap: Option<Wrap>,
    /// Inline styles, innermost last.
    styles: Vec<Style>,
    link: Option<Rc<str>>,
    /// A blank line goes before the next block.
    gap: bool,
    /// Text of a code block, front matter or table, collected until it ends.
    code: Option<(String, String)>,
    meta: Option<String>,
    table: Option<Table>,
}

struct Table {
    align: Vec<Alignment>,
    rows: Vec<Vec<String>>,
}

impl Md {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text, self.style()),
            Event::Code(text) => self.text(&text, self.style().fg(CODE)),
            Event::SoftBreak => self.text(" ", self.style()),
            Event::HardBreak => {
                if let Some(wrap) = &mut self.wrap {
                    wrap.newline();
                }
            }
            Event::Rule => {
                self.block();
                let room = self.width.saturating_sub(line_width(&self.indent));
                let mut line = self.indent.clone();
                line.push(Span::new("─".repeat(room), DIM));
                self.lines.push(line);
                self.gap = true;
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "☑ " } else { "☐ " };
                match self.bullet.as_mut().and_then(|b| b.last_mut()) {
                    Some(bullet) => bullet.text = mark.into(),
                    None => self.text(mark, self.style()),
                }
            }
            // Raw HTML would only show as tags, and math and footnotes are off.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.block(),
            Tag::Heading { level, .. } => {
                self.block();
                self.styles.push(match level {
                    HeadingLevel::H1 | HeadingLevel::H2 => BOLD.fg(ACCENT),
                    _ => BOLD,
                });
            }
            Tag::BlockQuote(_) => {
                self.block();
                self.indent.push(Span::new("│ ", DIM));
                self.styles
                    .push(Style::new().add_modifier(Modifier::ITALIC));
            }
            Tag::CodeBlock(kind) => {
                self.block();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split([' ', ',']).next().unwrap_or("").into()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(first) => {
                self.flush();
                if self.lists.is_empty() {
                    self.block();
                }
                self.lists.push(first);
            }
            Tag::Item => {
                self.flush();
                if self.gap {
                    self.block();
                }
                let depth = self.lists.len();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => match depth {
                        1 => "• ",
                        2 => "◦ ",
                        _ => "▪ ",
                    }
                    .into(),
                };
                let mut bullet = self.indent.clone();
                bullet.push(Span::new(marker.clone(), DIM));
                self.bullet = Some(bullet);
                self.indent
                    .push(Span::new(" ".repeat(fmt::width(&marker)), Style::new()));
            }
            Tag::Table(align) => {
                self.block();
                self.table = Some(Table {
                    align,
                    rows: Vec::new(),
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.rows.push(Vec::new());
                }
            }
            Tag::TableCell => {
                if let Some(row) = self.table.as_mut().and_then(|t| t.rows.last_mut()) {
                    row.push(String::new());
                }
            }
            Tag::Emphasis => self
                .styles
                .push(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.styles.push(BOLD),
            Tag::Strikethrough => self
                .styles
                .push(Style::new().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.link = Some(Rc::from(dest_url.as_ref()));
                self.styles
                    .push(Style::new().fg(ACCENT).add_modifier(Modifier::UNDERLINED));
            }
            Tag::Image { .. } => {
                self.styles.push(DIM);
                self.text("[", DIM);
            }
            Tag::MetadataBlock(_) => {
                self.block();
                self.meta = Some(String::new());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                self.gap = true;
            }
            TagEnd::Heading(_) => {
                self.styles.pop();
                self.flush();
                self.gap = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.indent.pop();
                self.styles.pop();
                self.gap = true;
            }
            TagEnd::CodeBlock => {
                if let Some((lang, text)) = self.code.take() {
                    self.code_block(&lang, &text);
                }
                self.gap = true;
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                // Inside an item the next item follows right on.
                self.gap = self.lists.is_empty();
            }
            TagEnd::Item => {
                self.flush();
                self.indent.pop();
                self.bullet = None;
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.table(table);
                }
                self.gap = true;
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                self.link = None;
            }
            TagEnd::Image => {
                self.styles.pop();
                self.text("]", DIM);
            }
            TagEnd::MetadataBlock(_) => {
                if let Some(meta) = self.meta.take() {
                    for raw in meta.lines() {
                        let mut wrap =
                            Wrap::new(self.width, self.indent.clone(), self.indent.clone());
                        wrap.text(raw, DIM, None);
                        self.lines.extend(wrap.finish());
                    }
                }
                self.gap = true;
            }
            _ => {}
        }
    }

    fn style(&self) -> Style {
        self.styles
            .iter()
            .fold(Style::new(), |style, inner| style.patch(*inner))
    }

    fn text(&mut self, text: &str, style: Style) {
        if let Some((_, code)) = &mut self.code {
            code.push_str(text);
        } else if let Some(meta) = &mut self.meta {
            meta.push_str(text);
        } else if let Some(table) = &mut self.table {
            if let Some(cell) = table.rows.last_mut().and_then(|r| r.last_mut()) {
                cell.push_str(text);
            }
        } else {
            let link = self.link.clone();
            self.wrap().text(text, style, link.as_ref());
        }
    }

    /// The text block being laid out, started on first use.
    fn wrap(&mut self) -> &mut Wrap {
        let (width, indent) = (self.width, &self.indent);
        let first = self.bullet.take();
        self.wrap.get_or_insert_with(|| {
            Wrap::new(
                width,
                first.unwrap_or_else(|| indent.clone()),
                indent.clone(),
            )
        })
    }

    /// Ends the text block being laid out.
    fn flush(&mut self) {
        if let Some(wrap) = self.wrap.take() {
            self.lines.extend(wrap.finish());
        }
    }

    /// Before a block: ends the one before it, with a blank line if it asked.
    fn block(&mut self) {
        self.flush();
        if self.gap && !self.lines.is_empty() {
            let blank = self.indent.iter().filter(|s| s.text.trim() != "").cloned();
            self.lines.push(blank.collect());
        }
        self.gap = false;
    }

    /// Indented and colored for its language, when there is one.
    fn code_block(&mut self, lang: &str, text: &str) {
        let mut pad = self.indent.clone();
        pad.push(Span::new("    ", Style::new()));
        let syntax = code::syntax_for_token(lang);
        for line in code::highlight(text, syntax) {
            let mut wrap = Wrap::new(self.width, pad.clone(), pad.clone());
            for span in &line {
                wrap.word(&span.text, span.style, None);
            }
            let wrapped = wrap.finish();
            if wrapped.is_empty() {
                self.lines.push(self.indent.clone());
            }
            self.lines.extend(wrapped);
        }
    }

    /// Columns as wide as their cells, narrowed to fit, with a rule under the
    /// header row.
    fn table(&mut self, table: Table) {
        let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let mut widths = vec![0; cols];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(fmt::width(cell.trim()));
            }
        }
        let room = self
            .width
            .saturating_sub(line_width(&self.indent) + 2 * (cols - 1));
        // Narrow the widest columns until the table fits.
        while widths.iter().sum::<usize>() > room {
            let Some(widest) = widths.iter_mut().max().filter(|w| **w > 3) else {
                break;
            };
            *widest -= 1;
        }
        for (r, row) in table.rows.iter().enumerate() {
            let mut line = self.indent.clone();
            for (i, w) in widths.iter().enumerate() {
                let cell = fmt::ellipsize_end(row.get(i).map_or("", |c| c.trim()), *w);
                let pad = w.saturating_sub(fmt::width(&cell));
                let (left, right) = match table.align.get(i) {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                let gap = if i + 1 < cols { 2 } else { 0 };
                let text = format!("{}{cell}{}", " ".repeat(left), " ".repeat(right + gap));
                line.push(Span::new(text, if r == 0 { BOLD } else { Style::new() }));
            }
            self.lines.push(line);
            if r == 0 {
                let mut rule = self.indent.clone();
                let total = widths.iter().sum::<usize>() + 2 * (cols - 1);
                rule.push(Span::new("─".repeat(total), DIM));
                self.lines.push(rule);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(src: &str, width: usize) -> Vec<String> {
        render(src, width, 100)
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
            .map(|s| s.trim_end().to_string())
            .collect()
    }

    #[test]
    fn lays_out_blocks_without_their_markup() {
        let src = "---\ntitle: Notes\n---\n\n# Plan\n\nSome **bold** text that wraps here.\n\n\
                   - one\n- two\n  - deeper\n\n1. first\n2. second\n\n> quoted\n\n```rust\nfn main() {}\n```\n\n---\n";
        assert_eq!(
            text(src, 20),
            [
                "title: Notes",
                "",
                "Plan",
                "",
                "Some bold text that",
                "wraps here.",
                "",
                "• one",
                "• two",
                "  ◦ deeper",
                "",
                "1. first",
                "2. second",
                "",
                "│ quoted",
                "",
                "    fn main() {}",
                "",
                "────────────────────",
            ]
        );
    }

    #[test]
    fn styles_inline_text() {
        let lines = render("A *b* `c` [d](https://e.com)", 40, 5);
        let find = |t: &str| lines[0].iter().find(|s| s.text == t).unwrap().clone();
        assert!(find("b").style.add_modifier.contains(Modifier::ITALIC));
        assert_eq!(find("c").style.fg, Some(CODE));
        assert_eq!(find("d").link.as_deref(), Some("https://e.com"));
    }

    #[test]
    fn fits_tables_and_task_lists() {
        let src = "| Key | What it does |\n| --- | ---: |\n| `gd` | Focus the diff |\n\n- [x] done\n- [ ] todo\n";
        assert_eq!(
            text(src, 24),
            [
                // The second column is right-aligned.
                "Key    What it does",
                "───────────────────",
                "gd   Focus the diff",
                "",
                "☑ done",
                "☐ todo",
            ]
        );
        // Too wide: the widest column gives way.
        let narrow = text("| a | bbbbbbbbbbbbbbbbbbbb |\n|---|---|\n| 1 | 2 |\n", 12);
        assert_eq!(narrow[0], "a  bbbbbbbb…");
    }

    #[test]
    fn stops_at_a_screenful() {
        let src = "para\n\n".repeat(10_000);
        assert_eq!(render(&src, 40, 5).len(), 5);
    }
}
