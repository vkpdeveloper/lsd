//! Pictures in the ratatui buffer, for each image protocol.
//!
//! - kitty: Unicode placeholders are ordinary cells, so ratatui's diff moves
//!   and clears them for free. Images are transmitted once, before the frame
//!   that first shows them.
//! - iTerm2: images sit on top of the text. Their cells are marked `Skip` so
//!   ratatui never paints over them, and an image is only sent again when it
//!   moves, changes, or was covered. When it goes away, its cells differ from
//!   the `Skip` cells of the previous frame, so ratatui repaints them.
//! - Text: colored half blocks, decoded on a worker thread.

use std::collections::HashSet;
use std::path::PathBuf;

use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use super::work::Loader;
use crate::halfblock;
use crate::iterm;
use crate::kitty::{self, Graphics, IdMode};
use crate::term::{Protocol, Term};
use crate::thumb::{self, Thumb};

#[derive(Clone, PartialEq, Eq)]
struct Placed {
    rect: Rect,
    png: PathBuf,
}

pub struct Pics {
    protocol: Protocol,
    /// Pixel size of one cell.
    pub cell: (u16, u16),
    truecolor: bool,
    /// kitty ids the terminal already has.
    sent: HashSet<u32>,
    /// kitty transmits for the next frame.
    pre: Vec<u8>,
    /// iTerm2 images on screen after the last frame, and whether all of each is
    /// visible.
    shown: Vec<(Placed, bool)>,
    /// iTerm2 images placed while drawing this frame.
    placed: Vec<Placed>,
}

impl Pics {
    pub fn new(term: &Term) -> Pics {
        Pics {
            protocol: term.protocol,
            cell: term.cell,
            truecolor: term.truecolor,
            sent: HashSet::new(),
            pre: Vec::new(),
            shown: Vec::new(),
            placed: Vec::new(),
        }
    }

    /// True when the terminal shows real images. Otherwise small icons are glyphs
    /// and only large pictures are drawn, with half blocks.
    pub fn images(&self) -> bool {
        self.protocol.has_images()
    }

    /// Rows that make a `cols` wide box look square.
    pub fn square_rows(&self, cols: u16) -> u16 {
        let (cw, ch) = if self.images() { self.cell } else { (1, 2) };
        ((cols as f64 * cw as f64 / ch.max(1) as f64).round() as u16).max(1)
    }

    /// Thumbnail size to ask for so a picture can fill `cells`.
    pub fn px(&self, (cols, rows): (u16, u16)) -> u32 {
        if self.images() {
            let (cw, ch) = self.cell;
            thumb::bucket((cols as u32 * cw as u32).max(rows as u32 * ch as u32))
        } else {
            // Half blocks are two pixels per cell; twice that resamples nicely.
            thumb::bucket(2 * (cols as u32).max(rows as u32 * 2))
        }
    }

    /// The cells `thumb` takes when drawn as large as fits in `area`, centered.
    pub fn fit(&self, area: Rect, thumb: &Thumb) -> Rect {
        let cell = if self.images() { self.cell } else { (1, 2) };
        let (cols, rows) = kitty::fit((thumb.width, thumb.height), (area.width, area.height), cell);
        Rect {
            x: area.x + (area.width - cols) / 2,
            y: area.y + (area.height - rows) / 2,
            width: cols,
            height: rows,
        }
    }

    /// Draws `thumb` over `rect` (from `fit`) on `bg`. Returns false when it
    /// cannot be drawn yet.
    pub fn draw(
        &mut self,
        buf: &mut Buffer,
        load: &mut Loader,
        rect: Rect,
        thumb: &Thumb,
        bg: Color,
    ) -> bool {
        let rect = rect.intersection(buf.area);
        if rect.is_empty() {
            return false;
        }
        match self.protocol {
            Protocol::Kitty(g) => return self.kitty(buf, g, rect, thumb, bg).is_some(),
            Protocol::Iterm { .. } => {
                for pos in rect.positions() {
                    if let Some(c) = buf.cell_mut(pos) {
                        c.reset();
                        c.set_bg(bg).set_diff_option(CellDiffOption::Skip);
                    }
                }
                self.placed.push(Placed {
                    rect,
                    png: thumb.path.clone(),
                });
            }
            Protocol::Text => {
                let Some(grid) = load.half(&thumb.path, rect.width, rect.height) else {
                    return false;
                };
                for (r, line) in grid.iter().enumerate() {
                    for (c, px) in line.iter().enumerate() {
                        let pos = (rect.x + c as u16, rect.y + r as u16);
                        if let Some(cell) = buf.cell_mut(pos) {
                            self.half_cell(cell, *px, bg);
                        }
                    }
                }
            }
        }
        true
    }

