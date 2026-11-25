//! Text-only output: pipes, `-1`, and terminals without images.

use std::io::{self, Write};

use super::{Ctx, linked};
use crate::entry::Entry;
use crate::fmt;
use crate::glyph;

/// One name per line, exactly what scripts expect from `ls` in a pipe.
pub fn lines(entries: &[Entry], out: &mut impl Write) -> io::Result<()> {
    let mut buf = String::new();
    for e in entries {
        buf.push_str(&e.name);
        buf.push('\n');
    }
    out.write_all(buf.as_bytes())
}

/// `ls`-style columns filled top to bottom, with icons and the terminal's own
/// colors. Names are clickable.
pub fn columns(ctx: &Ctx, entries: &[Entry], out: &mut impl Write) -> io::Result<()> {
    if !ctx.term.tty {
        return lines(entries, out);
    }
    if entries.is_empty() {
        return Ok(());
    }
    let cells: Vec<(String, usize)> = entries
        .iter()
        .map(|e| {
            let s = format!("{}{}", glyph::icon(e, ctx.icons), e.name);
            let w = fmt::width(&s);
            (s, w)
        })
        .collect();
    let gap = 2;
    let total = ctx.term.cols as usize;
    // Most columns whose widths still fit, like ls.
    let widest = cells.iter().map(|c| c.1).max().unwrap_or(0);
    let mut layout = (1, vec![widest]);
    for ncols in 2..=cells.len() {
        let nrows = cells.len().div_ceil(ncols);
        let widths: Vec<usize> = cells
            .chunks(nrows)
            .map(|col| col.iter().map(|c| c.1).max().unwrap_or(0))
            .collect();
        if widths.len() < ncols || widths.iter().sum::<usize>() + gap * (ncols - 1) > total {
            break;
        }
        layout = (ncols, widths);
    }
    let (ncols, widths) = layout;
    let nrows = cells.len().div_ceil(ncols);
    let mut buf = String::new();
    for r in 0..nrows {
        for (c, width) in widths.iter().enumerate() {
            let i = c * nrows + r;
            let Some((text, w)) = cells.get(i) else { break };
            let style = glyph::name_style(&entries[i]);
            let mut styled = String::new();
            if style.is_empty() {
                styled.push_str(text);
            } else {
                styled.push_str(style);
                styled.push_str(text);
                styled.push_str("\x1b[0m");
            }
            linked(&mut buf, &entries[i], &styled);
            if c + 1 < ncols && i + nrows < cells.len() {
                buf.push_str(&" ".repeat(width - w + gap));
            }
        }
        buf.push('\n');
    }
    out.write_all(buf.as_bytes())
}
