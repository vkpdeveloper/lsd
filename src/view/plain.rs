//! Text-only output for pipes and terminals without kitty graphics.

use std::io::{self, Write};

use crate::entry::Entry;
use crate::fmt;
use crate::term::Term;

/// One name per line, exactly what scripts expect from `ls` in a pipe.
pub fn lines(entries: &[Entry], out: &mut impl Write) -> io::Result<()> {
    let mut buf = String::new();
    for e in entries {
        buf.push_str(&e.name);
        buf.push('\n');
    }
    out.write_all(buf.as_bytes())
}

/// `ls`-style columns, filled top to bottom. Folders are bold blue with a slash.
pub fn columns(term: &Term, entries: &[Entry], out: &mut impl Write) -> io::Result<()> {
    if !term.tty {
        return lines(entries, out);
    }
    if entries.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = entries
        .iter()
        .map(|e| {
            if e.is_dir {
                format!("{}/", e.name)
            } else {
                e.name.clone()
            }
        })
        .collect();
    let gap = 2;
    let total = term.cols as usize;
    // Most columns whose widths still fit, like ls.
    let mut layout = (
        1,
        vec![names.iter().map(|n| fmt::width(n)).max().unwrap_or(0)],
    );
    for ncols in 2..=names.len() {
        let nrows = names.len().div_ceil(ncols);
        let widths: Vec<usize> = (0..ncols)
            .map(|c| {
                names
                    .iter()
                    .skip(c * nrows)
                    .take(nrows)
                    .map(|n| fmt::width(n))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        if widths.iter().sum::<usize>() + gap * (ncols - 1) > total {
            break;
        }
        layout = (ncols, widths);
    }
    let (ncols, widths) = layout;
    let nrows = names.len().div_ceil(ncols);
    let mut buf = String::new();
    for r in 0..nrows {
        for c in 0..ncols {
            let i = c * nrows + r;
            let Some(name) = names.get(i) else { break };
            let styled = if entries[i].is_dir {
                format!("\x1b[1;34m{name}\x1b[0m")
            } else {
                name.clone()
            };
            buf.push_str(&styled);
            let last = c + 1 == ncols || (c + 1) * nrows + r >= names.len();
            if !last {
                buf.push_str(&" ".repeat(widths[c] - fmt::width(name) + gap));
            }
        }
        buf.push('\n');
    }
    out.write_all(buf.as_bytes())
}
