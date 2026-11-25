//! iTerm2 inline images (OSC 1337), used for iTerm2 and WezTerm. The image is
//! drawn at the cursor over a box of cells. Callers position the cursor and save
//! and restore it around the image, so placement does not depend on whether the
//! terminal honors `doNotMoveCursor`.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

/// Appends one image covering `cols` x `rows` cells at the cursor.
pub fn image(out: &mut Vec<u8>, tmux: bool, png: &[u8], cols: u16, rows: u16) {
    let body = format!(
        "\x1b]1337;File=inline=1;size={};width={cols};height={rows};preserveAspectRatio=1;doNotMoveCursor=1:{}\x07",
        png.len(),
        B64.encode(png)
    );
    if tmux {
        out.extend_from_slice(b"\x1bPtmux;");
        out.extend_from_slice(body.replace('\x1b', "\x1b\x1b").as_bytes());
        out.extend_from_slice(b"\x1b\\");
    } else {
        out.extend_from_slice(body.as_bytes());
    }
}
