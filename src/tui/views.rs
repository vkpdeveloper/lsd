//! The four views (Icons, List, Columns, Gallery), the preview pane and Quick
//! Look. Only visible rows are drawn, and thumbnails are requested for those
//! plus about a page ahead.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Clear, Widget};

use super::app::{self, App, Hit, Mode};
use super::git;
use super::theme::Theme;
use super::ui::{
    Ctx, centered, fill, glyph, glyph_in, keys, rename_field, text, text_right, vline,
};
use super::work::{Lookup, PRIO_PREVIEW, PRIO_VISIBLE};
use crate::doc::Line;
use crate::entry::{Entry, SortKey};
use crate::fmt;
use crate::meta;
use crate::thumb::{self, Mode as ThumbMode, Thumb};

/// Icon grid geometry: columns spread across the width like Finder's.
#[derive(Debug, PartialEq, Eq)]
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    pub tile_w: u16,
}

pub fn grid(width: u16, height: u16, min_tile_w: u16, tile_h: u16) -> Grid {
    let cols = (width / min_tile_w.max(1)).max(1);
    Grid {
        cols: cols as usize,
        rows: (height / tile_h.max(1)).max(1) as usize,
        tile_w: (width / cols).max(1),
    }
}

pub fn icons(buf: &mut Buffer, area: Rect, app: &mut App, cx: &mut Ctx) {
    let images = cx.pics.images();
    let (box_w, box_h) = if images {
        (10, cx.pics.square_rows(10))
    } else {
        (2, 1)
    };
    let tile_h = box_h + 3;
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(1),
    };
    let g = grid(
        inner.width,
        inner.height,
        if images { 18 } else { 16 },
        tile_h,
    );
    app.grid_cols = g.cols;
    app.page = g.cols * g.rows;
    let n = app.visible.len();
    let total_rows = n.div_ceil(g.cols);
    app.scroll = if app.follow {
        app::follow(app.scroll, app.sel / g.cols, g.rows, total_rows)
    } else {
        app.scroll.min(total_rows.saturating_sub(g.rows))
    };
    app.follow = false;

    let list = app.list.clone();
    let px = cx.pics.px((box_w, box_h));
    let first = app.scroll * g.cols;
    let end = (first + g.cols * g.rows).min(n);
    let mut field = None;
    for i in first..end {
        let e = &list.entries[app.visible[i]];
        let (row, col) = ((i - first) / g.cols, (i - first) % g.cols);
        let tile = Rect::new(
            inner.x + col as u16 * g.tile_w,
            inner.y + row as u16 * tile_h,
            g.tile_w,
            tile_h,
        );
        let selected = i == app.sel;
        let marked = app.marks.contains(&e.path);
        let th = cx.th;

        let icon = Rect::new(tile.x + (tile.width - box_w) / 2, tile.y, box_w, box_h);
        tile_icon(buf, cx, icon, e, selected, px);
        let look = git::look(cx.git.as_deref(), &e.path);
        if let Some((badge, style)) = look.badge {
            text(buf, icon.right() + 1, icon.bottom() - 1, badge, 1, style);
        }

        let label_w = tile.width - 2;
        let y = tile.y + box_h;
        if let Mode::Rename(r) = &app.mode
            && r.path == e.path
        {
            // Like Finder, the field grows past the tile to fit a long name.
            // It is drawn last so the next tiles do not cover it.
            let w = (edit_width(&r.text) + 3).clamp(label_w, inner.width);
            let x = (tile.x + tile.width / 2)
                .saturating_sub(w / 2)
                .clamp(inner.x, inner.right() - w);
            field = Some(Rect::new(x, y, w, 1));
        } else {
            for (k, line) in fmt::label_lines(&e.name, label_w as usize - 2)
                .iter()
                .enumerate()
            {
                let w = fmt::width(line) as u16;
                let x = tile.x + (tile.width - w) / 2;
                let y = y + k as u16;
                let style = if selected {
                    fill(buf, Rect::new(x - 1, y, w + 2, 1), th.sel);
                    th.sel
                } else if marked {
                    th.accent
                } else if look.dim {
                    th.dim
                } else {
                    Style::new()
                };
                text(buf, x, y, line, w, style);
            }
        }
        app.hits.push((tile, Hit::Item(i)));
    }
    if let (Some(rect), Mode::Rename(r)) = (field, &app.mode) {
        rename_field(buf, rect, r, cx.th);
    }
    if images {
        for i in end..(end + g.cols * g.rows).min(n) {
            cx.load
                .prefetch(&list.entries[app.visible[i]], px, ThumbMode::Icon);
        }
    }
}

