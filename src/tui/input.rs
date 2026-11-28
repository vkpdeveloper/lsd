//! Keys and mouse. Keys follow vim (counts, motions, the `y` and `d`
//! operators, visual mode, `:` commands) on top of Finder's Enter, Space,
//! arrows and Delete.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;

use super::app::{self, Align, App, Hit, Mode, RenameAt, View};
use super::git;

/// Last left click, for double clicks.
#[derive(Default)]
pub struct Clicks {
    last: Option<(Instant, Hit)>,
}

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Counts past this are clamped, so a held key cannot overflow them.
const MAX_COUNT: usize = 9999;

/// A vim command typed so far: `2d3` waits for a motion, `g` for its second
/// key.
#[derive(Clone, Copy, Default)]
pub struct Pending {
    count: usize,
    /// `y` or `d`, with the count typed before it.
    op: Option<(char, usize)>,
    /// The first key of a pair: g, z, Z, c, f or F.
    prefix: Option<char>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.count == 0 && self.op.is_none() && self.prefix.is_none()
    }

    /// Both counts multiplied, as in vim: `2d3j` acts on six moves.
    fn total(&self) -> Option<usize> {
        let before = self.op.map_or(0, |(_, n)| n);
        (before > 0 || self.count > 0).then(|| before.max(1) * self.count.max(1))
    }

    /// The keys as typed, for the corner of the status bar (vim's showcmd).
    pub fn show(&self) -> String {
        let mut s = String::new();
        if let Some((op, n)) = self.op {
            if n > 0 {
                s.push_str(&n.to_string());
            }
            s.push(op);
        }
        if self.count > 0 {
            s.push_str(&self.count.to_string());
        }
        s.extend(self.prefix);
        s
    }
}

/// A key after the first key of a pair, if any: (prefix, key, Ctrl held).
type Key = (Option<char>, KeyCode, bool);

/// Where a motion moves the selection. With an operator, everything from the
/// selection to there is acted on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Motion {
    Down,
    Up,
    Left,
    Right,
    /// `gg`, or the item a count names.
    First,
    /// `G`, or the item a count names.
    Last,
    RowStart,
    RowEnd,
    /// Half pages (Ctrl-d, Ctrl-u) or whole ones (Ctrl-f, Ctrl-b), down
    /// when positive.
    Page {
        halves: isize,
    },
    /// `H`, `M`, `L`.
    Screen(Align),
    /// The next name starting with a letter: `f`, `F`, `;`, `,`.
    Find {
        c: char,
        back: bool,
    },
    /// The next name containing the last filter: `n`, `N`.
    Match {
        back: bool,
    },
}

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
        Mode::Normal | Mode::Visual(_) => vim(app, k),
        Mode::Filter => filter(app, k),
        Mode::Command(_) => command_line(app, k),
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
        Mode::Diff { .. } => git::key(app, k),
    }
}

