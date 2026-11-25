//! Pictures for terminals without an image protocol: each cell shows two
//! stacked pixels with `▀`, foreground on top and background below.

use std::path::Path;

use image::imageops::FilterType;

use crate::term::Rgb;

/// Two stacked pixels. `None` means transparent, so the terminal background
/// shows through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub top: Option<Rgb>,
    pub bottom: Option<Rgb>,
}

/// Scales a PNG to exactly `cols` x `rows` cells. Pick the footprint with
/// `kitty::fit` first to keep the aspect ratio.
pub fn cells(png: &Path, cols: u16, rows: u16) -> Option<Vec<Vec<Cell>>> {
    let img = image::open(png).ok()?;
    let img = img
        .resize_exact(
            cols.max(1) as u32,
            rows.max(1) as u32 * 2,
            FilterType::Triangle,
        )
        .to_rgba8();
    let px = |x: u32, y: u32| {
        let p = img.get_pixel(x, y).0;
        (p[3] >= 128).then_some((p[0], p[1], p[2]))
    };
    let grid = (0..rows as u32)
        .map(|r| {
            (0..cols as u32)
                .map(|c| Cell {
                    top: px(c, r * 2),
                    bottom: px(c, r * 2 + 1),
                })
                .collect()
        })
        .collect();
    Some(grid)
}

/// Appends one row of cells as colored text and resets colors at the end.
pub fn push_line(s: &mut String, row: &[Cell], truecolor: bool) {
    let color = |base: u8, (r, g, b): Rgb| {
        if truecolor {
            format!("\x1b[{base};2;{r};{g};{b}m")
        } else {
            format!("\x1b[{base};5;{}m", ansi256((r, g, b)))
        }
    };
    for cell in row {
        match (cell.top, cell.bottom) {
            (Some(t), Some(b)) => {
                s.push_str(&color(38, t));
                s.push_str(&color(48, b));
                s.push('▀');
            }
            (Some(t), None) => {
                s.push_str("\x1b[49m");
                s.push_str(&color(38, t));
                s.push('▀');
            }
            (None, Some(b)) => {
                s.push_str("\x1b[49m");
                s.push_str(&color(38, b));
                s.push('▄');
            }
            (None, None) => {
                s.push_str("\x1b[49m ");
            }
        }
    }
    s.push_str("\x1b[39;49m");
}

/// Nearest xterm 256-color index: the 6x6x6 cube or the gray ramp.
pub fn ansi256((r, g, b): Rgb) -> u8 {
    let level = |v: u8| {
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            (v - 35) / 40
        }
    };
    let cube_val = |i: u8| if i == 0 { 0 } else { 55 + i * 40 };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = 16 + 36 * ri + 6 * gi + bi;
    let avg = (r as u16 + g as u16 + b as u16) / 3;
    let gray_i = if avg > 238 {
        23
    } else {
        avg.saturating_sub(3) / 10
    } as u8;
    let gray_v = 8 + 10 * gray_i;
    let dist = |(x, y, z): Rgb| {
        let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    if dist((gray_v, gray_v, gray_v)) < dist((cube_val(ri), cube_val(gi), cube_val(bi))) {
        232 + gray_i
    } else {
        cube
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_to_256_palette() {
        assert_eq!(ansi256((0, 0, 0)), 16);
        assert_eq!(ansi256((255, 0, 0)), 196);
        assert_eq!(ansi256((128, 128, 128)), 244);
    }
}
