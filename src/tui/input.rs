//! Keys and mouse, with Finder semantics and vim-friendly letters.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;

use super::app::{App, Hit, Mode, View};

/// Last left click, for double clicks.
#[derive(Default)]
pub struct Clicks {
    last: Option<(Instant, Hit)>,
}

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Applies one input event. Returns true when the screen may have changed.
pub fn handle(app: &mut App, clicks: &mut Clicks, event: Event) -> bool {
    match event {
        Event::Key(k) if k.kind != KeyEventKind::Release => {
            key(app, k);
            true
        }
        Event::Mouse(m) => mouse(app, clicks, m),
        Event::Resize(..) => true,
        _ => false,
    }
}

fn key(app: &mut App, k: KeyEvent) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && k.code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }
    // A message stays until the next key.
    app.status = None;
    match &mut app.mode {
        Mode::Normal => normal(app, k),
        Mode::Filter => filter(app, k),
        Mode::Rename(_) => rename(app, k),
        Mode::Confirm(paths) => {
            if matches!(k.code, KeyCode::Char('y') | KeyCode::Enter) {
                let paths = std::mem::take(paths);
                app.trash(paths);
            } else {
                app.mode = Mode::Normal;
            }
        }
        Mode::Help => app.mode = Mode::Normal,
        Mode::QuickLook => quick_look(app, k),
    }
}

fn normal(app: &mut App, k: KeyEvent) {
    let grid = app.view == View::Icons;
    let columns = app.view == View::Columns;
    let page = app.page.max(1) as isize;
    match k.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Up | KeyCode::Char('k') if grid => app.move_rows(-1),
        KeyCode::Down | KeyCode::Char('j') if grid => app.move_rows(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_by(-1),
        KeyCode::Down | KeyCode::Char('j') => app.move_by(1),
        KeyCode::Left | KeyCode::Char('h') if columns => app.parent(),
        KeyCode::Right | KeyCode::Char('l') if columns => {
            if app.current().is_some_and(|e| e.is_dir) {
                app.open();
            }
        }
        KeyCode::Left | KeyCode::Char('h') if app.view != View::List => app.move_by(-1),
        KeyCode::Right | KeyCode::Char('l') if app.view != View::List => app.move_by(1),
        KeyCode::Enter => app.open(),
        KeyCode::Backspace => app.parent(),
        KeyCode::Char('[') => app.go_back(),
        KeyCode::Char(']') => app.go_forward(),
        KeyCode::Char('~') => app.home(),
        KeyCode::Char('g') | KeyCode::Home => app.select(0),
        KeyCode::Char('G') | KeyCode::End => app.select(usize::MAX),
        KeyCode::PageUp => app.move_by(-page),
        KeyCode::PageDown => app.move_by(page),
        KeyCode::Char('/') => app.mode = Mode::Filter,
        KeyCode::Esc => {
            if !app.filter.is_empty() {
                app.set_filter(String::new());
            } else {
                app.marks.clear();
            }
        }
        KeyCode::Char('.') => app.toggle_hidden(),
        KeyCode::Char('s') => app.cycle_sort(),
        KeyCode::Char('S') => app.reverse_sort(),
        KeyCode::Char('p') => app.preview = !app.preview,
        KeyCode::Char('b') => app.sidebar = !app.sidebar,
        KeyCode::Char('f') => app.show_in_finder(),
        KeyCode::Char('y') => app.copy_path(),
        KeyCode::Char('c') => app.set_clip(false),
        KeyCode::Char('x') => app.set_clip(true),
        KeyCode::Char('v') => app.paste(),
        KeyCode::Char('r') => app.start_rename(),
        KeyCode::Char('n') => app.new_folder(),
        KeyCode::Char('d') | KeyCode::Delete => app.ask_trash(),
        KeyCode::Char('m') => app.toggle_mark(),
        KeyCode::Char(' ') if app.current().is_some() => app.mode = Mode::QuickLook,
        KeyCode::Char('?') => app.mode = Mode::Help,
        KeyCode::Char(c @ '1'..='4') => {
            app.view = View::ALL[(c as u8 - b'1') as usize];
            app.follow = true;
        }
        _ => {}
    }
}

