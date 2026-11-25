//! Kitty graphics protocol, Unicode placeholder flavour.
//!
//! Every image is sent once with a *virtual placement* (`U=1`) and then drawn by
//! printing ordinary text: `U+10EEEE` plus diacritics for row, column and the high
//! byte of the image id, colored with the low bits of the id. Because the image is
//! just text, it scrolls with the output, survives `clear`, and works inside tmux,
//! which only has to pass the transmit escapes through.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

use crate::diacritics::DIACRITICS;

pub const PLACEHOLDER: char = '\u{10EEEE}';
/// Largest row or column a placeholder can address.
pub const MAX_CELLS: u16 = DIACRITICS.len() as u16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdMode {
    /// Low 24 bits of the id live in a truecolor foreground.
    Truecolor,
    /// tmux without RGB support: low 8 bits in a 256-color foreground. Ids are
    /// restricted to `high << 24 | low` so nothing is lost.
    Indexed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    /// The terminal reads the PNG straight from our cache file. Only a path
    /// crosses the pty, which keeps big listings and tmux fast.
    File,
    /// Base64 PNG over the pty. Needed when the terminal is on another machine.
    Direct,
}

#[derive(Clone, Copy, Debug)]
pub struct Graphics {
    pub tmux: bool,
    pub ids: IdMode,
    pub transfer: Transfer,
}

impl Graphics {
    /// Sends a PNG file and creates a virtual placement `cols` x `rows` cells big.
    /// The terminal scales the image to fit that box, keeping its aspect ratio.
    pub fn transmit(
        &self,
        out: &mut Vec<u8>,
        id: u32,
        png: &Path,
        cols: u16,
        rows: u16,
    ) -> io::Result<()> {
        let head = format!("a=T,U=1,f=100,q=2,i={id},c={cols},r={rows}");
        match self.transfer {
            Transfer::File => {
                let path = B64.encode(png.as_os_str().as_encoded_bytes());
                self.apc(out, &format!("{head},t=f"), &path);
            }
            Transfer::Direct => {
                let data = B64.encode(std::fs::read(png)?);
                self.chunked(out, &head, &data);
            }
        }
        Ok(())
    }

    /// Like [`Graphics::transmit`] but for PNG bytes already in memory.
    pub fn transmit_bytes(&self, out: &mut Vec<u8>, id: u32, png: &[u8], cols: u16, rows: u16) {
        let head = format!("a=T,U=1,f=100,q=2,i={id},c={cols},r={rows}");
        self.chunked(out, &head, &B64.encode(png));
    }

    /// Frees the image and every placement of it in the terminal.
    pub fn delete(&self, out: &mut Vec<u8>, id: u32) {
        self.apc(out, &format!("a=d,d=I,q=2,i={id}"), "");
    }

    fn chunked(&self, out: &mut Vec<u8>, head: &str, data: &str) {
        // The protocol caps chunks at 4096 bytes of base64.
        const CHUNK: usize = 4096;
        let mut chunks = data.as_bytes().chunks(CHUNK).peekable();
        let mut first = true;
        while let Some(chunk) = chunks.next() {
            let more = u8::from(chunks.peek().is_some());
            let keys = if first {
                format!("{head},m={more}")
            } else {
                format!("m={more}")
            };
            // base64 is ASCII, so this never fails.
            self.apc(out, &keys, std::str::from_utf8(chunk).unwrap_or_default());
            first = false;
        }
    }

    fn apc(&self, out: &mut Vec<u8>, keys: &str, payload: &str) {
        if self.tmux {
            // tmux passthrough: wrap in DCS and double every ESC inside.
            out.extend_from_slice(b"\x1bPtmux;\x1b\x1b_G");
            out.extend_from_slice(keys.as_bytes());
            out.push(b';');
            out.extend_from_slice(payload.as_bytes());
            out.extend_from_slice(b"\x1b\x1b\\\x1b\\");
        } else {
            out.extend_from_slice(b"\x1b_G");
            out.extend_from_slice(keys.as_bytes());
            out.push(b';');
            out.extend_from_slice(payload.as_bytes());
            out.extend_from_slice(b"\x1b\\");
        }
    }

