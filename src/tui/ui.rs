//! The window: toolbar, sidebar, path bar, key hints, help, and the helpers
//! every view uses to put text and pictures into the buffer.

use std::path::Path;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::{Margin, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Clear, Widget};

use super::app::{App, Hit, Mode, Rename, View};
use super::git;
use super::pics::Pics;
use super::theme::Theme;
use super::views;
use super::work::Loader;
use crate::entry::Entry;
use crate::fmt;
use crate::git::Status;
use crate::glyph::{self, Icons};
use crate::places::{Place, PlaceKind};

pub struct Ctx<'a> {
    pub th: &'a Theme,
    pub pics: &'a mut Pics,
    pub load: &'a mut Loader,
    pub icons: Icons,
    /// The git status of the repository holding the current folder.
    pub git: Option<Arc<Status>>,
}

const SIDEBAR_W: u16 = 20;

pub fn draw(buf: &mut Buffer, area: Rect, app: &mut App, cx: &mut Ctx) {
    app.hits.clear();
    if area.width < 40 || area.height < 10 {
        text(
            buf,
            area.x,
            area.y,
            "Make the window larger",
            area.width,
            cx.th.dim,
        );
        return;
    }
    if matches!(app.mode, Mode::QuickLook) {
        match app.current().cloned() {
            Some(e) => return views::quick_look(buf, area, app, &e, cx),
            None => app.mode = Mode::Normal,
        }
    }
    if matches!(app.mode, Mode::Diff { full: true }) {
        match app.current().cloned() {
            Some(e) if git::full(buf, area, app, &e, cx) => return,
            _ => app.mode = Mode::Normal,
        }
    }

    let top = Rect { height: 1, ..area };
    let path = Rect {
        y: area.bottom() - 2,
        height: 1,
        ..area
    };
    let hints_row = Rect {
        y: area.bottom() - 1,
        height: 1,
        ..area
    };
    let mut body = Rect {
        y: area.y + 1,
        height: area.height - 3,
        ..area
    };
    toolbar(buf, top, app, cx);

    if app.sidebar && body.width >= 72 {
        sidebar(
            buf,
            Rect {
                width: SIDEBAR_W,
                ..body
            },
            app,
            cx,
        );
        body.x += SIDEBAR_W;
        body.width -= SIDEBAR_W;
    }
    let pane = (app.preview && matches!(app.view, View::Icons | View::List) && body.width >= 64)
        .then(|| {
            let w = (body.width * 3 / 10).clamp(30, 44);
            body.width -= w;
            Rect {
                x: body.right(),
                width: w,
                ..body
            }
        });
    app.git.pane = pane.is_some() || app.view == View::Columns;

    if let Some(err) = &app.error {
        let msg = format!("Can’t open this folder: {err}");
        centered(buf, body, &msg, cx.th.dim);
    } else if app.visible.is_empty() {
        let msg = if app.is_loading() {
            "Loading"
        } else if !app.filter.is_empty() {
            "No matches"
        } else {
            "Empty folder"
        };
        if app.view == View::Columns {
            views::columns(buf, body, app, cx);
        } else {
            centered(buf, body, msg, cx.th.faint);
        }
    } else {
        match app.view {
            View::Icons => views::icons(buf, body, app, cx),
            View::List => views::list(buf, body, app, cx),
            View::Columns => views::columns(buf, body, app, cx),
            View::Gallery => views::gallery(buf, body, app, cx),
        }
    }
    if let Some(pane) = pane {
        views::pane(buf, pane, app, cx);
    }
    path_bar(buf, path, app, cx);
    hints(buf, hints_row, app, cx);
    if matches!(app.mode, Mode::Help) {
        help(buf, area, cx);
    }
}

