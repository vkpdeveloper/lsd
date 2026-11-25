//! Tree view: folders expanded recursively, a small picture on every line.

use std::io::{self, Write};

use super::{Ctx, Painter, end_line, linked};
use crate::entry::{self, Entry, Sort};
use crate::thumb::{Mode, Thumb};
use crate::{fmt, glyph};

pub struct TreeOpts {
    pub show_hidden: bool,
    pub sort: Sort,
    pub depth: Option<usize>,
}

/// One printed line: the guides in front of it and the entry.
struct Line {
    prefix: String,
    entry: Entry,
}

pub fn render(ctx: &Ctx, roots: &[Entry], opts: &TreeOpts, out: &mut impl Write) -> io::Result<()> {
    let mut lines = Vec::new();
    for root in roots {
        lines.push(Line {
            prefix: String::new(),
            entry: root.clone(),
        });
        if root.is_dir {
            walk(&root.path, "", 1, opts, &mut lines);
        }
    }
    let entries: Vec<Entry> = lines.iter().map(|l| l.entry.clone()).collect();
    let images = ctx.term.protocol.has_images();
    let rx = images.then(|| ctx.thumbs.batch(&entries, ctx.px_for((2, 1)), Mode::Icon));
    let mut thumbs: Vec<Option<Option<Thumb>>> = vec![None; entries.len()];
    let mut painter = Painter::new(ctx.term);

    for (i, line) in lines.iter().enumerate() {
        let mut text = String::from("\x1b[2m");
        text.push_str(&line.prefix);
        text.push_str("\x1b[0m");
        let x = fmt::width(&line.prefix) as u16;
        if let Some(rx) = &rx {
            while thumbs[i].is_none() {
                match rx.recv() {
                    Ok((j, t)) => thumbs[j] = Some(t),
                    Err(_) => break,
                }
            }
            let pic = thumbs[i]
                .as_ref()
                .and_then(Option::as_ref)
                .and_then(|t| painter.prepare(t, (2, 1)));
            match pic {
                Some(pic) => {
                    let mut cells = String::new();
                    painter.push_row(&mut cells, &pic, 0, x, 0);
                    linked(&mut text, &line.entry, &cells);
                    text.push_str(&" ".repeat(3 - pic.cols as usize));
                }
                None => text.push_str("   "),
            }
        } else {
            text.push_str(glyph::icon(&line.entry, ctx.icons));
        }
        let style = if images {
            ""
        } else {
            glyph::name_style(&line.entry)
        };
        let name = format!(
            "{style}{}{}",
            line.entry.name,
            if style.is_empty() { "" } else { "\x1b[0m" }
        );
        linked(&mut text, &line.entry, &name);
        end_line(&mut text);
        painter.flush(out, &text, 1)?;
    }
    Ok(())
}

fn walk(dir: &std::path::Path, prefix: &str, depth: usize, opts: &TreeOpts, lines: &mut Vec<Line>) {
    if opts.depth.is_some_and(|max| depth > max) {
        return;
    }
    let Ok(mut children) = entry::read_dir(dir, opts.show_hidden) else {
        return;
    };
    entry::sort(&mut children, opts.sort);
    let count = children.len();
    for (i, child) in children.into_iter().enumerate() {
        let last = i + 1 == count;
        let branch = if last { "└── " } else { "├── " };
        let next = format!("{prefix}{}", if last { "    " } else { "│   " });
        // Do not follow symlinked folders: they can loop.
        let descend = child.is_dir && !child.is_symlink;
        let path = child.path.clone();
        lines.push(Line {
            prefix: format!("{prefix}{branch}"),
            entry: child,
        });
        if descend {
            walk(&path, &next, depth + 1, opts, lines);
        }
    }
}
