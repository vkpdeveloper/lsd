//! Gallery view: a large preview of each item with its details beside it, like
//! Finder's Gallery view and Get Info. Also used for `lsd <one file>`.

use std::io::{self, Write};
use std::sync::mpsc;

use super::{Ctx, Painter, end_line, linked};
use crate::entry::Entry;
use crate::meta::{self, Info};
use crate::thumb::{Mode, Thumb};
use crate::{fmt, glyph};

/// Cells for each picture.
fn picture_box(ctx: &Ctx) -> (u16, u16) {
    let box_cols = ((ctx.term.cols as usize * 2 / 5) as u16).clamp(12, 48);
    // Keep every block on screen: iTerm2 pictures are placed by moving the
    // cursor back up, which cannot reach above the top row.
    let box_rows = ctx
        .square_rows(box_cols)
        .min(ctx.term.rows.saturating_sub(2).max(6));
    (box_cols, box_rows)
}

/// Thumbnail size the gallery asks for.
pub fn px(ctx: &Ctx) -> u32 {
    ctx.px_for(picture_box(ctx))
}

pub fn render(ctx: &Ctx, entries: &[Entry], out: &mut impl Write) -> io::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let total = ctx.term.cols as usize;
    let (box_cols, box_rows) = picture_box(ctx);
    let text_x = box_cols as usize + 3;
    let text_w = total.saturating_sub(text_x + 1).max(20);

    let rx = ctx.thumbs.batch(entries, px(ctx), Mode::Preview);
    let infos = infos(entries);
    let mut thumbs: Vec<Option<Option<Thumb>>> = vec![None; entries.len()];
    let mut details: Vec<Option<Info>> = vec![None; entries.len()];
    let mut painter = Painter::new(ctx.term);

    for (i, entry) in entries.iter().enumerate() {
        while thumbs[i].is_none() {
            match rx.recv() {
                Ok((j, t)) => thumbs[j] = Some(t),
                Err(_) => break,
            }
        }
        while details[i].is_none() {
            match infos.recv() {
                Ok((j, info)) => details[j] = Some(info),
                Err(_) => break,
            }
        }
        let info = details[i].take().unwrap_or_default();
        let pic = thumbs[i]
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|t| painter.prepare(t, (box_cols, box_rows)));
        let side = side_text(entry, &info, text_w);
        // The picture starts on the name's line, so wide pictures do not leave
        // a gap above the details or blank rows under them.
        let pic_rows = pic.as_ref().map_or(1, |p| p.rows);
        let height = pic_rows.max(side.len() as u16);
        let left = pic.as_ref().map_or(0, |p| (box_cols - p.cols) / 2);

        let mut text = String::new();
        if i > 0 {
            text.push('\n');
        }
        for line in 0..height {
            let mut used = 0;
            if let Some(p) = &pic
                && line < p.rows
            {
                text.push_str(&" ".repeat(left as usize + 1));
                let mut cells = String::new();
                painter.push_row(&mut cells, p, line, left + 1, line + u16::from(i > 0));
                linked(&mut text, entry, &cells);
                used = left as usize + 1 + p.cols as usize;
            } else if pic.is_none() && line == 0 {
                let icon = glyph::icon(entry, ctx.icons);
                let pad = (box_cols as usize).saturating_sub(fmt::width(icon)) / 2 + 1;
                text.push_str(&" ".repeat(pad));
                text.push_str(icon);
                used = pad + fmt::width(icon);
            }
            if let Some(s) = side.get(line as usize) {
                text.push_str(&" ".repeat(text_x.saturating_sub(used)));
                text.push_str(s);
            }
            end_line(&mut text);
        }
        painter.flush(out, &text, height + u16::from(i > 0))?;
    }
    Ok(())
}

/// Name, kind and size, then the Information table, each line already styled.
fn side_text(entry: &Entry, info: &Info, w: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let name = fmt::ellipsize_middle(&entry.name, w);
    let mut title = String::from("\x1b[1m");
    linked(&mut title, entry, &name);
    title.push_str("\x1b[0m");
    lines.push(title);

    let size = if entry.is_dir {
        info.items
            .map(|n| format!("{n} {}", if n == 1 { "item" } else { "items" }))
    } else {
        Some(fmt::size(entry.size))
    };
    let kind = if info.kind.is_empty() {
        meta::kind(entry)
    } else {
        info.kind.clone()
    };
    let sub = match size {
        Some(size) => format!("{kind} - {size}"),
        None => kind,
    };
    lines.push(format!("\x1b[2m{}\x1b[0m", fmt::ellipsize_end(&sub, w)));
    lines.push(String::new());
    lines.push("\x1b[1mInformation\x1b[0m".into());

    let mut rows: Vec<(&str, String)> = Vec::new();
    if let Some(t) = entry.created {
        rows.push(("Created", fmt::date(t)));
    }
    if let Some(t) = entry.modified {
        rows.push(("Modified", fmt::date(t)));
    }
    if let Some(t) = info.last_opened {
        rows.push(("Last opened", fmt::date(t)));
    }
    if let Some((x, y)) = info.dimensions {
        rows.push(("Dimensions", format!("{x} × {y}")));
    }
    if let Some(p) = info.pages {
        rows.push(("Pages", p.to_string()));
    }
    if let Some(d) = info.duration {
        rows.push(("Duration", fmt::duration(d)));
    }
    let table_w = w.min(44);
    for (label, value) in rows {
        let gap = table_w
            .saturating_sub(fmt::width(label) + fmt::width(&value))
            .max(2);
        lines.push(format!("\x1b[2m{label}\x1b[0m{}{value}", " ".repeat(gap)));
    }
    lines
}

/// `meta::info` for every entry on a few threads, streamed as they finish.
fn infos(entries: &[Entry]) -> mpsc::Receiver<(usize, Info)> {
    let (tx, rx) = mpsc::channel();
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(entries.len())
        .max(1);
    for w in 0..workers {
        let tx = tx.clone();
        let mine: Vec<(usize, Entry)> = entries
            .iter()
            .cloned()
            .enumerate()
            .skip(w)
            .step_by(workers)
            .collect();
        std::thread::spawn(move || {
            for (i, entry) in mine {
                if tx.send((i, meta::info(&entry))).is_err() {
                    break;
                }
            }
        });
    }
    rx
}