    /// Stable image id for `key`, valid under this id mode. Stable ids mean a re-run
    /// reuses the same id for the same picture, so scrollback stays correct.
    pub fn image_id(&self, key: impl Hash) -> u32 {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let h = hasher.finish();
        let high = ((h >> 32) & 0xff) as u32;
        match self.ids {
            IdMode::Truecolor => {
                let low = (h & 0xff_ffff) as u32;
                (high << 24) | if low == 0 { 1 } else { low }
            }
            IdMode::Indexed => (high << 24) | (1 + (h % 255) as u32),
        }
    }

    /// SGR that selects the foreground color encoding `id`.
    pub fn fg(&self, id: u32) -> String {
        match self.ids {
            IdMode::Truecolor => format!(
                "\x1b[38;2;{};{};{}m",
                (id >> 16) & 0xff,
                (id >> 8) & 0xff,
                id & 0xff
            ),
            IdMode::Indexed => format!("\x1b[38;5;{}m", id & 0xff),
        }
    }

    /// The foreground color for `id` as RGB, for UI toolkits (`Indexed` ids use
    /// the palette index in the last byte instead).
    pub fn fg_rgb(id: u32) -> (u8, u8, u8) {
        (
            ((id >> 16) & 0xff) as u8,
            ((id >> 8) & 0xff) as u8,
            (id & 0xff) as u8,
        )
    }
}

/// One placeholder cell: `U+10EEEE` + row + column + high id byte. Always fully
/// specified so partial redraws (as in a TUI diff) stay correct.
pub fn cell(row: u16, col: u16, id: u32) -> String {
    let mut s = String::with_capacity(16);
    push_cell(&mut s, row, col, id);
    s
}

pub fn push_cell(s: &mut String, row: u16, col: u16, id: u32) {
    s.push(PLACEHOLDER);
    s.push(DIACRITICS[row as usize]);
    s.push(DIACRITICS[col as usize]);
    s.push(DIACRITICS[(id >> 24) as usize]);
}

/// One full row of an image, without color. The caller sets the foreground with
/// [`Graphics::fg`] first. Only the first cell carries diacritics; the rest are
/// inferred by the terminal, which keeps the output small.
pub fn push_row(s: &mut String, row: u16, cols: u16, id: u32) {
    push_cell(s, row, 0, id);
    for _ in 1..cols {
        s.push(PLACEHOLDER);
    }
}

/// Picks the cell footprint for an image so it fills as much of a
/// `box_cols` x `box_rows` box as it can without distortion.
pub fn fit(img: (u32, u32), box_cells: (u16, u16), cell_px: (u16, u16)) -> (u16, u16) {
    let (iw, ih) = (img.0.max(1) as f64, img.1.max(1) as f64);
    let (bc, br) = (box_cells.0.max(1) as f64, box_cells.1.max(1) as f64);
    let (cw, ch) = (cell_px.0.max(1) as f64, cell_px.1.max(1) as f64);
    let scale = (bc * cw / iw).min(br * ch / ih);
    let cols = ((iw * scale / cw).round() as u16).clamp(1, box_cells.0.max(1));
    let rows = ((ih * scale / ch).round() as u16).clamp(1, box_cells.1.max(1));
    (cols.min(MAX_CELLS), rows.min(MAX_CELLS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_ids_only_use_low_and_high_bytes() {
        let g = Graphics {
            tmux: true,
            ids: IdMode::Indexed,
            transfer: Transfer::Direct,
        };
        for key in 0..1000 {
            let id = g.image_id(key);
            assert_eq!(id & 0x00ff_ff00, 0);
            assert_ne!(id & 0xff, 0);
        }
    }

    #[test]
    fn fit_keeps_aspect_inside_box() {
        // A portrait page in a square-ish 10x5 box with 2:1 cells.
        assert_eq!(fit((612, 792), (10, 5), (10, 20)), (8, 5));
        // A wide video frame.
        assert_eq!(fit((1920, 1080), (10, 5), (10, 20)), (10, 3));
    }

    #[test]
    fn tmux_wrapping_doubles_escapes() {
        let g = Graphics {
            tmux: true,
            ids: IdMode::Truecolor,
            transfer: Transfer::Direct,
        };
        let mut out = Vec::new();
        g.delete(&mut out, 7);
        assert_eq!(out, b"\x1bPtmux;\x1b\x1b_Ga=d,d=I,q=2,i=7;\x1b\x1b\\\x1b\\");
    }
}
