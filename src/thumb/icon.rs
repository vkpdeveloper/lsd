//! Drawn icons for folders and files without a content thumbnail: a blue
//! folder with a tab, a white page with a folded corner (tinted by type), and
//! a terminal for executables. Used on Linux for files and for folders when
//! the icon theme has none, and on macOS only when QuickLook cannot produce an
//! icon.
//!
//! Shapes are signed distance functions in unit coordinates, so every edge is
//! antialiased at any size.

use super::Icon;

type Color = [f32; 4];

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

/// Straight RGBA, `px` x `px`.
pub fn draw(icon: &Icon, px: u32) -> Vec<u8> {
    let mut c = Canvas::new(px);
    match icon {
        Icon::Folder | Icon::Place(_) | Icon::Package(_) => folder(&mut c),
        Icon::Exec => terminal(&mut c),
        Icon::Plain => page(&mut c, Style::Plain),
        Icon::Ext(ext) => page(&mut c, style(ext)),
    }
    c.into_rgba()
}

fn folder(c: &mut Canvas) {
    let back = |x, y| {
        rounded_rect(x, y, (0.08, 0.20, 0.92, 0.84), 0.055).min(rounded_rect(
            x,
            y,
            (0.08, 0.14, 0.42, 0.30),
            0.045,
        ))
    };
    c.fill(back, |_, y| mix(rgb(86, 166, 232), rgb(62, 142, 216), y));
    c.fill(
        |x, y| rounded_rect(x, y, (0.11, 0.25, 0.89, 0.40), 0.03),
        |_, _| rgb(236, 244, 251),
    );
    c.fill(
        |x, y| rounded_rect(x, y, (0.08, 0.31, 0.92, 0.84), 0.055),
        |_, y| mix(rgb(132, 203, 247), rgb(92, 172, 234), (y - 0.31) / 0.53),
    );
}

#[derive(Clone, Copy)]
enum Style {
    Plain,
    /// Lines of text, optionally colored.
    Text(Option<Color>),
    /// A colored badge.
    Badge(Color),
}

fn style(ext: &str) -> Style {
    let code = rgb(142, 108, 230);
    let data = rgb(32, 168, 168);
    match ext {
        "rs" | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "m" | "mm" | "swift" | "go" | "py"
        | "rb" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "java" | "kt" | "kts" | "scala"
        | "cs" | "php" | "lua" | "sh" | "bash" | "zsh" | "fish" | "pl" | "r" | "dart" | "zig"
        | "nim" | "ex" | "exs" | "erl" | "hs" | "ml" | "clj" | "vue" | "svelte" | "sql" => {
            Style::Text(Some(code))
        }
        "json" | "yaml" | "yml" | "toml" | "xml" | "plist" | "csv" | "tsv" | "ini" | "conf"
        | "cfg" | "lock" | "html" | "htm" | "css" | "scss" => Style::Text(Some(data)),
        "txt" | "md" | "markdown" | "rst" | "log" | "tex" | "org" => Style::Text(None),
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "dmg" | "iso"
        | "pkg" | "deb" | "rpm" | "jar" | "apk" | "whl" => Style::Badge(rgb(222, 162, 52)),
        "mp3" | "m4a" | "aac" | "flac" | "wav" | "aiff" | "aif" | "ogg" | "opus" | "mid" => {
            Style::Badge(rgb(236, 78, 128))
        }
        "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "wmv" | "mpg" | "mpeg" => {
            Style::Badge(rgb(242, 122, 42))
        }
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "avif" | "bmp" | "tif"
        | "tiff" | "svg" | "psd" | "ico" | "icns" => Style::Badge(rgb(62, 182, 92)),
        "pdf" => Style::Badge(rgb(222, 62, 52)),
        "doc" | "docx" | "odt" | "rtf" | "pages" | "epub" => Style::Badge(rgb(58, 120, 222)),
        "xls" | "xlsx" | "ods" | "numbers" => Style::Badge(rgb(48, 160, 82)),
        "ppt" | "pptx" | "odp" | "key" => Style::Badge(rgb(232, 118, 40)),
        "ttf" | "otf" | "ttc" | "woff" | "woff2" => Style::Badge(rgb(96, 96, 110)),
        _ => Style::Plain,
    }
}