fn toolbar(buf: &mut Buffer, area: Rect, app: &mut App, cx: &mut Ctx) {
    let th = cx.th;
    fill(buf, area, th.chrome);
    let y = area.y;
    let mut x = area.x + 2;
    let nav = |on: bool| if on { Style::new() } else { th.faint };
    text(buf, x, y, "‹", 1, nav(!app.back.is_empty()));
    app.hits.push((Rect::new(x - 1, y, 3, 1), Hit::Back));
    x += 3;
    text(buf, x, y, "›", 1, nav(!app.fwd.is_empty()));
    app.hits.push((Rect::new(x - 1, y, 3, 1), Hit::Forward));
    x += 4;

    // Right side first, so the title takes what is left.
    let mut right = area.right() - 1;
    let filtering = matches!(app.mode, Mode::Filter);
    let full: u16 = View::ALL.iter().map(|v| v.label().len() as u16 + 2).sum();
    let room = right.saturating_sub(x + 12);
    let show_filter = filtering || !app.filter.is_empty() || room >= full + 22;
    if show_filter {
        let w = 18.min(room);
        right -= w;
        filter_field(buf, Rect::new(right, y, w, 1), app, th);
        app.hits.push((Rect::new(right, y, w, 1), Hit::Filter));
        right = right.saturating_sub(2);
    }
    let short = right.saturating_sub(x + 12) < full;
    let labels: Vec<String> = View::ALL
        .iter()
        .map(|v| {
            let label = if short { &v.label()[..1] } else { v.label() };
            format!(" {label} ")
        })
        .collect();
    let switch_w: u16 = labels.iter().map(|l| l.len() as u16).sum();
    if right >= x + switch_w {
        let mut sx = right - switch_w;
        right = sx;
        for (view, label) in View::ALL.iter().zip(&labels) {
            let w = label.len() as u16;
            let style = if *view == app.view {
                th.sel.add_modifier(Modifier::BOLD)
            } else {
                th.dim
            };
            text(buf, sx, y, label, w, style);
            app.hits.push((Rect::new(sx, y, w, 1), Hit::View(*view)));
            sx += w;
        }
    }

    let title = display_name(app, &app.cwd);
    let max = right.saturating_sub(x + 2);
    text(
        buf,
        x,
        y,
        &fmt::ellipsize_end(&title, max as usize),
        max,
        Style::new().add_modifier(Modifier::BOLD),
    );
}

fn filter_field(buf: &mut Buffer, area: Rect, app: &App, th: &Theme) {
    fill(buf, area, th.soft.remove_modifier(Modifier::BOLD));
    let inner = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    };
    let filtering = matches!(app.mode, Mode::Filter);
    if app.filter.is_empty() && !filtering {
        text(buf, inner.x, inner.y, "/ Filter", inner.width, th.dim);
        return;
    }
    let chars: Vec<char> = app.filter.chars().collect();
    let style = if filtering { Style::new() } else { th.accent };
    edit_line(buf, inner, &chars, chars.len(), filtering, style);
}

fn sidebar(buf: &mut Buffer, area: Rect, app: &mut App, cx: &mut Ctx) {
    let th = cx.th;
    fill(buf, area, th.chrome);
    let mut y = area.y + 1;
    let sections = [
        ("Favorites", app.favorites.clone()),
        ("Locations", app.locations.clone()),
    ];
    for (title, places) in sections {
        if places.is_empty() || y >= area.bottom() {
            continue;
        }
        text(buf, area.x + 2, y, title, area.width - 3, th.dim);
        y += 1;
        for place in places {
            if y >= area.bottom() {
                break;
            }
            let row = Rect::new(area.x + 1, y, area.width - 2, 1);
            if place.path == app.cwd {
                fill(buf, row, th.soft);
            }
            let mut x = row.x + 1;
            if cx.icons == Icons::Nerd {
                text(buf, x, y, place_glyph(place.kind), 1, th.accent);
                x += 2;
            }
            let name = fmt::ellipsize_end(&place.name, (row.right() - x - 1) as usize);
            text(buf, x, y, &name, row.right() - x, Style::new());
            app.hits.push((row, Hit::Go(place.path.clone())));
            y += 1;
        }
        y += 1;
    }
}

fn place_glyph(kind: PlaceKind) -> &'static str {
    match kind {
        PlaceKind::Home => "\u{f015}",
        PlaceKind::Desktop => "\u{f108}",
        PlaceKind::Documents => "\u{f15c}",
        PlaceKind::Downloads => "\u{f019}",
        PlaceKind::Applications => "\u{f135}",
        PlaceKind::Pictures => "\u{f03e}",
        PlaceKind::Music => "\u{f001}",
        PlaceKind::Movies => "\u{f03d}",
        PlaceKind::Root | PlaceKind::Volume => "\u{f0a0}",
        PlaceKind::Trash => "\u{f1f8}",
    }
}

fn place<'a>(app: &'a App, path: &Path) -> Option<&'a Place> {
    app.favorites
        .iter()
        .chain(&app.locations)
        .find(|p| p.path == path)
}

