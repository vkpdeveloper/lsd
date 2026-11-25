//! Icon view: a Finder-style grid of thumbnails with two-line labels.
//!
//! Thumbnails are generated in parallel and each grid row is printed as soon as
//! all of its tiles are ready, so the first row shows up without waiting for the
//! whole folder.

use std::io::{self, Write};

use super::{Ctx, Painter, Pic, end_line, linked};
use crate::entry::Entry;
use crate::fmt;
use crate::thumb::{Mode, Thumb};

pub fn render(ctx: &Ctx, entries: &[Entry], out: &mut impl Write) -> io::Result<()> {
    if !ctx.term.protocol.has_images() {
        // A grid of tiny half-block mosaics reads worse than clean columns.
        return super::plain::columns(ctx, entries, out);
    }
    if entries.is_empty() {
        return Ok(());
    }
    let box_cols = ctx.size.box_cols();
    let box_rows = ctx.square_rows(box_cols);
    let tile = box_cols as usize + 8;
    let label_w = tile - 2;
    let per_row = (ctx.term.cols as usize / tile).max(1);

    let rx = ctx
        .thumbs
        .batch(entries, ctx.px_for((box_cols, box_rows)), Mode::Icon);
    let mut thumbs: Vec<Option<Option<Thumb>>> = vec![None; entries.len()];
    let mut painter = Painter::new(ctx.term);

    out.write_all(b"\n")?;
    for (row_idx, row) in entries.chunks(per_row).enumerate() {
        let start = row_idx * per_row;
        let range = start..start + row.len();
        while thumbs[range.clone()].iter().any(Option::is_none) {
            match rx.recv() {
                Ok((i, t)) => thumbs[i] = Some(t),
                // Workers are gone; whatever is missing stays empty.
                Err(_) => break,
            }
        }
        let pics: Vec<Option<Pic>> = thumbs[range]
            .iter()
            .map(|t| {
                t.as_ref()
                    .and_then(Option::as_ref)
                    .and_then(|t| painter.prepare(t, (box_cols, box_rows)))
            })
            .collect();

        let mut text = String::new();
        for line in 0..box_rows {
            for (col, (entry, pic)) in row.iter().zip(&pics).enumerate() {
                let Some(pic) = pic else {
                    text.push_str(&" ".repeat(tile));
                    continue;
                };
                // Center inside the box, both ways.
                let top = (box_rows - pic.rows) / 2;
                let left = (tile - pic.cols as usize) / 2;
                if line < top || line >= top + pic.rows {
                    text.push_str(&" ".repeat(tile));
                    continue;
                }
                text.push_str(&" ".repeat(left));
                let mut cells = String::new();
                painter.push_row(
                    &mut cells,
                    pic,
                    line - top,
                    (col * tile + left) as u16,
                    line,
                );
                linked(&mut text, entry, &cells);
                text.push_str(&" ".repeat(tile - left - pic.cols as usize));
            }
            end_line(&mut text);
        }

        let labels: Vec<Vec<String>> = row
            .iter()
            .map(|e| fmt::label_lines(&e.name, label_w))
            .collect();
        let label_rows = labels.iter().map(Vec::len).max().unwrap_or(1);
        for line in 0..label_rows {
            for (entry, lines) in row.iter().zip(&labels) {
                let s = lines.get(line).map(String::as_str).unwrap_or("");
                let w = fmt::width(s);
                let left = tile.saturating_sub(w) / 2;
                text.push_str(&" ".repeat(left));
                linked(&mut text, entry, s);
                text.push_str(&" ".repeat(tile.saturating_sub(left + w)));
            }
            end_line(&mut text);
        }
        let mut height = box_rows + label_rows as u16;
        if start + row.len() < entries.len() {
            text.push('\n');
            height += 1;
        }
        painter.flush(out, &text, height)?;
    }
    Ok(())
}
