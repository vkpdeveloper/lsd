//! List view: Finder's columns (Name, Date Modified, Size, Kind) with a small
//! picture in front of every name.

use std::io::{self, Write};

use super::{Ctx, Painter, Pic, end_line, linked};
use crate::entry::Entry;
use crate::thumb::{Mode, Thumb};
use crate::{fmt, glyph, meta};

const DATE_W: usize = 21;
const SIZE_W: usize = 10;
const GAP: usize = 2;

pub fn render(ctx: &Ctx, entries: &[Entry], out: &mut impl Write) -> io::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let images = ctx.term.protocol.has_images();
    let icon_w = if images {
        3
    } else {
        fmt::width(glyph::icon(&entries[0], ctx.icons))
    };

    let kinds: Vec<String> = entries.iter().map(meta::kind).collect();
    let kind_w = kinds
        .iter()
        .map(|k| fmt::width(k))
        .max()
        .unwrap_or(4)
        .max(4);
    let name_w = entries
        .iter()
        .map(|e| fmt::width(&e.name))
        .max()
        .unwrap_or(4)
        .max(4);
    let total = ctx.term.cols as usize;

    // Drop columns from the right until the names get at least 24 cells.
    let mut cols = (true, true, true);
    let fixed = |c: (bool, bool, bool)| {
        icon_w
            + usize::from(c.0) * (GAP + DATE_W)
            + usize::from(c.1) * (GAP + SIZE_W)
            + usize::from(c.2) * (GAP + kind_w)
    };
    for drop in [
        (true, true, false),
        (false, true, false),
        (false, false, false),
    ] {
        if total.saturating_sub(fixed(cols)) >= name_w.min(24) {
            break;
        }
        cols = drop;
    }
    let name_w = name_w.min(total.saturating_sub(fixed(cols)).max(8));
    let (show_date, show_size, show_kind) = cols;

    let mut head = String::from("\x1b[2m");
    head.push_str(&" ".repeat(icon_w));
    push_cell(&mut head, "Name", name_w, false);
    if show_date {
        head.push_str(&" ".repeat(GAP));
        push_cell(&mut head, "Date Modified", DATE_W, false);
    }
    if show_size {
        head.push_str(&" ".repeat(GAP));
        push_cell(&mut head, "Size", SIZE_W, true);
    }
    if show_kind {
        head.push_str(&" ".repeat(GAP));
        head.push_str("Kind");
    }
    head.push_str("\x1b[0m");
    end_line(&mut head);
    out.write_all(head.as_bytes())?;

    let rx = images.then(|| ctx.thumbs.batch(entries, ctx.px_for((2, 1)), Mode::Icon));
    let mut thumbs: Vec<Option<Option<Thumb>>> = vec![None; entries.len()];
    let mut painter = Painter::new(ctx.term);

    for (i, entry) in entries.iter().enumerate() {
        let mut line = String::new();
        if let Some(rx) = &rx {
            while thumbs[i].is_none() {
                match rx.recv() {
                    Ok((j, t)) => thumbs[j] = Some(t),
                    Err(_) => break,
                }
            }
            let pic: Option<Pic> = thumbs[i]
                .as_ref()
                .and_then(Option::as_ref)
                .and_then(|t| painter.prepare(t, (2, 1)));
            match pic {
                Some(pic) => {
                    let mut cells = String::new();
                    painter.push_row(&mut cells, &pic, 0, 0, 0);
                    linked(&mut line, entry, &cells);
                    line.push_str(&" ".repeat(icon_w - pic.cols as usize));
                }
                None => line.push_str(&" ".repeat(icon_w)),
            }
        } else {
            line.push_str(glyph::icon(entry, ctx.icons));
        }

        let name = fmt::ellipsize_middle(&entry.name, name_w);
        let style = if images { "" } else { glyph::name_style(entry) };
        let mut styled = String::from(style);
        styled.push_str(&name);
        if !style.is_empty() {
            styled.push_str("\x1b[0m");
        }
        linked(&mut line, entry, &styled);
        line.push_str(&" ".repeat(name_w - fmt::width(&name)));

        line.push_str("\x1b[2m");
        if show_date {
            line.push_str(&" ".repeat(GAP));
            let date = entry.modified.map(fmt::date).unwrap_or_else(|| "--".into());
            push_cell(&mut line, &date, DATE_W, false);
        }
        if show_size {
            line.push_str(&" ".repeat(GAP));
            let size = if entry.is_dir {
                "--".to_string()
            } else {
                fmt::size(entry.size)
            };
            push_cell(&mut line, &size, SIZE_W, true);
        }
        if show_kind {
            line.push_str(&" ".repeat(GAP));
            line.push_str(&kinds[i]);
        }
        line.push_str("\x1b[0m");
        end_line(&mut line);
        painter.flush(out, &line, 1)?;
    }
    Ok(())
}

/// Pads (or right-aligns) `s` into a `w` wide column.
fn push_cell(line: &mut String, s: &str, w: usize, right: bool) {
    let s = fmt::ellipsize_end(s, w);
    let pad = " ".repeat(w - fmt::width(&s));
    if right {
        line.push_str(&pad);
        line.push_str(&s);
    } else {
        line.push_str(&s);
        line.push_str(&pad);
    }
}