fn filter(app: &mut App, k: KeyEvent) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
            app.set_filter(String::new());
        }
        KeyCode::Enter => {
            app.mode = Mode::Normal;
            if app.filter.is_empty() {
                return;
            }
            // Like a search field: Enter on a single match opens it.
            if app.visible.len() == 1 {
                app.open();
            }
        }
        KeyCode::Up => app.move_by(-1),
        KeyCode::Down => app.move_by(1),
        KeyCode::Backspace => {
            let mut text = app.filter.clone();
            if text.pop().is_none() {
                app.mode = Mode::Normal;
            }
            app.set_filter(text);
        }
        KeyCode::Char('u') if ctrl => app.set_filter(String::new()),
        KeyCode::Char(c) if !ctrl => {
            let mut text = app.filter.clone();
            text.push(c);
            app.set_filter(text);
        }
        _ => {}
    }
}

fn rename(app: &mut App, k: KeyEvent) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Enter => {
            app.commit_rename();
            return;
        }
        KeyCode::Esc => {
            app.mode = Mode::Normal;
            return;
        }
        _ => {}
    }
    let Mode::Rename(r) = &mut app.mode else {
        return;
    };
    match k.code {
        KeyCode::Left => r.cursor = r.cursor.saturating_sub(1),
        KeyCode::Right => r.cursor = (r.cursor + 1).min(r.text.len()),
        KeyCode::Home => r.cursor = 0,
        KeyCode::End => r.cursor = r.text.len(),
        KeyCode::Char('a') if ctrl => r.cursor = 0,
        KeyCode::Char('e') if ctrl => r.cursor = r.text.len(),
        KeyCode::Char('u') if ctrl => {
            r.text.drain(..r.cursor);
            r.cursor = 0;
        }
        KeyCode::Backspace if r.cursor > 0 => {
            r.cursor -= 1;
            r.text.remove(r.cursor);
        }
        KeyCode::Delete if r.cursor < r.text.len() => {
            r.text.remove(r.cursor);
        }
        // A slash would make a path, not a name.
        KeyCode::Char(c) if !ctrl && c != '/' => {
            r.text.insert(r.cursor, c);
            r.cursor += 1;
        }
        _ => {}
    }
}

fn quick_look(app: &mut App, k: KeyEvent) {
    match k.code {
        KeyCode::Char(' ') | KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        KeyCode::Up | KeyCode::Left | KeyCode::Char('k') | KeyCode::Char('h') => app.move_by(-1),
        KeyCode::Down | KeyCode::Right | KeyCode::Char('j') | KeyCode::Char('l') => app.move_by(1),
        KeyCode::Enter => {
            app.mode = Mode::Normal;
            app.open();
        }
        _ => {}
    }
}

fn mouse(app: &mut App, clicks: &mut Clicks, m: MouseEvent) -> bool {
    let pos = Position::new(m.column, m.row);
    let hit = app
        .hits
        .iter()
        .rev()
        .find(|(rect, _)| rect.contains(pos))
        .map(|(_, hit)| hit.clone());
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            app.status = None;
            match app.mode {
                Mode::Help | Mode::Confirm(_) => {
                    app.mode = Mode::Normal;
                    return true;
                }
                Mode::Rename(_) => app.commit_rename(),
                Mode::Filter => app.mode = Mode::Normal,
                _ => {}
            }
            let Some(hit) = hit else { return true };
            let now = Instant::now();
            let double = clicks
                .last
                .as_ref()
                .is_some_and(|(at, prev)| *prev == hit && now - *at < DOUBLE_CLICK);
            clicks.last = (!double).then(|| (now, hit.clone()));
            click(app, hit, double);
            true
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            let up = m.kind == MouseEventKind::ScrollUp;
            let quick_look = matches!(app.mode, Mode::QuickLook);
            if !quick_look && matches!(app.view, View::Icons | View::List) {
                app.scroll_by(if up { -1 } else { 1 } * if app.view == View::List { 3 } else { 1 });
            } else {
                app.move_by(if up { -1 } else { 1 });
            }
            true
        }
        _ => false,
    }
}

fn click(app: &mut App, hit: Hit, double: bool) {
    match hit {
        Hit::Item(i) => {
            app.select(i);
            if double {
                if matches!(app.mode, Mode::QuickLook) {
                    app.mode = Mode::Normal;
                }
                app.open();
            }
        }
        Hit::Go(path) => app.go(path),
        Hit::Reveal(path) => {
            app.reveal(&path);
            if double {
                app.open();
            }
        }
        Hit::Back => app.go_back(),
        Hit::Forward => app.go_forward(),
        Hit::View(v) => {
            app.view = v;
            app.follow = true;
        }
        Hit::Filter => app.mode = Mode::Filter,
        Hit::Sort(key) => app.sort_by(key),
    }
}