/// What Finder calls a folder: its place name ("Macintosh HD"), else its own name.
pub fn display_name(app: &App, path: &Path) -> String {
    if let Some(p) = place(app, path)
        && p.kind != PlaceKind::Home
    {
        return p.name.clone();
    }
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn path_bar(buf: &mut Buffer, area: Rect, app: &mut App, cx: &mut Ctx) {
    let th = cx.th;
    fill(buf, area, th.chrome);
    let y = area.y;

    let n = app.visible.len();
    let mut status = if app.is_loading() && n == 0 {
        "Loading".to_string()
    } else if !app.marks.is_empty() {
        format!("{} of {n} selected", app.marks.len())
    } else if n == 1 {
        "1 item".to_string()
    } else {
        format!("{n} items")
    };
    if let Some(free) = app.free {
        status.push_str(&format!(", {} available", fmt::size(free)));
    }
    let sw = fmt::width(&status) as u16;
    let mut status_x = area.right().saturating_sub(sw + 2);
    text(buf, status_x, y, &status, sw, th.dim);
    if let Some(branch) = cx.git.as_deref().and_then(git::branch) {
        let branch = match cx.icons {
            Icons::Nerd => format!("\u{e0a0} {branch}"),
            _ => branch,
        };
        let bw = fmt::width(&branch) as u16;
        // Only when the path keeps some room.
        if status_x > area.x + bw + 24 {
            status_x -= bw + 3;
            text(buf, status_x, y, &branch, bw, Style::new());
        }
    }

    let mut crumbs = Vec::new();
    let mut p = Some(app.cwd.as_path());
    while let Some(dir) = p {
        crumbs.push((display_name(app, dir), dir.to_path_buf()));
        p = dir.parent();
    }
    crumbs.reverse();
    let sep = " › ";
    let room = status_x.saturating_sub(area.x + 4) as usize;
    let width = |cs: &[(String, std::path::PathBuf)]| {
        cs.iter().map(|(n, _)| fmt::width(n) + 3).sum::<usize>()
    };
    let mut start = 0;
    while start + 1 < crumbs.len() && width(&crumbs[start..]) + 4 > room {
        start += 1;
    }
    let mut x = area.x + 2;
    if start > 0 {
        x = text(buf, x, y, "…", 1, th.dim);
        x = text(buf, x, y, sep, 3, th.faint);
    }
    let last = crumbs.len() - 1;
    for (i, (name, path)) in crumbs.iter().enumerate().skip(start) {
        let max = (area.x + 2 + room as u16).saturating_sub(x);
        let name = fmt::ellipsize_end(name, max as usize);
        let w = fmt::width(&name) as u16;
        let style = if i == last { Style::new() } else { th.dim };
        text(buf, x, y, &name, w, style);
        app.hits
            .push((Rect::new(x, y, w, 1), Hit::Go(path.clone())));
        x += w;
        if i != last {
            x = text(buf, x, y, sep, 3, th.faint);
        }
    }
}

fn hints(buf: &mut Buffer, area: Rect, app: &App, cx: &mut Ctx) {
    let th = cx.th;
    let (x, y) = (area.x + 2, area.y);
    let mut w = area.width.saturating_sub(4);
    // A half-typed vim command, in the corner like vim's showcmd.
    let pending = app.keys.show();
    if !pending.is_empty() {
        text_right(buf, area.right() - 2, y, &pending, Style::new());
        w = w.saturating_sub(fmt::width(&pending) as u16 + 2);
    }
    match &app.mode {
        Mode::Confirm(paths) => {
            let what = match paths.as_slice() {
                [one] => format!(
                    "“{}”",
                    one.file_name().unwrap_or_default().to_string_lossy()
                ),
                many => format!("{} items", many.len()),
            };
            let x = text(
                buf,
                x,
                y,
                &format!("Move {what} to the Trash?  "),
                w,
                Style::new(),
            );
            keys(
                buf,
                x,
                y,
                area.right().saturating_sub(x),
                &[("y", "Move"), ("n", "Cancel")],
                th,
            );
        }
        Mode::Visual(_) => {
            let selected = format!("{} selected  ", app.marks.len());
            let bold = Style::new().add_modifier(Modifier::BOLD);
            let x = text(buf, x, y, &selected, w, bold);
            let pairs = [
                ("y", "Copy"),
                ("d", "Cut"),
                ("D", "Trash"),
                ("o", "Other end"),
                ("Esc", "Cancel"),
            ];
            keys(buf, x, y, (area.x + 2 + w).saturating_sub(x), &pairs, th);
        }
        Mode::Command(line) => {
            let x = text(buf, x, y, ":", 1, Style::new());
            let chars: Vec<char> = line.chars().collect();
            let field = Rect::new(x, y, (area.x + 2 + w).saturating_sub(x), 1);
            edit_line(buf, field, &chars, chars.len(), true, Style::new());
        }
        Mode::Rename(_) => keys(buf, x, y, w, &[("Enter", "Rename"), ("Esc", "Cancel")], th),
        Mode::Filter => keys(buf, x, y, w, &[("Enter", "Done"), ("Esc", "Clear")], th),
        Mode::Diff { .. } => keys(
            buf,
            x,
            y,
            w,
            &[
                ("j k", "Scroll"),
                ("n N", "Next, previous change"),
                ("h l", "Sideways"),
                ("Enter", "Full screen"),
                ("Esc", "Back"),
            ],
            th,
        ),
        _ => match &app.status {
            Some(s) => {
                let style = if s.error { th.error } else { Style::new() };
                text(
                    buf,
                    x,
                    y,
                    &fmt::ellipsize_end(&s.text, w as usize),
                    w,
                    style,
                );
            }
            None => keys(
                buf,
                x,
                y,
                w,
                &[
                    ("Enter", "Open"),
                    ("Space", "Quick Look"),
                    ("/", "Filter"),
                    ("yy", "Copy"),
                    ("dd", "Cut"),
                    ("p", "Paste"),
                    ("?", "Keys"),
                    ("q", "Quit"),
                ],
                th,
            ),
        },
    }
}

/// "key label  key label", dropping pairs that do not fit.
pub fn keys(buf: &mut Buffer, mut x: u16, y: u16, w: u16, pairs: &[(&str, &str)], th: &Theme) {
    let end = x + w;
    for (key, label) in pairs {
        let need = (fmt::width(key) + 1 + fmt::width(label)) as u16;
        if x + need > end {
            break;
        }
        x = text(
            buf,
            x,
            y,
            key,
            need,
            Style::new().add_modifier(Modifier::BOLD),
        );
        x = text(buf, x + 1, y, label, need, th.dim) + 2;
    }
}

const HELP: &[(&str, &str)] = &[
    ("h j k l", "Move"),
    ("Enter", "Open"),
    ("gx", "Open in default app"),
    ("-  Backspace", "Enclosing folder"),
    ("Ctrl-o  Ctrl-i", "Back, forward"),
    ("~", "Home"),
    ("gg  G", "First, last"),
    ("Ctrl-d  Ctrl-u", "Half page down, up"),
    ("Ctrl-f  Ctrl-b", "Page down, up"),
    ("H  M  L", "Top, middle, bottom"),
    ("fx  Fx  ;  ,", "Jump by letter"),
    ("/  n  N", "Filter, find again"),
    ("5j  3dd", "Repeat with a count"),
    ("yy  dd  p", "Copy, cut, paste"),
    ("yj  dG", "Copy, cut to a move"),
    ("v", "Select a range"),
    ("m", "Mark"),
    ("yp  yn", "Copy path, name"),
    ("r  I  A", "Rename"),
    ("cw  cc", "Replace name, all"),
    ("a", "New folder"),
    ("D  Delete", "Move to Trash"),
    ("Space", "Quick Look"),
    ("gd", "Focus the diff"),
    ("gD", "Full-screen diff"),
    ("zi  zl", "Icons, list"),
    ("zc  zg", "Columns, gallery"),
    ("zt  zz  zb", "Scroll top, mid, bottom"),
    ("zh  .", "Hidden files"),
    ("zp  zs", "Preview pane, sidebar"),
    ("s  S", "Sort by, reverse"),
    ("O", REVEAL),
    (":cd", "Go to a folder"),
    ("Esc", "Clear filter, marks"),
    ("q  ZZ  :q", "Quit"),
];

const REVEAL: &str = if cfg!(target_os = "macos") {
    "Show in Finder"
} else {
    "Show in file manager"
};

fn help(buf: &mut Buffer, area: Rect, cx: &mut Ctx) {
    const GAP: u16 = 3;
    let th = cx.th;
    let cols: u16 = if area.height >= HELP.len() as u16 + 4 {
        1
    } else {
        2
    };
    let per_col = HELP.len().div_ceil(cols as usize);
    // Natural widths, shrunk to fit a narrow window. Each column sizes its
    // keys on its own, so short keys leave more room for their labels.
    let key_ws: Vec<u16> = HELP
        .chunks(per_col)
        .map(|keys| keys.iter().map(|(k, _)| fmt::width(k)).max().unwrap_or(0) as u16 + 2)
        .collect();
    let natural = HELP
        .chunks(per_col)
        .zip(&key_ws)
        .map(|(keys, key_w)| {
            key_w + keys.iter().map(|(_, w)| fmt::width(w)).max().unwrap_or(0) as u16
        })
        .max()
        .unwrap_or(0);
    let room = area.width.saturating_sub(6 + GAP * (cols - 1)) / cols;
    let col_w = natural.min(room);
    let w = col_w * cols + GAP * (cols - 1) + 4;
    let h = (per_col as u16 + 4).min(area.height - 2);
    let rect = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    // A blank margin keeps the labels underneath off the border.
    Clear.render(rect.outer(Margin::new(2, 1)).intersection(area), buf);
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(th.faint)
        .title(" Keys ")
        .title_style(Style::new().add_modifier(Modifier::BOLD))
        .render(rect, buf);
    let inner = Rect::new(rect.x + 2, rect.y + 2, rect.width - 4, rect.height - 3);
    for (i, (key, what)) in HELP.iter().enumerate() {
        let (c, r) = (i / per_col, i % per_col);
        let key_w = key_ws[c].min(col_w / 2);
        let x = inner.x + c as u16 * (col_w + GAP);
        let y = inner.y + r as u16;
        if y >= inner.bottom() {
            continue;
        }
        let key = fmt::ellipsize_end(key, key_w.saturating_sub(1) as usize);
        let what = fmt::ellipsize_end(what, (col_w - key_w) as usize);
        text(
            buf,
            x,
            y,
            &key,
            key_w,
            Style::new().add_modifier(Modifier::BOLD),
        );
        text(buf, x + key_w, y, &what, col_w - key_w, th.dim);
    }
}

// Helpers shared by the views.

/// Writes `s` at (x, y), at most `max` columns. Returns the column after it.
pub fn text(buf: &mut Buffer, x: u16, y: u16, s: &str, max: u16, style: Style) -> u16 {
    if max == 0 || !buf.area.contains(Position::new(x, y)) {
        return x;
    }
    let max = max.min(buf.area.right() - x);
    buf.set_stringn(x, y, s, max as usize, style).0
}

/// Writes `s` so it ends just before column `right`.
pub fn text_right(buf: &mut Buffer, right: u16, y: u16, s: &str, style: Style) {
    let w = fmt::width(s) as u16;
    text(buf, right.saturating_sub(w), y, s, w, style);
}

pub fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    buf.set_style(area.intersection(buf.area), style);
}