fn edit_width(text: &[char]) -> u16 {
    use unicode_width::UnicodeWidthChar;
    text.iter().map(|c| c.width().unwrap_or(0) as u16).sum()
}

/// An item's picture in an icon box, standing on the label below it like in
/// Finder. A selected item gets a soft surface that hugs the picture.
fn tile_icon(buf: &mut Buffer, cx: &mut Ctx, icon: Rect, e: &Entry, selected: bool, px: u32) {
    let thumb = if cx.pics.images() {
        cx.load.thumb(e, px, ThumbMode::Icon, PRIO_VISIBLE)
    } else {
        Lookup::Failed
    };
    let shape = match &thumb {
        Lookup::Ready(t) => {
            let fit = cx.pics.fit(icon, t);
            Rect {
                y: icon.bottom() - fit.height,
                ..fit
            }
        }
        // A glyph sits in a small box of its own, on the same baseline.
        _ => {
            let (w, h) = (icon.width.min(4), icon.height.min(3));
            Rect::new(icon.x + (icon.width - w) / 2, icon.bottom() - h, w, h)
        }
    };
    let bg = if selected {
        let th = cx.th;
        fill(
            buf,
            Rect {
                x: shape.x - 1,
                width: shape.width + 2,
                ..shape
            },
            th.soft,
        );
        Theme::bg_of(th.soft)
    } else {
        Color::Reset
    };
    match thumb {
        Lookup::Ready(t) if cx.pics.draw(buf, cx.load, shape, &t, bg) => {}
        Lookup::Pending => {}
        _ => glyph_in(buf, shape, e, cx.icons),
    }
}

/// Widths of the list view columns; 0 hides one. Narrow windows drop Kind,
/// then Size, then Date Modified.
#[derive(Debug, PartialEq, Eq)]
pub struct ListCols {
    pub name: u16,
    pub date: u16,
    pub size: u16,
    pub kind: u16,
}

const GAP: u16 = 2;

pub fn list_cols(width: u16) -> ListCols {
    let mut c = ListCols {
        name: 0,
        date: 21,
        size: 10,
        kind: 18,
    };
    let used = |c: &ListCols| {
        [c.date, c.size, c.kind]
            .iter()
            .filter(|&&w| w > 0)
            .map(|w| w + GAP)
            .sum::<u16>()
    };
    for drop in [2, 1, 0] {
        if width.saturating_sub(used(&c)) >= 24 {
            break;
        }
        match drop {
            2 => c.kind = 0,
            0 => c.date = 0,
            _ => c.size = 0,
        }
    }
    c.name = width.saturating_sub(used(&c));
    c
}

