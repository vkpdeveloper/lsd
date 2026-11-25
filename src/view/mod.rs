//! Inline views: output that lands in the scrollback like `ls` does.
//!
//! Views lay out *blocks* of text lines and ask the [`Painter`] to draw picture
//! rows into them. kitty pictures and half blocks are plain text inside the
//! lines. iTerm2 pictures are drawn after the block's text by moving the cursor
//! back up into the space the block left blank.

pub mod gallery;
pub mod icons;
pub mod list;
pub mod plain;
pub mod quicklook;
pub mod tree;

use std::collections::HashSet;
use std::io::{self, Write};
use std::rc::Rc;

use crate::entry::Entry;
use crate::glyph::Icons;
use crate::halfblock;
use crate::kitty;
use crate::term::{Protocol, Term};
use crate::thumb::{Thumb, Thumbnailer};
use crate::{fmt, iterm};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum IconSize {
    Small,
    #[default]
    Medium,
    Large,
    Huge,
}

impl IconSize {
    /// Width of the icon box in cells. Height follows from the cell aspect ratio.
    pub fn box_cols(self) -> u16 {
        match self {
            IconSize::Small => 6,
            IconSize::Medium => 10,
            IconSize::Large => 14,
            IconSize::Huge => 20,
        }
    }
}

pub struct Ctx<'a> {
    pub term: &'a Term,
    pub thumbs: Thumbnailer,
    pub size: IconSize,
    pub icons: Icons,
}

impl Ctx<'_> {
    /// Rows that make a `cols` wide box look square on this terminal.
    pub fn square_rows(&self, cols: u16) -> u16 {
        let (cw, ch) = self.term.cell;
        ((cols as f64 * cw as f64 / ch.max(1) as f64).round() as u16).max(1)
    }

    /// Thumbnail pixel size (bucketed) for a box of `cells`.
    pub fn px_for(&self, cells: (u16, u16)) -> u32 {
        let (cw, ch) = self.term.cell;
        crate::thumb::bucket((cells.0 as u32 * cw as u32).max(cells.1 as u32 * ch as u32))
    }
}

/// A picture ready to draw, `cols` x `rows` cells.
#[derive(Clone)]
pub struct Pic {
    pub cols: u16,
    pub rows: u16,
    data: PicData,
}

#[derive(Clone)]
enum PicData {
    Kitty(u32),
    Iterm(Rc<Vec<u8>>),
    Text(Rc<Vec<Vec<halfblock::Cell>>>),
}

pub struct Painter {
    protocol: Protocol,
    cell: (u16, u16),
    truecolor: bool,
    /// kitty ids already sent this run.
    sent: HashSet<u32>,
    /// Escapes that must reach the terminal before the next block's text.
    pre: Vec<u8>,
    /// iTerm2 images waiting for the block to be written: (x, y, pic).
    pending: Vec<(u16, u16, Pic)>,
}

impl Painter {
    pub fn new(term: &Term) -> Painter {
        Painter {
            protocol: term.protocol,
            cell: term.cell,
            truecolor: term.truecolor,
            sent: HashSet::new(),
            pre: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Readies `thumb` to fill as much of `box_cells` as it can without distortion.
    pub fn prepare(&mut self, thumb: &Thumb, box_cells: (u16, u16)) -> Option<Pic> {
        let (cols, rows) = kitty::fit((thumb.width, thumb.height), box_cells, self.cell);
        let data = match self.protocol {
            Protocol::Kitty(g) => {
                let id = g.image_id((&thumb.path, cols, rows));
                if self.sent.insert(id) {
                    g.transmit(&mut self.pre, id, &thumb.path, cols, rows)
                        .ok()?;
                }
                PicData::Kitty(id)
            }
            Protocol::Iterm { .. } => PicData::Iterm(Rc::new(std::fs::read(&thumb.path).ok()?)),
            Protocol::Text => PicData::Text(Rc::new(halfblock::cells(&thumb.path, cols, rows)?)),
        };
        Some(Pic { cols, rows, data })
    }

    /// Appends row `r` of `pic` to a line. `x` and `y` are where this row lands,
    /// in cells from the block's top left corner.
    pub fn push_row(&mut self, s: &mut String, pic: &Pic, r: u16, x: u16, y: u16) {
        match &pic.data {
            PicData::Kitty(id) => {
                let Protocol::Kitty(g) = self.protocol else {
                    return;
                };
                s.push_str(&g.fg(*id));
                kitty::push_row(s, r, pic.cols, *id);
                s.push_str("\x1b[39m");
            }
            PicData::Iterm(_) => {
                if r == 0 {
                    self.pending.push((x, y, pic.clone()));
                }
                s.push_str(&" ".repeat(pic.cols as usize));
            }
            PicData::Text(cells) => match cells.get(r as usize) {
                Some(row) => halfblock::push_line(s, row, self.truecolor),
                None => s.push_str(&" ".repeat(pic.cols as usize)),
            },
        }
    }

    /// Writes one block: pending transmits, its text (`height` lines, ending in a
    /// newline), then any iTerm2 images on top of it.
    pub fn flush(&mut self, out: &mut impl Write, text: &str, height: u16) -> io::Result<()> {
        let mut buf = std::mem::take(&mut self.pre);
        buf.extend_from_slice(text.as_bytes());
        if let Protocol::Iterm { tmux } = self.protocol
            && !self.pending.is_empty()
        {
            buf.extend_from_slice(format!("\x1b[{height}A").as_bytes());
            for (x, y, pic) in self.pending.drain(..) {
                let PicData::Iterm(png) = &pic.data else {
                    continue;
                };
                buf.extend_from_slice(b"\x1b7");
                if y > 0 {
                    buf.extend_from_slice(format!("\x1b[{y}B").as_bytes());
                }
                buf.extend_from_slice(format!("\x1b[{}G", x + 1).as_bytes());
                iterm::image(&mut buf, tmux, png, pic.cols, pic.rows);
                buf.extend_from_slice(b"\x1b8");
            }
            buf.extend_from_slice(format!("\x1b[{height}B\r").as_bytes());
        }
        out.write_all(&buf)?;
        out.flush()
    }
}

/// Wraps `text` in an OSC 8 link to the entry so a click opens it.
pub fn linked(s: &mut String, entry: &Entry, text: &str) {
    if text.is_empty() {
        return;
    }
    s.push_str(&fmt::link_start(&fmt::file_url(&entry.path)));
    s.push_str(text);
    s.push_str(fmt::LINK_END);
}

/// Ends the current output line, dropping trailing padding.
pub fn end_line(text: &mut String) {
    let trimmed = text.trim_end_matches(' ').len();
    text.truncate(trimmed);
    text.push('\n');
}
