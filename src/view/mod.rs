//! Inline views: output that lands in the scrollback like `ls` does.

pub mod icons;
pub mod plain;

use std::collections::HashSet;
use std::io::{self, Write};

use crate::entry::Entry;
use crate::fmt;
use crate::kitty::{self, Graphics};
use crate::term::Term;
use crate::thumb::{Thumb, Thumbnailer};

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

/// An image that has been sent to the terminal and can be drawn as text.
#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
}

/// Sends each picture once per run and hands back what is needed to draw it.
pub struct Placer {
    pub g: Graphics,
    cell: (u16, u16),
    sent: HashSet<u32>,
}

impl Placer {
    pub fn new(g: Graphics, cell: (u16, u16)) -> Placer {
        Placer {
            g,
            cell,
            sent: HashSet::new(),
        }
    }

    /// Queues the transmit escape into `out` (if this picture at this size is
    /// new) and returns the footprint that fits `box_cells`.
    pub fn place(
        &mut self,
        out: &mut Vec<u8>,
        thumb: &Thumb,
        box_cells: (u16, u16),
    ) -> Option<Placed> {
        let (cols, rows) = kitty::fit((thumb.width, thumb.height), box_cells, self.cell);
        let id = self.g.image_id((&thumb.path, cols, rows));
        if self.sent.insert(id) {
            self.g.transmit(out, id, &thumb.path, cols, rows).ok()?;
        }
        Some(Placed { id, cols, rows })
    }

    /// Appends image row `row` of `p` as colored placeholder text.
    pub fn push_row(&self, s: &mut String, p: Placed, row: u16) {
        s.push_str(&self.g.fg(p.id));
        kitty::push_row(s, row, p.cols, p.id);
        s.push_str("\x1b[39m");
    }
}

/// Wraps `text` in an OSC 8 link to the entry so a click opens it.
pub fn linked(s: &mut String, entry: &Entry, text: &str) {
    s.push_str(&fmt::link_start(&fmt::file_url(&entry.path)));
    s.push_str(text);
    s.push_str(fmt::LINK_END);
}

pub fn write_all(out: &mut impl Write, buf: &[u8]) -> io::Result<()> {
    out.write_all(buf)?;
    out.flush()
}