pub fn list(buf: &mut Buffer, area: Rect, app: &mut App, cx: &mut Ctx) {
    let th = cx.th;
    // Mark, icon, gap, then the columns, with one column of padding on the right.
    let x0 = area.x + 5;
    let cols = list_cols(area.width.saturating_sub(6));
    let date_x = x0 + cols.name + GAP;
    let size_x = date_x + if cols.date > 0 { cols.date + GAP } else { 0 };
    let kind_x = size_x + if cols.size > 0 { cols.size + GAP } else { 0 };

    // Header.
    let arrow = |key: SortKey| {
        if app.sort.key != key {
            return "";
        }
        // Name and Kind run A to Z, Date and Size newest and largest first.
        let descending = matches!(key, SortKey::Date | SortKey::Size) != app.sort.reverse;
        if descending { " ▾" } else { " ▴" }
    };
    let y = area.y;
    let head = [
        ("Name", SortKey::Name, x0, cols.name, false),
        ("Date Modified", SortKey::Date, date_x, cols.date, false),
        ("Size", SortKey::Size, size_x, cols.size, true),
        ("Kind", SortKey::Kind, kind_x, cols.kind, false),
    ];
    for (label, key, x, w, right) in head {
        if w == 0 {
            continue;
        }
        let s = format!("{label}{}", arrow(key));
        if right {
            text_right(buf, x + w, y, &s, th.dim);
        } else {
            text(buf, x, y, &s, w, th.dim);
        }
        app.hits.push((Rect::new(x, y, w, 1), Hit::Sort(key)));
    }

    let body = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    let page = body.height as usize;
    let n = app.visible.len();
    app.page = page;
    app.scroll = if app.follow {
        app::follow(app.scroll, app.sel, page, n)
    } else {
        app.scroll.min(n.saturating_sub(page))
    };
    app.follow = false;

    let list = app.list.clone();
    let images = cx.pics.images();
    let px = cx.pics.px((2, 1));
    for r in 0..page {
        let i = app.scroll + r;
        let row = Rect::new(area.x, body.y + r as u16, area.width, 1);
        if i % 2 == 1 {
            fill(buf, row, th.stripe);
        }
        if i >= n {
            continue;
        }
        let e = &list.entries[app.visible[i]];
        let selected = i == app.sel;
        if selected {
            fill(buf, row, th.sel);
        }
        let base = if selected { th.sel } else { Style::new() };
        let dim = if selected { th.sel } else { th.dim };
        let y = row.y;
        if app.marks.contains(&e.path) {
            text(
                buf,
                area.x + 1,
                y,
                "●",
                1,
                if selected { th.sel } else { th.accent },
            );
        }
        let bg = Theme::bg_of(if selected {
            th.sel
        } else if i % 2 == 1 {
            th.stripe
        } else {
            Style::new()
        });
        small_icon(buf, cx, area.x + 2, y, e, bg, PRIO_VISIBLE);

        match &app.mode {
            Mode::Rename(rn) if rn.path == e.path => {
                rename_field(buf, Rect::new(x0 - 1, y, cols.name + 2, 1), rn, th);
            }
            _ => {
                let look = git::look(cx.git.as_deref(), &e.path);
                let room = cols.name - if look.badge.is_some() { 2 } else { 0 };
                let name = fmt::ellipsize_middle(&e.name, room as usize);
                let style = if look.dim && !selected { th.dim } else { base };
                text(buf, x0, y, &name, room, style);
                if let Some((badge, style)) = look.badge {
                    let style = if selected { th.sel } else { style };
                    text(buf, x0 + cols.name - 1, y, badge, 1, style);
                }
            }
        }
        if cols.date > 0 {
            let date = e.modified.map(fmt::date).unwrap_or_else(|| "--".into());
            text(buf, date_x, y, &date, cols.date, dim);
        }
        if cols.size > 0 {
            let size = if e.is_dir {
                "--".into()
            } else {
                fmt::size(e.size)
            };
            text_right(buf, size_x + cols.size, y, &size, dim);
        }
        if cols.kind > 0 {
            let kind = fmt::ellipsize_end(&meta::kind(e), cols.kind as usize);
            text(buf, kind_x, y, &kind, cols.kind, dim);
        }
        app.hits.push((row, Hit::Item(i)));
    }
    if images {
        let end = (app.scroll + page).min(n);
        for i in end..(end + page).min(n) {
            cx.load
                .prefetch(&list.entries[app.visible[i]], px, ThumbMode::Icon);
        }
    }
}