    fn kitty(
        &mut self,
        buf: &mut Buffer,
        g: Graphics,
        rect: Rect,
        thumb: &Thumb,
        bg: Color,
    ) -> Option<()> {
        let id = g.image_id((&thumb.path, rect.width, rect.height));
        if !self.sent.contains(&id) {
            g.transmit(&mut self.pre, id, &thumb.path, rect.width, rect.height)
                .ok()?;
            self.sent.insert(id);
        }
        let fg = match g.ids {
            IdMode::Truecolor => {
                let (r, gr, b) = Graphics::fg_rgb(id);
                Color::Rgb(r, gr, b)
            }
            IdMode::Indexed => Color::Indexed((id & 0xff) as u8),
        };
        let mut sym = String::with_capacity(16);
        for r in 0..rect.height {
            for c in 0..rect.width {
                let Some(cell) = buf.cell_mut((rect.x + c, rect.y + r)) else {
                    continue;
                };
                sym.clear();
                kitty::push_cell(&mut sym, r, c, id);
                cell.reset();
                cell.set_symbol(&sym).set_fg(fg).set_bg(bg);
            }
        }
        Some(())
    }

    fn half_cell(&self, cell: &mut ratatui::buffer::Cell, px: halfblock::Cell, bg: Color) {
        let color = |(r, g, b): crate::term::Rgb| {
            if self.truecolor {
                Color::Rgb(r, g, b)
            } else {
                Color::Indexed(halfblock::ansi256((r, g, b)))
            }
        };
        cell.reset();
        cell.modifier = Modifier::empty();
        match (px.top, px.bottom) {
            (Some(t), Some(b)) => cell.set_symbol("▀").set_fg(color(t)).set_bg(color(b)),
            (Some(t), None) => cell.set_symbol("▀").set_fg(color(t)).set_bg(bg),
            (None, Some(b)) => cell.set_symbol("▄").set_fg(color(b)).set_bg(bg),
            (None, None) => cell.set_symbol(" ").set_bg(bg),
        };
    }

    /// The screen was cleared (resize), so every iTerm2 image must be sent again.
    pub fn invalidate(&mut self) {
        self.shown.clear();
    }

    /// Escapes to write before the frame's text: kitty transmits.
    pub fn take_pre(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pre)
    }

    /// Settles iTerm2 images once the whole frame is drawn, and returns the
    /// escapes to write after the frame's text.
    pub fn finish(&mut self, buf: &mut Buffer) -> Vec<u8> {
        let Protocol::Iterm { tmux } = self.protocol else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut shown = Vec::new();
        for p in std::mem::take(&mut self.placed) {
            let before = self
                .shown
                .iter()
                .find(|(q, _)| *q == p)
                .map(|(_, whole)| *whole);
            let intact = p.rect.positions().all(|pos| {
                buf.cell(pos)
                    .is_some_and(|c| c.diff_option == CellDiffOption::Skip)
            });
            if !intact {
                // An overlay drew on top. A picture already on screen keeps
                // showing around it and is sent again once uncovered. A new one
                // is not sent at all, and its cells are cleared instead.
                if before.is_some() {
                    shown.push((p, false));
                } else {
                    for pos in p.rect.positions() {
                        if let Some(c) = buf.cell_mut(pos)
                            && c.diff_option == CellDiffOption::Skip
                        {
                            c.set_diff_option(CellDiffOption::None);
                        }
                    }
                }
                continue;
            }
            if before != Some(true) {
                // Clear the box first: the terminal keeps old text around a
                // picture that does not fill it.
                for pos in p.rect.positions() {
                    if let Some(c) = buf.cell_mut(pos) {
                        c.set_diff_option(CellDiffOption::AlwaysUpdate);
                    }
                }
                if let Ok(png) = std::fs::read(&p.png) {
                    out.extend_from_slice(
                        format!("\x1b7\x1b[{};{}H", p.rect.y + 1, p.rect.x + 1).as_bytes(),
                    );
                    iterm::image(&mut out, tmux, &png, p.rect.width, p.rect.height);
                    out.extend_from_slice(b"\x1b8");
                }
            }
            shown.push((p, true));
        }
        self.shown = shown;
        out
    }

    /// Frees every kitty image we sent.
    pub fn cleanup(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Protocol::Kitty(g) = self.protocol {
            for id in self.sent.drain() {
                g.delete(&mut out, id);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::CellWidth;

    use crate::kitty;

    #[test]
    fn placeholder_is_one_column_wide() {
        let g = kitty::Graphics {
            tmux: false,
            ids: kitty::IdMode::Truecolor,
            transfer: kitty::Transfer::File,
        };
        for key in 0..200 {
            let id = g.image_id(key);
            for (r, c) in [(0, 0), (3, 7), (40, 120), (296, 296)] {
                let mut cell = String::new();
                kitty::push_cell(&mut cell, r, c, id);
                assert_eq!(cell.as_str().cell_width(), 1);
            }
        }
    }
}
