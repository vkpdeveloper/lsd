//! Icon view: a Finder-style grid of thumbnails with two-line labels.
//!
//! Thumbnails are generated in parallel and each grid row is printed as soon as
//! all of its tiles are ready, so the first row shows up without waiting for the
//! whole directory.

use std::io::{self, Write};

use super::{Ctx, Placed, Placer, linked, write_all};
use crate::entry::Entry;
use crate::fmt;
use crate::thumb::{Mode, Thumb};

pub fn render(ctx: &Ctx, entries: &[Entry], out: &mut impl Write) -> io::Result<()> {
    let Some(g) = ctx.term.graphics else {
        return super::plain::columns(ctx.term, entries, out);
    };
    if entries.is_empty() {
        return Ok(());
    }
    let box_cols = ctx.size.box_cols();
    let box_rows = ctx.square_rows(box_cols);
    let tile = box_cols as usize + 8;
    let label_w = tile - 2;
    let per_row = (ctx.term.cols as usize / tile).max(1);

    let px = ctx.px_for((box_cols, box_rows));
    let rx = ctx.thumbs.batch(entries, px, Mode::Icon);
    let mut thumbs: Vec<Option<Option<Thumb>>> = vec![None; entries.len()];
    let mut placer = Placer::new(g, ctx.term.cell);

    write_all(out, b"\n")?;
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

        let mut buf = Vec::new();
        let placed: Vec<Option<Placed>> = thumbs[range]
            .iter()
            .map(|t| {
                t.as_ref()
                    .and_then(Option::as_ref)
                    .and_then(|t| placer.place(&mut buf, t, (box_cols, box_rows)))
            })
            .collect();

        let mut text = String::new();
        for line in 0..box_rows {
            for (entry, p) in row.iter().zip(&placed) {
                let Some(p) = *p else {
                    text.push_str(&" ".repeat(tile));
                    continue;
                };
                // Center horizontally and vertically inside the box.
                let top = (box_rows - p.rows) / 2;
                let left = (tile - p.cols as usize) / 2;
                if line < top || line >= top + p.rows {
                    text.push_str(&" ".repeat(tile));
                    continue;
                }
                text.push_str(&" ".repeat(left));
                let mut cells = String::new();
                placer.push_row(&mut cells, p, line - top);
                linked(&mut text, entry, &cells);
                text.push_str(&" ".repeat(tile - left - p.cols as usize));
            }
            trim_line(&mut text);
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
                let left = (tile.saturating_sub(w)) / 2;
                text.push_str(&" ".repeat(left));
                linked(&mut text, entry, s);
                text.push_str(&" ".repeat(tile.saturating_sub(left + w)));
            }
            trim_line(&mut text);
        }
        if start + row.len() < entries.len() {
            text.push('\n');
        }
        buf.extend_from_slice(text.as_bytes());
        write_all(out, &buf)?;
    }
    Ok(())
}

/// Ends the current output line, dropping trailing padding.
fn trim_line(text: &mut String) {
    let trimmed = text.trim_end_matches(' ').len();
    text.truncate(trimmed);
    text.push('\n');
}