/// Normal and visual mode.
fn vim(app: &mut App, k: KeyEvent) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let visual = matches!(app.mode, Mode::Visual(_));
    let mut keys = std::mem::take(&mut app.keys);
    if k.code == KeyCode::Esc && !keys.is_empty() {
        // Esc drops a half-typed command and does nothing else.
        return;
    }
    // Digits build a count. A lone 0 is a motion instead.
    if let KeyCode::Char(d @ '0'..='9') = k.code
        && !ctrl
        && keys.prefix.is_none()
        && (d != '0' || keys.count > 0)
    {
        keys.count = (keys.count * 10 + d.to_digit(10).unwrap_or(0) as usize).min(MAX_COUNT);
        app.keys = keys;
        return;
    }
    let count = keys.total();
    let key: Key = (keys.prefix.take(), k.code, ctrl);

    // The first key of a pair waits for the second.
    if let (None, KeyCode::Char(c), false) = key {
        let pair = match (keys.op, visual) {
            (Some(_), _) => matches!(c, 'g' | 'f' | 'F'),
            (None, true) => matches!(c, 'g' | 'z' | 'f' | 'F'),
            (None, false) => matches!(c, 'g' | 'z' | 'Z' | 'c' | 'f' | 'F'),
        };
        if pair {
            keys.prefix = Some(c);
            app.keys = keys;
            return;
        }
    }
    // Before motions, so `yn` copies names instead of yanking to the next match.
    if let Some((op, _)) = keys.op {
        match key {
            (None, KeyCode::Char(c), false) if c == op => {
                let paths = lines(app, count);
                app.set_clip(paths, op == 'd');
                return;
            }
            (None, KeyCode::Char(c @ ('p' | 'n')), false) if op == 'y' => {
                let paths = lines(app, count);
                app.copy_text(&paths, c == 'n');
                return;
            }
            _ => {}
        }
    }
    if let Some(m) = motion(app, key) {
        if let (Some(p @ ('f' | 'F')), KeyCode::Char(c), _) = key {
            app.find = Some((c, p == 'F'));
        }
        moved(app, keys.op.map(|(op, _)| op), m, count);
        return;
    }
    if keys.op.is_some() {
        // Anything but a motion cancels the operator.
        return;
    }
    if visual {
        visual_key(app, key);
        return;
    }
    if let (None, KeyCode::Char(op @ ('y' | 'd')), false) = key {
        keys.op = Some((op, keys.count));
        keys.count = 0;
        app.keys = keys;
        return;
    }
    command(app, key, count);
}

fn motion(app: &App, key: Key) -> Option<Motion> {
    use KeyCode::*;
    // In the list and columns, h and l go up and into folders instead.
    let sideways = matches!(app.view, View::Icons | View::Gallery);
    Some(match key {
        (None, Down, _) | (None, Char('j'), false) | (None, Char('n'), true) => Motion::Down,
        (None, Up, _) | (None, Char('k'), false) | (None, Char('p'), true) => Motion::Up,
        (None, Left, _) | (None, Char('h'), false) if sideways => Motion::Left,
        (None, Right, _) | (None, Char('l'), false) if sideways => Motion::Right,
        (Some('g'), Char('g'), _) | (None, Home, _) => Motion::First,
        (None, Char('G'), false) | (None, End, _) => Motion::Last,
        (None, Char('0' | '^'), false) => Motion::RowStart,
        (None, Char('$'), false) => Motion::RowEnd,
        (None, Char('d'), true) => Motion::Page { halves: 1 },
        (None, Char('u'), true) => Motion::Page { halves: -1 },
        (None, Char('f'), true) | (None, PageDown, _) => Motion::Page { halves: 2 },
        (None, Char('b'), true) | (None, PageUp, _) => Motion::Page { halves: -2 },
        (None, Char('H'), false) => Motion::Screen(Align::Top),
        (None, Char('M'), false) => Motion::Screen(Align::Middle),
        (None, Char('L'), false) => Motion::Screen(Align::Bottom),
        (Some(p @ ('f' | 'F')), Char(c), false) => Motion::Find { c, back: p == 'F' },
        (None, Char(';'), false) => {
            let (c, back) = app.find?;
            Motion::Find { c, back }
        }
        (None, Char(','), false) => {
            let (c, back) = app.find?;
            Motion::Find { c, back: !back }
        }
        (None, Char('n'), false) => Motion::Match { back: false },
        (None, Char('N'), false) => Motion::Match { back: true },
        _ => return None,
    })
}

