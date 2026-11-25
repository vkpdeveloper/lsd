//! `lsd data.json`: the file itself, like pressing Space in Finder. JSON is
//! pretty-printed and code colored. Other files get their card.

use std::io::{self, Write};

use super::{Ctx, end_line, linked};
use crate::doc::{self, Kind};
use crate::entry::Entry;
use crate::{fmt, meta};

/// Shows `entry` if it is text. Returns false when it is not, so the caller
/// can show its card instead.
pub fn render(ctx: &Ctx, entry: &Entry, out: &mut impl Write) -> io::Result<bool> {
    let Some((kind, text, cut)) = doc::load(entry) else {
        return Ok(false);
    };
    let width = (ctx.term.cols as usize).saturating_sub(1).max(20);
    let json = match kind {
        Kind::Json => doc::json::render(&text, width),
        Kind::Code => None,
    };
    let lines: Box<dyn Iterator<Item = doc::Line>> = match json {
        Some(lines) => Box::new(lines.into_iter()),
        None => doc::code::render(&text, &entry.path, width),
    };

    let mut buf = header(entry, width);
    // Coloring is the slow part, so the first screen goes out on its own.
    let first_screen = ctx.term.rows as usize;
    for (i, line) in lines.enumerate() {
        doc::push_ansi(&mut buf, &line, ctx.term.truecolor);
        end_line(&mut buf);
        if i + 1 == first_screen {
            out.write_all(buf.as_bytes())?;
            out.flush()?;
            buf.clear();
        }
    }
    if cut {
        buf.push_str(&format!(
            "\n\x1b[2mShowing the first {}\x1b[0m\n",
            fmt::size(doc::MAX_BYTES as u64)
        ));
    }
    out.write_all(buf.as_bytes())?;
    Ok(true)
}

/// Name, then kind and size, over a thin rule.
fn header(entry: &Entry, width: usize) -> String {
    let mut s = String::from("\x1b[1m");
    linked(&mut s, entry, &entry.name);
    s.push_str("\x1b[0m  \x1b[2m");
    s.push_str(&meta::kind(entry));
    s.push_str(" - ");
    s.push_str(&fmt::size(entry.size));
    s.push_str("\x1b[0m\n\x1b[2m");
    s.push_str(&"─".repeat(width));
    s.push_str("\x1b[0m\n");
    s
}