/// A one-row icon two columns wide: a tiny thumbnail, or a glyph.
fn small_icon(buf: &mut Buffer, cx: &mut Ctx, x: u16, y: u16, e: &Entry, bg: Color, prio: u8) {
    if cx.pics.images() {
        let px = cx.pics.px((2, 1));
        match cx.load.thumb(e, px, ThumbMode::Icon, prio) {
            Lookup::Ready(t) => {
                let rect = cx.pics.fit(Rect::new(x, y, 2, 1), &t);
                if cx.pics.draw(buf, cx.load, rect, &t, bg) {
                    return;
                }
            }
            Lookup::Pending => return,
            Lookup::Failed => {}
        }
    }
    text(buf, x, y, glyph(e, cx.icons), 2, Style::new());
}

/// Miller columns: the enclosing folder, this folder, then the selected
/// folder's contents or the selected file's preview.
pub fn columns(buf: &mut Buffer, area: Rect, app: &mut App, cx: &mut Ctx) {
    let th = cx.th;
    let parent = app.cwd.parent().map(|p| p.to_path_buf());
    let show_parent = area.width >= 70 && parent.is_some();
    let pw = if show_parent {
        (area.width / 4).clamp(18, 34)
    } else {
        0
    };
    let cw = (area.width * 3 / 10).clamp(22, 44).min(area.width - pw);
    let parent_rect = Rect { width: pw, ..area };
    let cur = Rect {
        x: area.x + pw + u16::from(show_parent),
        width: cw,
        ..area
    };
    let child = Rect {
        x: cur.right() + 1,
        width: area.right().saturating_sub(cur.right() + 1),
        ..area
    };

    if let (true, Some(parent)) = (show_parent, &parent) {
        if let Some(list) = app.dir(parent) {
            let idx = app::visible(&list.entries, app.hidden, "");
            let at = idx.iter().position(|&i| list.entries[i].path == app.cwd);
            let rows: Vec<&Entry> = idx.iter().map(|&i| &list.entries[i]).collect();
            column(buf, cx, parent_rect, &rows, at, th.soft, &mut app.hits);
        }
        vline(buf, parent_rect.right(), area, th.faint);
    }

    // This folder.
    let page = cur.height as usize;
    let n = app.visible.len();
    app.page = page;
    app.scroll = if app.follow {
        app::follow(app.scroll, app.sel, page, n)
    } else {
        app.scroll.min(n.saturating_sub(page))
    };
    app.follow = false;
    let list = app.list.clone();
    for r in 0..page.min(n.saturating_sub(app.scroll)) {
        let i = app.scroll + r;
        let e = &list.entries[app.visible[i]];
        let row = Rect::new(cur.x, cur.y + r as u16, cur.width, 1);
        let style = if i == app.sel { th.sel } else { Style::new() };
        match &app.mode {
            Mode::Rename(rn) if rn.path == e.path => {
                column_row(buf, cx, row, e, style, app.marks.contains(&e.path));
                rename_field(buf, Rect::new(row.x + 3, row.y, row.width - 3, 1), rn, th);
            }
            _ => column_row(buf, cx, row, e, style, app.marks.contains(&e.path)),
        }
        app.hits.push((row, Hit::Item(i)));
    }
    if n == 0 {
        let msg = if app.is_loading() {
            "Loading"
        } else {
            "Empty folder"
        };
        centered(buf, cur, msg, th.faint);
    }
    vline(buf, cur.right(), area, th.faint);

    // The selection.
    let Some(e) = app.current().cloned() else {
        return;
    };
    if e.is_dir && !e.name.ends_with(".app") {
        if let Some(list) = app.dir(&e.path) {
            let idx = app::visible(&list.entries, app.hidden, "");
            let rows: Vec<&Entry> = idx.iter().map(|&i| &list.entries[i]).collect();
            if rows.is_empty() {
                centered(buf, child, "Empty folder", th.faint);
            }
            column(buf, cx, child, &rows, None, th.soft, &mut app.hits);
        }
    } else {
        let inner = Rect {
            x: child.x + 2,
            y: child.y + 1,
            width: child.width.saturating_sub(4),
            height: child.height.saturating_sub(1),
        };
        if !git::preview(buf, inner, app, &e, cx) {
            details(buf, inner, &e, cx);
        }
    }
}