/// The visible item a motion lands on, or None when it cannot move.
fn target(app: &App, m: Motion, count: Option<usize>) -> Option<usize> {
    let n = app.visible.len();
    let last = n.checked_sub(1)?;
    let sel = app.sel.min(last);
    let cols = app.cols();
    let times = count.unwrap_or(1);
    let step = |by: usize, back: bool| {
        if back {
            sel.saturating_sub(by)
        } else {
            sel.saturating_add(by).min(last)
        }
    };
    let to = match m {
        Motion::Down | Motion::Up if cols > 1 => {
            // Row by row in the icon grid, until the edge.
            let rows = if m == Motion::Down { 1 } else { -1 };
            let mut at = None;
            for _ in 0..times {
                match app::grid_step(at.unwrap_or(sel), rows, cols, n) {
                    Some(i) => at = Some(i),
                    None => break,
                }
            }
            return at;
        }
        Motion::Down | Motion::Right => step(times, false),
        Motion::Up | Motion::Left => step(times, true),
        Motion::First => count.map_or(0, |c| c - 1).min(last),
        Motion::Last => count.map_or(last, |c| c - 1).min(last),
        Motion::RowStart => sel / cols * cols,
        Motion::RowEnd => (sel / cols * cols + cols - 1).min(last),
        Motion::Page { halves } => {
            let rows = app.rows_shown();
            let rows = if halves.abs() == 1 {
                rows.div_ceil(2)
            } else {
                rows
            };
            step(rows * cols * times, halves < 0)
        }
        Motion::Screen(at) => {
            let last_row = last / cols;
            let top = app.top_row().min(last_row);
            let bottom = (top + app.rows_shown() - 1).min(last_row);
            let row = match at {
                Align::Top => (top + times - 1).min(bottom),
                Align::Middle => top + (bottom - top) / 2,
                Align::Bottom => bottom.saturating_sub(times - 1).max(top),
            };
            (row * cols + sel % cols).min(last)
        }
        Motion::Find { c, back } => seek(sel, n, back, times, |i| {
            app.entry(i)
                .and_then(|e| e.name.chars().next())
                .is_some_and(|first| first.to_lowercase().eq(c.to_lowercase()))
        })?,
        Motion::Match { back } => {
            let needle = app.search.to_lowercase();
            seek(sel, n, back, times, |i| {
                app.entry(i)
                    .is_some_and(|e| e.name.to_lowercase().contains(&needle))
            })?
        }
    };
    Some(to)
}

/// The `times`-th item after `from` (before it when `back`) that `hit`
/// accepts, wrapping around the ends.
fn seek(
    from: usize,
    n: usize,
    back: bool,
    times: usize,
    hit: impl Fn(usize) -> bool,
) -> Option<usize> {
    let mut at = from;
    for _ in 0..times {
        at = (1..=n)
            .map(|d| if back { (at + n - d) % n } else { (at + d) % n })
            .find(|&i| hit(i))?;
    }
    Some(at)
}

/// Moves the selection, or with an operator, acts on everything from the
/// selection to where the motion lands.
fn moved(app: &mut App, op: Option<char>, m: Motion, count: Option<usize>) {
    if matches!(m, Motion::Match { .. }) && app.search.is_empty() {
        app.say("Filter with / first");
        return;
    }
    let Some(to) = target(app, m, count) else {
        return;
    };
    match op {
        Some(op) => {
            let paths = app.span(app.sel, to);
            app.select(app.sel.min(to));
            app.set_clip(paths, op == 'd');
        }
        None => {
            app.select(to);
            app.update_visual();
        }
    }
}

/// What `yy`, `dd`, `x` and `D` act on: with a count, that many items from
/// the selection down; otherwise the marked items, or the selection.
fn lines(app: &App, count: Option<usize>) -> Vec<PathBuf> {
    match count {
        Some(n) => app.span(app.sel, app.sel + n - 1),
        None => app.targets(),
    }
}

fn visual_key(app: &mut App, key: Key) {
    use KeyCode::*;
    match key {
        (None, Char(c @ ('y' | 'Y' | 'd' | 'x')), false) => {
            let paths = app.targets();
            app.end_visual(false);
            app.set_clip(paths, matches!(c, 'd' | 'x'));
        }
        (None, Char('D'), false) | (None, Delete, _) => {
            let paths = app.targets();
            app.end_visual(false);
            app.ask_trash(paths);
        }
        (None, Char('o' | 'O'), false) => app.swap_visual(),
        (None, Char('v' | 'V') | Esc, _) => app.end_visual(true),
        _ => {}
    }
}