fn page(c: &mut Canvas, style: Style) {
    let (x0, y0, x1, y1) = (0.20, 0.08, 0.80, 0.92);
    let fold = 0.17;
    // Outward normal of the cut across the top right corner.
    let (nx, ny) = (
        std::f32::consts::FRAC_1_SQRT_2,
        -std::f32::consts::FRAC_1_SQRT_2,
    );
    let cut = nx * (x1 - fold) + ny * y0;
    let sheet = move |x: f32, y: f32| {
        rounded_rect(x, y, (x0, y0, x1, y1), 0.035).max(nx * x + ny * y - cut)
    };
    let edge = 1.0 / c.px as f32;
    c.fill(move |x, y| sheet(x, y) - edge, |_, _| [0.0, 0.0, 0.0, 0.16]);
    c.fill(sheet, |_, y| mix(rgb(252, 252, 252), rgb(236, 236, 240), y));
    // The flap: the triangle under the cut, darker along the crease.
    let depth = fold * std::f32::consts::FRAC_1_SQRT_2;
    c.fill(
        move |x, y| {
            ((x1 - fold) - x)
                .max(y - (y0 + fold))
                .max(nx * x + ny * y - cut)
        },
        move |x, y| {
            let t = (cut - (nx * x + ny * y)) / depth;
            mix(rgb(200, 200, 208), rgb(234, 234, 238), t)
        },
    );
    match style {
        Style::Plain => {}
        Style::Text(color) => {
            let color = color.unwrap_or(rgb(170, 170, 178));
            let lines = [
                (0.28, 0.58),
                (0.28, 0.66),
                (0.34, 0.70),
                (0.34, 0.62),
                (0.28, 0.52),
            ];
            for (i, (a, b)) in lines.into_iter().enumerate() {
                let y = 0.36 + i as f32 * 0.09;
                c.fill(
                    move |x, yy| segment(x, yy, (a, y), (b, y)) - 0.018,
                    move |_, _| color,
                );
            }
        }
        Style::Badge(color) => {
            c.fill(
                |x, y| rounded_rect(x, y, (0.27, 0.60, 0.73, 0.76), 0.04),
                move |_, y| mix(color, darker(color), (y - 0.60) / 0.16),
            );
        }
    }
}

fn terminal(c: &mut Canvas) {
    let body = (0.12, 0.18, 0.88, 0.82);
    c.fill(
        |x, y| rounded_rect(x, y, body, 0.07),
        |_, y| mix(rgb(58, 58, 64), rgb(30, 30, 34), y),
    );
    let green = rgb(88, 224, 108);
    let w = 0.032;
    c.fill(
        move |x, y| {
            segment(x, y, (0.27, 0.37), (0.39, 0.47)).min(segment(x, y, (0.39, 0.47), (0.27, 0.57)))
                - w
        },
        move |_, _| green,
    );
    c.fill(
        move |x, y| segment(x, y, (0.46, 0.58), (0.62, 0.58)) - w,
        move |_, _| green,
    );
}

struct Canvas {
    px: u32,
    /// Premultiplied RGBA.
    buf: Vec<Color>,
}

impl Canvas {
    fn new(px: u32) -> Canvas {
        Canvas {
            px,
            buf: vec![[0.0; 4]; (px * px) as usize],
        }
    }

    /// Paints `color` wherever `sdf` (unit coordinates) is negative, with a
    /// one pixel soft edge.
    fn fill(&mut self, sdf: impl Fn(f32, f32) -> f32, color: impl Fn(f32, f32) -> Color) {
        let scale = self.px as f32;
        for (i, dst) in self.buf.iter_mut().enumerate() {
            let x = ((i as u32 % self.px) as f32 + 0.5) / scale;
            let y = ((i as u32 / self.px) as f32 + 0.5) / scale;
            let cover = (0.5 - sdf(x, y) * scale).clamp(0.0, 1.0);
            if cover == 0.0 {
                continue;
            }
            let [r, g, b, a] = color(x, y);
            let a = a * cover;
            for (d, s) in dst.iter_mut().zip([r * a, g * a, b * a, a]) {
                *d = s + *d * (1.0 - a);
            }
        }
    }

    fn into_rgba(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.buf.len() * 4);
        for [r, g, b, a] in self.buf {
            let un = if a > 0.0 { 1.0 / a } else { 0.0 };
            for v in [r * un, g * un, b * un, a] {
                out.push((v.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
        out
    }
}

fn rounded_rect(x: f32, y: f32, (x0, y0, x1, y1): (f32, f32, f32, f32), r: f32) -> f32 {
    let qx = (x - (x0 + x1) / 2.0).abs() - ((x1 - x0) / 2.0 - r);
    let qy = (y - (y0 + y1) / 2.0).abs() - ((y1 - y0) / 2.0 - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn segment(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py, bx, by) = (x - a.0, y - a.1, b.0 - a.0, b.1 - a.1);
    let t = ((px * bx + py * by) / (bx * bx + by * by)).clamp(0.0, 1.0);
    (px - bx * t).hypot(py - by * t)
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn darker(c: Color) -> Color {
    [c[0] * 0.82, c[1] * 0.82, c[2] * 0.82, c[3]]
}