/// A read-only column of another folder. Clicking an item goes there.
fn column(
    buf: &mut Buffer,
    cx: &mut Ctx,
    area: Rect,
    rows: &[&Entry],
    highlight: Option<usize>,
    style: Style,
    hits: &mut Vec<(Rect, Hit)>,
) {
    let page = area.height as usize;
    let scroll = match highlight {
        Some(h) if h >= page => h + 1 - page / 2,
        _ => 0,
    }
    .min(rows.len().saturating_sub(page));
    for (r, e) in rows.iter().skip(scroll).take(page).enumerate() {
        let row = Rect::new(area.x, area.y + r as u16, area.width, 1);
        let s = if highlight == Some(scroll + r) {
            style
        } else {
            Style::new()
        };
        column_row(buf, cx, row, e, s, false);
        hits.push((row, Hit::Reveal(e.path.clone())));
    }
}

fn column_row(buf: &mut Buffer, cx: &mut Ctx, row: Rect, e: &Entry, style: Style, marked: bool) {
    let th = cx.th;
    fill(buf, row, style);
    let bg = Theme::bg_of(style);
    small_icon(buf, cx, row.x + 1, row.y, e, bg, PRIO_VISIBLE);
    let folder = e.is_dir && !e.name.ends_with(".app");
    let look = git::look(cx.git.as_deref(), &e.path);
    // A folder's mark sits before its arrow.
    let badge_w = if look.badge.is_some() && folder { 2 } else { 0 };
    let name_w = row.width.saturating_sub(7 + badge_w);
    let name = fmt::ellipsize_middle(&e.name, name_w as usize);
    let name_style = if style != Style::new() {
        style
    } else if marked {
        th.accent
    } else if look.dim {
        th.dim
    } else {
        Style::new()
    };
    text(buf, row.x + 4, row.y, &name, name_w, name_style);
    if let Some((badge, badge_style)) = look.badge {
        let s = if style != Style::new() {
            style
        } else {
            badge_style
        };
        let x = row.right().saturating_sub(2 + badge_w);
        text(buf, x, row.y, badge, 1, s);
    }
    if folder {
        let arrow = if style != Style::new() { style } else { th.dim };
        text(buf, row.right().saturating_sub(2), row.y, "›", 1, arrow);
    }
}

/// A big preview with a strip of thumbnails below it.
pub fn gallery(buf: &mut Buffer, area: Rect, app: &mut App, cx: &mut Ctx) {
    let th = cx.th;
    let images = cx.pics.images();
    let (box_w, box_h) = if images {
        (8, cx.pics.square_rows(8))
    } else {
        (2, 1)
    };
    let tile_w = (box_w + 6).max(14);
    let strip_h = box_h + 2;
    let strip = Rect {
        y: area.bottom().saturating_sub(strip_h + 1),
        height: strip_h,
        ..area
    };
    let big = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: strip.y.saturating_sub(area.y + 4),
    };

    if let Some(e) = app.current().cloned() {
        // The name sits right under the picture, which is centered above it.
        let y = match picture(buf, cx, big, &e, false) {
            Some(drawn) => drawn.bottom() + 1,
            None => big.bottom() + 1,
        };
        match &app.mode {
            Mode::Rename(rn) if rn.path == e.path => {
                let w = 40.min(area.width - 4);
                rename_field(
                    buf,
                    Rect::new(area.x + (area.width - w) / 2, y, w, 1),
                    rn,
                    th,
                );
            }
            _ => {
                let name = fmt::ellipsize_middle(&e.name, area.width as usize - 4);
                let w = fmt::width(&name) as u16;
                text(
                    buf,
                    area.x + (area.width - w) / 2,
                    y,
                    &name,
                    w,
                    Style::new().add_modifier(Modifier::BOLD),
                );
            }
        }
    }

    let n = app.visible.len();
    let count = ((area.width / tile_w) as usize).max(1);
    app.page = count;
    let first = app
        .sel
        .saturating_sub(count / 2)
        .min(n.saturating_sub(count));
    let used = (count.min(n) as u16) * tile_w;
    let x0 = area.x + area.width.saturating_sub(used) / 2;
    let px = cx.pics.px((box_w, box_h));
    let list = app.list.clone();
    for (k, i) in (first..(first + count).min(n)).enumerate() {
        let e = &list.entries[app.visible[i]];
        let tile = Rect::new(x0 + k as u16 * tile_w, strip.y, tile_w, strip_h);
        let icon = Rect::new(tile.x + (tile_w - box_w) / 2, tile.y, box_w, box_h);
        tile_icon(buf, cx, icon, e, i == app.sel, px);
        let look = git::look(cx.git.as_deref(), &e.path);
        if let Some((badge, style)) = look.badge {
            text(buf, icon.right() + 1, icon.bottom() - 1, badge, 1, style);
        }
        let label = fmt::ellipsize_middle(&e.name, tile_w as usize - 2);
        let w = fmt::width(&label) as u16;
        let x = tile.x + (tile_w - w) / 2;
        let y = icon.bottom();
        let style = if i == app.sel {
            th.sel
        } else if look.dim {
            th.faint
        } else {
            th.dim
        };
        text(buf, x, y, &label, w, style);
        app.hits.push((tile, Hit::Item(i)));
    }
}