/// Normal mode keys that are neither motions nor operators.
fn command(app: &mut App, key: Key, count: Option<usize>) {
    use KeyCode::*;
    let times = count.unwrap_or(1);
    let lists = matches!(app.view, View::List | View::Columns);
    match key {
        (None, Enter, _) => app.open(),
        (Some('g'), Char('x'), _) => app.open_default(),
        (Some('g'), Char('d'), _) => git::focus(app, false),
        (Some('g'), Char('D'), _) => git::focus(app, true),
        (None, Backspace, _) | (None, Char('-'), false) => {
            for _ in 0..times {
                app.parent();
            }
        }
        (None, Left, _) | (None, Char('h'), false) if lists => {
            for _ in 0..times {
                app.parent();
            }
        }
        (None, Right, _) | (None, Char('l'), false) if lists => {
            if app.current().is_some_and(|e| e.is_dir) {
                app.open();
            }
        }
        (None, Char('['), false) | (None, Char('o'), true) => {
            for _ in 0..times {
                app.go_back();
            }
        }
        (None, Char(']') | Tab, false) | (None, Char('i'), true) => {
            for _ in 0..times {
                app.go_forward();
            }
        }
        (None, Char('~'), false) => app.home(),
        (None, Char('/'), false) => app.mode = Mode::Filter,
        (None, Char(':'), false) => app.mode = Mode::Command(String::new()),
        (None, Esc, _) => {
            if !app.filter.is_empty() {
                app.set_filter(String::new());
            } else {
                app.marks.clear();
            }
        }
        (None, Char('.'), false) | (Some('z'), Char('h'), _) => app.toggle_hidden(),
        (None, Char('s'), false) => app.cycle_sort(),
        (None, Char('S'), false) => app.reverse_sort(),
        (Some('z'), Char('p'), _) => app.preview = !app.preview,
        (Some('z'), Char('s'), _) => app.sidebar = !app.sidebar,
        (Some('z'), Char(c @ ('i' | 'l' | 'c' | 'g')), _) => {
            app.view = match c {
                'i' => View::Icons,
                'l' => View::List,
                'c' => View::Columns,
                _ => View::Gallery,
            };
            app.follow = true;
        }
        (Some('z'), Char('t'), _) => app.align(Align::Top),
        (Some('z'), Char('z'), _) => app.align(Align::Middle),
        (Some('z'), Char('b'), _) => app.align(Align::Bottom),
        (None, Char('e'), true) => app.scroll_by(times as isize),
        (None, Char('y'), true) => app.scroll_by(-(times as isize)),
        (None, Char('O'), false) => app.show_in_finder(),
        (None, Char(c @ ('x' | 'Y')), false) => {
            let paths = lines(app, count);
            app.set_clip(paths, c == 'x');
        }
        (None, Char('p' | 'P'), false) => app.paste(),
        (None, Char('D'), false) | (None, Delete, _) => {
            let paths = lines(app, count);
            app.ask_trash(paths);
        }
        (None, Char('r'), false) => app.start_rename(RenameAt::Extension),
        (None, Char('I'), false) => app.start_rename(RenameAt::Start),
        (None, Char('A'), false) => app.start_rename(RenameAt::End),
        (Some('c'), Char('w'), _) => app.start_rename(RenameAt::NewStem),
        (Some('c'), Char('c'), _) | (None, Char('C'), false) => app.start_rename(RenameAt::NewName),
        (None, Char('a'), false) => app.new_folder(),
        (None, Char('m'), false) => {
            for _ in 0..times {
                app.toggle_mark();
            }
        }
        (None, Char('v' | 'V'), false) => app.start_visual(),
        (None, Char(' '), false) if app.current().is_some() => app.mode = Mode::QuickLook,
        (None, Char('?'), false) => app.mode = Mode::Help,
        (None, Char('q'), false) | (Some('Z'), Char('Z' | 'Q'), _) => app.quit = true,
        _ => {}
    }
}