pub fn vline(buf: &mut Buffer, x: u16, area: Rect, style: Style) {
    for y in area.top()..area.bottom() {
        text(buf, x, y, "│", 1, style);
    }
}

pub fn centered(buf: &mut Buffer, area: Rect, msg: &str, style: Style) {
    let msg = fmt::ellipsize_end(msg, area.width.saturating_sub(4) as usize);
    let w = fmt::width(&msg) as u16;
    let x = area.x + area.width.saturating_sub(w) / 2;
    text(buf, x, area.y + area.height / 2, &msg, w, style);
}

/// The text icon for an entry, without its trailing space.
pub fn glyph(e: &Entry, icons: Icons) -> &'static str {
    glyph::icon(e, icons).trim_end()
}

/// A glyph centered in `area`.
pub fn glyph_in(buf: &mut Buffer, area: Rect, e: &Entry, icons: Icons) {
    let g = glyph(e, icons);
    let w = fmt::width(g) as u16;
    let x = area.x + area.width.saturating_sub(w) / 2;
    text(buf, x, area.y + area.height / 2, g, w, Style::new());
}

/// A one-line text field with a block cursor, scrolled to keep the cursor in view.
pub fn edit_line(
    buf: &mut Buffer,
    area: Rect,
    text_: &[char],
    cursor: usize,
    show_cursor: bool,
    style: Style,
) {
    use unicode_width::UnicodeWidthChar;
    let w = area.width as usize;
    if w == 0 {
        return;
    }
    let width = |cs: &[char]| cs.iter().map(|c| c.width().unwrap_or(0)).sum::<usize>();
    let mut start = 0;
    while start < cursor && width(&text_[start..cursor]) >= w {
        start += 1;
    }
    let shown: String = text_[start..].iter().collect();
    text(buf, area.x, area.y, &shown, area.width, style);
    if show_cursor {
        let cx = area.x + width(&text_[start..cursor]) as u16;
        if let Some(cell) = buf.cell_mut((cx, area.y)) {
            cell.modifier.insert(Modifier::REVERSED);
        }
    }
}

/// The rename field, over a name.
/// The inline rename field, with a column of padding on each side.
pub fn rename_field(buf: &mut Buffer, area: Rect, r: &Rename, th: &Theme) {
    // It may sit over other labels, so blank it first.
    Clear.render(area.intersection(buf.area), buf);
    fill(buf, area, th.soft.remove_modifier(Modifier::BOLD));
    let inner = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    };
    edit_line(buf, inner, &r.text, r.cursor, true, Style::new());
}