/// The preview pane beside Icons and List.
pub fn pane(buf: &mut Buffer, area: Rect, app: &mut App, cx: &mut Ctx) {
    vline(buf, area.x, area, cx.th.faint);
    let Some(e) = app.current().cloned() else {
        return;
    };
    let inner = Rect {
        x: area.x + 3,
        y: area.y + 1,
        width: area.width.saturating_sub(5),
        height: area.height.saturating_sub(1),
    };
    if !git::preview(buf, inner, app, &e, cx) {
        details(buf, inner, &e, cx);
    }
}

/// Picture, name, "Kind - Size", then Information, like Finder's preview pane.
fn details(buf: &mut Buffer, area: Rect, e: &Entry, cx: &mut Ctx) {
    let th = cx.th;
    if area.width < 8 || area.height < 4 {
        return;
    }
    let pic_h = cx
        .pics
        .square_rows(area.width)
        .min(area.height * 2 / 5)
        .max(3);
    let pic = Rect {
        height: pic_h,
        ..area
    };
    if !page(buf, cx, pic, e, true) {
        match cx.load.text(e, pic.width, pic.height) {
            Some(lines) => draw_lines(buf, pic, &lines),
            None => {
                picture(buf, cx, pic, e, true);
            }
        }
    }

    let mut y = area.y + pic_h + 1;
    let bottom = area.bottom();
    let w = area.width as usize;
    for line in fmt::label_lines(&e.name, w) {
        text(
            buf,
            area.x,
            y,
            &line,
            area.width,
            Style::new().add_modifier(Modifier::BOLD),
        );
        y += 1;
    }
    let info = cx.load.info(e);
    let kind = info
        .as_ref()
        .map(|i| i.kind.clone())
        .unwrap_or_else(|| meta::kind(e));
    let sub = if e.is_dir {
        match info.as_ref().and_then(|i| i.items) {
            Some(n) => format!("{kind} - {}", items(n)),
            None => kind,
        }
    } else {
        format!("{kind} - {}", fmt::size(e.size))
    };
    text(
        buf,
        area.x,
        y,
        &fmt::ellipsize_end(&sub, w),
        area.width,
        th.dim,
    );
    y += 2;
    if y >= bottom {
        return;
    }
    text(
        buf,
        area.x,
        y,
        "Information",
        area.width,
        Style::new().add_modifier(Modifier::BOLD),
    );
    y += 1;

    let mut rows: Vec<(&str, String)> = Vec::new();
    if let Some(t) = e.created {
        rows.push(("Created", fmt::date(t)));
    }
    if let Some(t) = e.modified {
        rows.push(("Modified", fmt::date(t)));
    }
    if let Some(info) = &info {
        if let Some(t) = info.last_opened {
            rows.push(("Last opened", fmt::date(t)));
        }
        if let Some((w, h)) = info.dimensions {
            rows.push(("Dimensions", format!("{w} × {h}")));
        }
        if let Some(p) = info.pages {
            rows.push(("Pages", p.to_string()));
        }
        if let Some(d) = info.duration {
            rows.push(("Duration", fmt::duration(d)));
        }
        if e.is_dir
            && let Some(n) = info.items
        {
            rows.push(("Items", n.to_string()));
        }
    }
    for (label, value) in rows {
        if y >= bottom {
            break;
        }
        let lw = fmt::width(label) as u16;
        text(buf, area.x, y, label, area.width, th.dim);
        let room = area.width.saturating_sub(lw + 1) as usize;
        text_right(
            buf,
            area.right(),
            y,
            &fmt::ellipsize_end(&value, room),
            Style::new(),
        );
        y += 1;
    }
}