/// Runs a `:` command.
fn ex(app: &mut App, line: &str) {
    let (cmd, arg) = line
        .split_once(' ')
        .map_or((line, ""), |(cmd, arg)| (cmd, arg.trim()));
    if let Ok(n) = cmd.parse::<usize>() {
        app.select(n.saturating_sub(1));
        return;
    }
    match cmd {
        "" => {}
        "q" | "q!" | "qa" | "qa!" | "quit" | "wq" | "wqa" | "x" | "xa" => app.quit = true,
        "cd" => app.cd(arg),
        "h" | "help" => app.mode = Mode::Help,
        _ => app.fail(format!("Unknown command: {line}")),
    }
}

/// Where the word before the end of `text` starts, for Ctrl-w: spaces, then
/// a run of letters and digits or a run of other characters.
fn word_start(text: &[char]) -> usize {
    let mut i = text.len();
    while i > 0 && text[i - 1].is_whitespace() {
        i -= 1;
    }
    let Some(word) = text[..i].last().map(|c| c.is_alphanumeric()) else {
        return 0;
    };
    while i > 0 && !text[i - 1].is_whitespace() && text[i - 1].is_alphanumeric() == word {
        i -= 1;
    }
    i
}

/// Ctrl-w at the end of a one-line field.
fn delete_word(text: &mut String) {
    let chars: Vec<char> = text.chars().collect();
    *text = chars[..word_start(&chars)].iter().collect();
}