/// Styled lines from the top of `area`, cut at its edges.
fn draw_lines(buf: &mut Buffer, area: Rect, lines: &[Line]) {
    for (line, y) in lines.iter().zip(area.top()..area.bottom()) {
        let mut x = area.x;
        for span in line {
            x = text(
                buf,
                x,
                y,
                &span.text,
                area.right().saturating_sub(x),
                span.style,
            );
        }
    }
}

fn items(n: usize) -> String {
    if n == 1 {
        "1 item".into()
    } else {
        format!("{n} items")
    }
}

/// The large picture of an entry: its content when the thumbnailer can render
/// it, else its big icon, else a glyph. Blank while loading. With `bottom`, it
/// sits on the bottom edge of `area`, else in the middle. Returns the cells it
/// covers once drawn.
fn picture(buf: &mut Buffer, cx: &mut Ctx, area: Rect, e: &Entry, bottom: bool) -> Option<Rect> {
    if area.is_empty() {
        return None;
    }
    let px = cx.pics.px((area.width, area.height));
    let thumb = match cx.load.thumb(e, px, ThumbMode::Preview, PRIO_PREVIEW) {
        Lookup::Ready(t) => Some(t),
        Lookup::Pending => return None,
        Lookup::Failed => match cx.load.thumb(e, px, ThumbMode::Icon, PRIO_PREVIEW) {
            Lookup::Ready(t) => Some(t),
            Lookup::Pending => return None,
            Lookup::Failed => None,
        },
    };
    match thumb {
        Some(t) => place(buf, cx, area, &t, bottom),
        None => {
            let rect = Rect {
                y: if bottom {
                    area.bottom() - 1
                } else {
                    area.y + area.height / 2
                },
                height: 1,
                ..area
            };
            glyph_in(buf, rect, e, cx.icons);
            Some(rect)
        }
    }
}

/// Draws `t` as large as fits in `area`, on its bottom edge or in the middle.
/// Returns the cells it covers, or None when it could not be drawn yet.
fn place(buf: &mut Buffer, cx: &mut Ctx, area: Rect, t: &Thumb, bottom: bool) -> Option<Rect> {
    let rect = cx.pics.fit(area, t);
    let rect = if bottom {
        Rect {
            y: area.bottom() - rect.height,
            ..rect
        }
    } else {
        rect
    };
    cx.pics
        .draw(buf, cx.load, rect, t, Color::Reset)
        .then_some(rect)
}

/// A web page as the page itself, once it has been rendered. Until then, or
/// when it cannot be, this draws nothing and returns false, and the caller
/// shows the source as usual. Half blocks are too coarse to read a page, so
/// that needs real images.
fn page(buf: &mut Buffer, cx: &mut Ctx, area: Rect, e: &Entry, bottom: bool) -> bool {
    if area.is_empty() || !cx.pics.images() || !thumb::page(e) {
        return false;
    }
    let px = cx.pics.px((area.width, area.height));
    match cx.load.thumb(e, px, ThumbMode::Preview, PRIO_PREVIEW) {
        Lookup::Ready(t) => place(buf, cx, area, &t, bottom).is_some(),
        Lookup::Pending | Lookup::Failed => false,
    }
}

/// Full-window preview. Arrows move between files.
pub fn quick_look(buf: &mut Buffer, area: Rect, app: &mut App, e: &Entry, cx: &mut Ctx) {
    let th = cx.th;
    Clear.render(area, buf);
    let head = Rect { height: 1, ..area };
    let foot = Rect {
        y: area.bottom() - 1,
        height: 1,
        ..area
    };
    fill(buf, head, th.chrome);
    fill(buf, foot, th.chrome);

    let info = cx.load.info(e);
    let mut facts = vec![
        info.as_ref()
            .map(|i| i.kind.clone())
            .unwrap_or_else(|| meta::kind(e)),
    ];
    if !e.is_dir {
        facts[0] = format!("{} - {}", facts[0], fmt::size(e.size));
    }
    if let Some(info) = &info {
        if let Some((w, h)) = info.dimensions {
            facts.push(format!("{w} × {h}"));
        }
        if let Some(d) = info.duration {
            facts.push(fmt::duration(d));
        }
        if let Some(p) = info.pages {
            facts.push(if p == 1 {
                "1 page".into()
            } else {
                format!("{p} pages")
            });
        }
        if e.is_dir
            && let Some(n) = info.items
        {
            facts.push(items(n));
        }
    }
    let facts = facts.join(" · ");
    let fw = (fmt::width(&facts) as u16).min(area.width / 2);
    text_right(
        buf,
        area.right() - 2,
        head.y,
        &fmt::ellipsize_end(&facts, fw as usize),
        th.dim,
    );
    let name_w = area.width.saturating_sub(fw + 6);
    let name = fmt::ellipsize_middle(&e.name, name_w as usize);
    text(
        buf,
        area.x + 2,
        head.y,
        &name,
        name_w,
        Style::new().add_modifier(Modifier::BOLD),
    );

    let pairs = [
        ("↑↓", "prev/next"),
        ("Enter", "open"),
        ("Space/Esc", "close"),
    ];
    let w: u16 = pairs
        .iter()
        .map(|(k, l)| (fmt::width(k) + fmt::width(l) + 3) as u16)
        .sum::<u16>()
        - 2;
    keys(
        buf,
        area.x + area.width.saturating_sub(w) / 2,
        foot.y,
        w,
        &pairs,
        th,
    );

    let pic = Rect {
        x: area.x + 2,
        y: area.y + 2,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(4),
    };
    if !page(buf, cx, pic, e, false) {
        match cx.load.text(e, pic.width, pic.height) {
            Some(lines) => draw_lines(buf, pic, &lines),
            None => {
                picture(buf, cx, pic, e, false);
            }
        }
    }
    // Neighbours first, so arrowing through a folder feels instant.
    let px = cx.pics.px((pic.width, pic.height));
    for i in [app.sel + 1, app.sel.wrapping_sub(1)] {
        if let Some(next) = app.entry(i).cloned() {
            cx.load.prefetch(&next, px, ThumbMode::Preview);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_spreads_tiles_across_the_width() {
        assert_eq!(
            grid(100, 40, 18, 8),
            Grid {
                cols: 5,
                rows: 5,
                tile_w: 20
            }
        );
        // Never zero columns or rows, however small.
        assert_eq!(
            grid(10, 3, 18, 8),
            Grid {
                cols: 1,
                rows: 1,
                tile_w: 10
            }
        );
    }

    #[test]
    fn list_drops_columns_when_narrow() {
        let wide = list_cols(120);
        assert_eq!((wide.date, wide.size, wide.kind), (21, 10, 18));
        assert_eq!(wide.name, 120 - 21 - 10 - 18 - 3 * GAP);
        let mid = list_cols(70);
        assert_eq!((mid.date, mid.size, mid.kind), (21, 10, 0));
        let narrow = list_cols(54);
        assert_eq!((narrow.date, narrow.size, narrow.kind), (21, 0, 0));
        assert!(narrow.name >= 24);
        let tiny = list_cols(30);
        assert_eq!((tiny.date, tiny.size, tiny.kind, tiny.name), (0, 0, 0, 30));
    }
}