fn command_line(app: &mut App, k: KeyEvent) {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let Mode::Command(text) = &mut app.mode else {
        return;
    };
    match k.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => {
            let line = std::mem::take(text);
            app.mode = Mode::Normal;
            ex(app, line.trim());
        }
        KeyCode::Backspace => {
            if text.pop().is_none() {
                app.mode = Mode::Normal;
            }
        }
        KeyCode::Char('u') if ctrl => text.clear(),
        KeyCode::Char('w') if ctrl => delete_word(text),
        KeyCode::Char(c) if !ctrl => text.push(c),
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
        KeyCode::Char('w') if ctrl => {
            let mut text = app.filter.clone();
            delete_word(&mut text);
            app.set_filter(text);
        }
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
        KeyCode::Char('w') if ctrl => {
            let start = word_start(&r.text[..r.cursor]);
            r.text.drain(start..r.cursor);
            r.cursor = start;
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
            app.keys = Pending::default();
            match app.mode {
                Mode::Help | Mode::Confirm(_) => {
                    app.mode = Mode::Normal;
                    return true;
                }
                Mode::Rename(_) => app.commit_rename(),
                Mode::Filter | Mode::Command(_) => app.mode = Mode::Normal,
                Mode::Visual(_) => app.end_visual(true),
                Mode::Diff { .. } => app.mode = Mode::Normal,
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
            if matches!(app.mode, Mode::Diff { .. }) {
                app.git.view.scroll(if up { -3 } else { 3 });
                return true;
            }
            let quick_look = matches!(app.mode, Mode::QuickLook);
            if !quick_look && matches!(app.view, View::Icons | View::List) {
                app.scroll_by(if up { -1 } else { 1 } * if app.view == View::List { 3 } else { 1 });
            } else {
                app.move_by(if up { -1 } else { 1 });
                app.update_visual();
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::entry::{Entry, Sort};
    use crate::tui::work::Listing;

    const DIR: &str = "/lsd-input-test";

    /// A browser showing `names` (already in sorted order) in the list view,
    /// four rows to a screen.
    fn app(names: &[&str]) -> App {
        let (tx, _rx) = std::sync::mpsc::channel();
        let dir = PathBuf::from(DIR);
        let mut app = App::new(dir.clone(), None, Sort::default(), false, tx);
        let entries = names
            .iter()
            .map(|name| Entry {
                name: name.to_string(),
                path: dir.join(name),
                is_dir: false,
                is_symlink: false,
                size: 0,
                modified: None,
                created: None,
                mode: 0o644,
            })
            .collect();
        let sort = Sort::default();
        app.on_dir(
            dir,
            Ok(Listing {
                entries,
                mtime: None,
                sort,
            }),
        );
        app.view = View::List;
        app.page = 4;
        app
    }

    /// Types keys: plain characters, or `<Esc>`, `<CR>`, `<C-x>`.
    fn press(app: &mut App, keys: &str) {
        let mut rest = keys;
        while let Some(c) = rest.chars().next() {
            let (code, mods, len) = match rest.find('>') {
                Some(end) if c == '<' => {
                    let (code, mods) = match &rest[1..end] {
                        "Esc" => (KeyCode::Esc, KeyModifiers::NONE),
                        "CR" => (KeyCode::Enter, KeyModifiers::NONE),
                        name => {
                            let c = name.strip_prefix("C-").and_then(|n| n.chars().next());
                            (KeyCode::Char(c.unwrap()), KeyModifiers::CONTROL)
                        }
                    };
                    (code, mods, end + 1)
                }
                _ => (KeyCode::Char(c), KeyModifiers::NONE, c.len_utf8()),
            };
            key(app, KeyEvent::new(code, mods));
            rest = &rest[len..];
        }
    }

    fn names(paths: &[PathBuf]) -> Vec<&str> {
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect()
    }

    fn clip(app: &App) -> (Vec<&str>, bool) {
        let clip = app.clip.as_ref().expect("nothing copied or cut");
        (names(&clip.paths), clip.cut)
    }

    const ABC: &[&str] = &["a", "b", "c", "d", "e", "f", "g", "h"];

    #[test]
    fn counts_and_motions() {
        let mut app = app(ABC);
        press(&mut app, "5j");
        assert_eq!(app.sel, 5);
        press(&mut app, "2k");
        assert_eq!(app.sel, 3);
        press(&mut app, "G");
        assert_eq!(app.sel, 7);
        press(&mut app, "gg");
        assert_eq!(app.sel, 0);
        press(&mut app, "3G");
        assert_eq!(app.sel, 2);
        press(&mut app, "10j");
        assert_eq!(app.sel, 7);
        // Half a screen of four rows.
        press(&mut app, "gg<C-d>");
        assert_eq!(app.sel, 2);
        press(&mut app, "<C-f>");
        assert_eq!(app.sel, 6);
        // H, M and L use the rows on screen: c to f.
        app.scroll = 2;
        press(&mut app, "H");
        assert_eq!(app.sel, 2);
        press(&mut app, "L");
        assert_eq!(app.sel, 5);
        press(&mut app, "2H");
        assert_eq!(app.sel, 3);
    }

    #[test]
    fn operators_take_counts_and_motions() {
        let mut app = app(ABC);
        press(&mut app, "jyy");
        assert_eq!(clip(&app), (vec!["b"], false));
        press(&mut app, "3dd");
        assert_eq!(clip(&app), (vec!["b", "c", "d"], true));
        press(&mut app, "dj");
        assert_eq!(clip(&app), (vec!["b", "c"], true));
        // Counts on both sides multiply: 2 * 2 moves up from f.
        press(&mut app, "4j2y2k");
        assert_eq!(clip(&app), (vec!["b", "c", "d", "e", "f"], false));
        assert_eq!(app.sel, 1, "the selection goes to the top of the range");
        press(&mut app, "yG");
        assert_eq!(clip(&app), (vec!["b", "c", "d", "e", "f", "g", "h"], false));
        press(&mut app, "2x");
        assert_eq!(clip(&app), (vec!["b", "c"], true));
        // Esc drops a half-typed operator, and the next key is a plain move.
        app.clip = None;
        press(&mut app, "d<Esc>j");
        assert!(app.clip.is_none());
        assert_eq!(app.sel, 2);
        // Without a count, yy takes the marked items.
        press(&mut app, "ggmmjmyy");
        assert_eq!(clip(&app), (vec!["a", "b", "d"], false));
    }

    #[test]
    fn yank_path_and_name_go_to_the_clipboard() {
        let mut app = app(ABC);
        press(&mut app, "2yn");
        assert!(app.clip.is_none());
        assert_eq!(app.status.as_ref().unwrap().text, "Copied 2 names");
        assert!(!app.osc.is_empty());
        press(&mut app, "yp");
        assert_eq!(app.status.as_ref().unwrap().text, "Copied path");
    }

    #[test]
    fn visual_mode_selects_a_range() {
        let mut app = app(ABC);
        press(&mut app, "jvjj");
        assert!(matches!(app.mode, Mode::Visual(_)));
        assert_eq!(app.marks.len(), 3);
        // o jumps to the other end; the range stays the same.
        press(&mut app, "o");
        assert_eq!(app.sel, 1);
        press(&mut app, "k");
        assert_eq!(app.marks.len(), 4);
        press(&mut app, "d");
        assert_eq!(clip(&app), (vec!["a", "b", "c", "d"], true));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.marks.is_empty());
        // Esc cancels and puts the marks from before back.
        press(&mut app, "Gm");
        press(&mut app, "ggVjj<Esc>");
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(app.marks.len(), 1);
        assert!(app.marks.contains(Path::new(DIR).join("h").as_path()));
    }

    #[test]
    fn find_and_repeat() {
        let mut app = app(&["alpha", "beta", "Bravo", "charlie", "delta"]);
        press(&mut app, "fb");
        assert_eq!(app.sel, 1);
        press(&mut app, ";");
        assert_eq!(app.sel, 2, "letters match either case");
        press(&mut app, ",");
        assert_eq!(app.sel, 1);
        press(&mut app, "Fd");
        assert_eq!(app.sel, 4, "searching back wraps around");
        // n and N repeat the last filter after it was cleared.
        app.set_filter("ta".into());
        app.set_filter(String::new());
        press(&mut app, "ggn");
        assert_eq!(app.sel, 1);
        press(&mut app, "n");
        assert_eq!(app.sel, 4);
        press(&mut app, "N");
        assert_eq!(app.sel, 1);
    }

    #[test]
    fn rename_variants() {
        let mut app = app(&["notes.txt"]);
        let field = |app: &App| match &app.mode {
            Mode::Rename(r) => (r.text.iter().collect::<String>(), r.cursor),
            _ => panic!("not renaming"),
        };
        for (keys, text, cursor) in [
            ("r", "notes.txt", 5),
            ("I", "notes.txt", 0),
            ("A", "notes.txt", 9),
            ("cw", ".txt", 0),
            ("cc", "", 0),
        ] {
            press(&mut app, keys);
            assert_eq!(field(&app), (text.to_string(), cursor), "{keys}");
            press(&mut app, "<Esc>");
        }
        press(&mut app, "A<C-w>");
        assert_eq!(field(&app), ("notes.".to_string(), 6));
    }

    #[test]
    fn colon_commands() {
        let mut app = app(ABC);
        press(&mut app, ":5<CR>");
        assert_eq!(app.sel, 4);
        press(&mut app, ":nope<CR>");
        assert!(app.status.as_ref().unwrap().error);
        assert!(!app.quit);
        press(&mut app, ":q<CR>");
        assert!(app.quit);
    }

    #[test]
    fn grid_moves_by_rows() {
        let mut app = app(ABC);
        app.view = View::Icons;
        app.grid_cols = 3;
        app.page = 6;
        press(&mut app, "2j");
        assert_eq!(app.sel, 6);
        press(&mut app, "l$");
        assert_eq!(app.sel, 7, "the last row is short");
        press(&mut app, "0k");
        assert_eq!(app.sel, 3);
    }

    #[test]
    fn ctrl_w_deletes_a_word() {
        let start = |s: &str| word_start(&s.chars().collect::<Vec<_>>());
        assert_eq!(start("my notes"), 3);
        assert_eq!(start("my notes  "), 3);
        assert_eq!(start("report-2026"), 7);
        assert_eq!(start("a.."), 1);
        assert_eq!(start(""), 0);
    }
}
