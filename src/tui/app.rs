//! Browser state and the actions keys and clicks trigger. Nothing here draws.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ratatui::layout::Rect;

use super::work::{self, Done, Listing, Msg};
use crate::entry::{self, Entry, Sort, SortKey};
use crate::fsops;
use crate::places::{self, Place};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Icons,
    List,
    Columns,
    Gallery,
}

impl View {
    pub const ALL: [View; 4] = [View::Icons, View::List, View::Columns, View::Gallery];

    pub fn label(self) -> &'static str {
        match self {
            View::Icons => "Icons",
            View::List => "List",
            View::Columns => "Columns",
            View::Gallery => "Gallery",
        }
    }
}

pub enum Mode {
    Normal,
    /// Typing in the filter field.
    Filter,
    Rename(Rename),
    /// Waiting for y/n before moving these to the Trash.
    Confirm(Vec<PathBuf>),
    Help,
    QuickLook,
}

pub struct Rename {
    pub path: PathBuf,
    pub text: Vec<char>,
    pub cursor: usize,
}

pub struct Status {
    pub text: String,
    pub error: bool,
}

pub struct Clip {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

/// Something clickable, recorded while drawing.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    /// An item of the current folder, by index into `visible`.
    Item(usize),
    /// A folder to go to (sidebar, path bar).
    Go(PathBuf),
    /// An item in another folder (Miller columns): go there and select it.
    Reveal(PathBuf),
    Back,
    Forward,
    View(View),
    Filter,
    Sort(SortKey),
}

/// How long to hold the old frame while a folder loads, so fast folders
/// appear without a "Loading" flash.
const DEFER: Duration = Duration::from_millis(60);

pub struct App {
    pub cwd: PathBuf,
    pub list: Rc<Listing>,
    /// Indices into `list.entries` that pass the hidden and name filters.
    pub visible: Vec<usize>,
    /// Index into `visible`.
    pub sel: usize,
    /// First visible row of the current view.
    pub scroll: usize,
    /// Scroll so the selection is visible on the next draw.
    pub follow: bool,
    pub view: View,
    pub sort: Sort,
    pub hidden: bool,
    pub filter: String,
    pub mode: Mode,
    pub marks: HashSet<PathBuf>,
    pub clip: Option<Clip>,
    pub back: Vec<PathBuf>,
    pub fwd: Vec<PathBuf>,
    /// Last selected name per folder, restored on return.
    memory: HashMap<PathBuf, String>,
    /// Name to select once the current folder (re)loads.
    pending: Option<String>,
    pub dirs: HashMap<PathBuf, Rc<Listing>>,
    loading: HashSet<PathBuf>,
    /// Folders the last draw needed but did not have.
    pub want_dirs: Vec<PathBuf>,
    pub error: Option<String>,
    pub status: Option<Status>,
    pub preview: bool,
    pub sidebar: bool,
    pub favorites: Vec<Place>,
    pub locations: Vec<Place>,
    pub free: Option<u64>,
    pub hits: Vec<(Rect, Hit)>,
    /// Icon grid columns and items per page, from the last draw.
    pub grid_cols: usize,
    pub page: usize,
    /// Escapes for the terminal itself (clipboard), written before the next frame.
    pub osc: Vec<u8>,
    /// Hold drawing until this instant while the current folder loads.
    pub defer: Option<Instant>,
    pub quit: bool,
    tx: Sender<Msg>,
}

impl App {
    pub fn new(
        start: PathBuf,
        select: Option<String>,
        sort: Sort,
        hidden: bool,
        tx: Sender<Msg>,
    ) -> App {
        let canon = |mut p: Place| {
            p.path = std::fs::canonicalize(&p.path).unwrap_or(p.path);
            p
        };
        let mut app = App {
            cwd: PathBuf::new(),
            list: Rc::new(Listing::empty(sort)),
            visible: Vec::new(),
            sel: 0,
            scroll: 0,
            follow: true,
            view: View::Icons,
            sort,
            hidden,
            filter: String::new(),
            mode: Mode::Normal,
            marks: HashSet::new(),
            clip: None,
            back: Vec::new(),
            fwd: Vec::new(),
            memory: HashMap::new(),
            pending: None,
            dirs: HashMap::new(),
            loading: HashSet::new(),
            want_dirs: Vec::new(),
            error: None,
            status: None,
            preview: true,
            sidebar: true,
            favorites: places::favorites().into_iter().map(canon).collect(),
            locations: places::locations().into_iter().map(canon).collect(),
            free: None,
            hits: Vec::new(),
            grid_cols: 1,
            page: 1,
            osc: Vec::new(),
            defer: None,
            quit: false,
            tx,
        };
        if let Some(name) = select {
            app.memory.insert(start.clone(), name);
        }
        app.enter(start);
        app
    }

    pub fn current(&self) -> Option<&Entry> {
        self.visible.get(self.sel).map(|&i| &self.list.entries[i])
    }

    pub fn entry(&self, visible_index: usize) -> Option<&Entry> {
        self.visible
            .get(visible_index)
            .map(|&i| &self.list.entries[i])
    }

    pub fn is_loading(&self) -> bool {
        self.loading.contains(&self.cwd)
    }

    /// Marked items in display order, or the selection when nothing is marked.
    pub fn targets(&self) -> Vec<PathBuf> {
        if self.marks.is_empty() {
            return self.current().map(|e| e.path.clone()).into_iter().collect();
        }
        self.visible
            .iter()
            .map(|&i| &self.list.entries[i].path)
            .filter(|p| self.marks.contains(*p))
            .cloned()
            .collect()
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
        });
    }

    pub fn fail(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: true,
        });
    }

    // Selection

    pub fn select(&mut self, i: usize) {
        self.sel = i.min(self.visible.len().saturating_sub(1));
        self.follow = true;
    }

    pub fn move_by(&mut self, delta: isize) {
        let last = self.visible.len().saturating_sub(1) as isize;
        self.select((self.sel as isize + delta).clamp(0, last.max(0)) as usize);
    }

    /// Up and down in the icon grid. Moving down into a short last row lands on
    /// its last item; moving past the edge does nothing.
    pub fn move_rows(&mut self, rows: isize) {
        let cols = self.grid_cols.max(1);
        let len = self.visible.len();
        if let Some(i) = grid_step(self.sel, rows, cols, len) {
            self.select(i);
        }
    }

    pub fn scroll_by(&mut self, rows: isize) {
        self.scroll = (self.scroll as isize + rows).max(0) as usize;
        self.follow = false;
    }

    pub fn toggle_mark(&mut self) {
        let Some(path) = self.current().map(|e| e.path.clone()) else {
            return;
        };
        if !self.marks.remove(&path) {
            self.marks.insert(path);
        }
        self.move_by(1);
    }

    // Navigation

    pub fn go(&mut self, path: PathBuf) {
        if path == self.cwd {
            return;
        }
        self.back.push(self.cwd.clone());
        self.fwd.clear();
        self.enter(path);
    }

    /// Goes to the folder holding `path` and selects it.
    pub fn reveal(&mut self, path: &Path) {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return;
        };
        let name = name.to_string_lossy().into_owned();
        if dir == self.cwd {
            if let Some(i) = self.position(&name) {
                self.select(i);
            }
            return;
        }
        self.memory.insert(dir.to_path_buf(), name);
        self.go(dir.to_path_buf());
    }

    pub fn parent(&mut self) {
        let Some(parent) = self.cwd.parent().map(Path::to_path_buf) else {
            return;
        };
        if let Some(name) = self.cwd.file_name() {
            self.memory
                .insert(parent.clone(), name.to_string_lossy().into_owned());
        }
        self.go(parent);
    }

    pub fn go_back(&mut self) {
        if let Some(path) = self.back.pop() {
            self.fwd.push(self.cwd.clone());
            self.enter(path);
        }
    }

    pub fn go_forward(&mut self) {
        if let Some(path) = self.fwd.pop() {
            self.back.push(self.cwd.clone());
            self.enter(path);
        }
    }

    pub fn home(&mut self) {
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            self.go(std::fs::canonicalize(&home).unwrap_or(home));
        }
    }

    fn enter(&mut self, path: PathBuf) {
        if let Some(name) = self.current().map(|e| e.name.clone()) {
            self.memory.insert(self.cwd.clone(), name);
        }
        self.cwd = path;
        self.filter.clear();
        self.marks.clear();
        self.mode = Mode::Normal;
        self.error = None;
        self.scroll = 0;
        self.follow = true;
        self.pending = self.memory.get(&self.cwd).cloned();
        self.free = places::free_space(&self.cwd);
        match self.dirs.get(&self.cwd).cloned() {
            Some(list) => {
                let fresh = modified(&self.cwd) == list.mtime;
                self.set_list(list);
                if !fresh {
                    self.load(self.cwd.clone());
                }
            }
            None => {
                // Keep `pending` for when the real listing arrives.
                self.list = Rc::new(Listing::empty(self.sort));
                self.visible.clear();
                self.sel = 0;
                self.load(self.cwd.clone());
                self.defer = Some(Instant::now() + DEFER);
            }
        }
    }

    pub fn load(&mut self, path: PathBuf) {
        if self.loading.insert(path.clone()) {
            work::load_dir(&self.tx, path, self.sort);
        }
    }

    /// A listing for another folder (Miller columns), loading it if needed.
    pub fn dir(&mut self, path: &Path) -> Option<Rc<Listing>> {
        match self.dirs.get(path) {
            Some(list) => Some(list.clone()),
            None => {
                if !self.loading.contains(path) {
                    self.want_dirs.push(path.to_path_buf());
                }
                None
            }
        }
    }

    pub fn on_dir(&mut self, path: PathBuf, result: Result<Listing, String>) {
        self.loading.remove(&path);
        let here = path == self.cwd;
        match result {
            Ok(mut listing) => {
                if !same_sort(listing.sort, self.sort) {
                    entry::sort(&mut listing.entries, self.sort);
                    listing.sort = self.sort;
                }
                let list = Rc::new(listing);
                if self.dirs.len() > 64 {
                    self.dirs.clear();
                }
                self.dirs.insert(path, list.clone());
                if here {
                    self.error = None;
                    self.set_list(list);
                }
            }
            Err(err) if here => {
                self.error = Some(err);
                self.set_list(Rc::new(Listing::empty(self.sort)));
            }
            Err(_) => {}
        }
        if here {
            self.defer = None;
        }
    }

    /// Replaces the current listing, keeping the selection on the same name.
    fn set_list(&mut self, list: Rc<Listing>) {
        let keep = self
            .pending
            .take()
            .or_else(|| self.current().map(|e| e.name.clone()));
        self.list = list;
        self.rebuild();
        let at = keep.and_then(|name| self.position(&name));
        self.select(at.unwrap_or(self.sel));
    }

    fn position(&self, name: &str) -> Option<usize> {
        self.visible
            .iter()
            .position(|&i| self.list.entries[i].name == name)
    }

    fn rebuild(&mut self) {
        self.visible = visible(&self.list.entries, self.hidden, &self.filter);
    }

    /// Once a second: reload when the folder changed, leave it if it is gone.
    pub fn tick(&mut self) -> bool {
        let free = places::free_space(&self.cwd);
        let changed = free != self.free;
        self.free = free;
        if self.is_loading() || self.error.is_some() {
            return changed;
        }
        if !self.cwd.is_dir() {
            let mut dir = self.cwd.clone();
            while !dir.is_dir() && dir.pop() {}
            self.enter(dir);
            return true;
        }
        if modified(&self.cwd) != self.list.mtime {
            self.load(self.cwd.clone());
        }
        changed
    }

    // View options

    pub fn set_filter(&mut self, text: String) {
        let keep = self.current().map(|e| e.name.clone());
        self.filter = text;
        self.rebuild();
        let at = keep.and_then(|name| self.position(&name));
        self.select(at.unwrap_or(0));
    }

    pub fn toggle_hidden(&mut self) {
        self.hidden = !self.hidden;
        let keep = self.current().map(|e| e.name.clone());
        self.rebuild();
        let at = keep.and_then(|name| self.position(&name));
        self.select(at.unwrap_or(self.sel));
        self.say(if self.hidden {
            "Showing hidden files"
        } else {
            "Hiding hidden files"
        });
    }

    pub fn cycle_sort(&mut self) {
        self.sort.key = match self.sort.key {
            SortKey::Name => SortKey::Kind,
            SortKey::Kind => SortKey::Date,
            SortKey::Date => SortKey::Size,
            SortKey::Size => SortKey::Name,
        };
        self.sort.reverse = false;
        self.resort();
    }

    pub fn reverse_sort(&mut self) {
        self.sort.reverse = !self.sort.reverse;
        self.resort();
    }

    pub fn sort_by(&mut self, key: SortKey) {
        if self.sort.key == key {
            self.sort.reverse = !self.sort.reverse;
        } else {
            self.sort.key = key;
            self.sort.reverse = false;
        }
        self.resort();
    }

    fn resort(&mut self) {
        let mut entries = self.list.entries.clone();
        entry::sort(&mut entries, self.sort);
        let list = Rc::new(Listing {
            entries,
            mtime: self.list.mtime,
            sort: self.sort,
        });
        // Other folders are re-read sorted the new way when shown again.
        self.dirs.clear();
        self.dirs.insert(self.cwd.clone(), list.clone());
        self.set_list(list);
        self.say(format!("Sorted by {}", sort_label(self.sort)));
    }

    // Actions

    /// Opens a folder in place, anything else (apps too) with its default app.
    pub fn open(&mut self) {
        let Some(e) = self.current() else { return };
        let (path, name) = (e.path.clone(), e.name.clone());
        if e.is_dir && !name.ends_with(".app") {
            self.go(path);
            return;
        }
        if let Err(err) = fsops::open(&path) {
            self.fail(format!("Could not open “{name}”: {}", work::describe(&err)));
        }
    }

    pub fn show_in_finder(&mut self) {
        let Some(path) = self.current().map(|e| e.path.clone()) else {
            return;
        };
        if let Err(err) = fsops::reveal(&path) {
            self.fail(format!("Could not reveal: {}", work::describe(&err)));
        }
    }

    pub fn copy_path(&mut self) {
        let paths = self.targets();
        if paths.is_empty() {
            return;
        }
        let text = paths
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");
        self.osc.extend_from_slice(&clipboard_osc(&text));
        self.say(match paths.len() {
            1 => "Copied path".to_string(),
            n => format!("Copied {n} paths"),
        });
    }

    pub fn set_clip(&mut self, cut: bool) {
        let paths = self.targets();
        if paths.is_empty() {
            return;
        }
        let what = describe_items(&paths);
        self.say(if cut {
            format!("Cut {what}")
        } else {
            format!("Copied {what}")
        });
        self.clip = Some(Clip { paths, cut });
    }

    pub fn paste(&mut self) {
        let Some(clip) = &self.clip else {
            self.say("Nothing to paste");
            return;
        };
        let (paths, cut, dest) = (clip.paths.clone(), clip.cut, self.cwd.clone());
        if cut {
            self.clip = None;
        }
        let what = describe_items(&paths);
        self.say(if cut {
            format!("Moving {what}")
        } else {
            format!("Copying {what}")
        });
        work::run_op(&self.tx, move || {
            let result = if cut {
                fsops::move_into(&paths, &dest)
            } else {
                fsops::copy_into(&paths, &dest)
            };
            match result {
                Ok(new) => Done {
                    text: format!("{} {what}", if cut { "Moved" } else { "Copied" }),
                    error: false,
                    select: new
                        .first()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned()),
                },
                Err(err) => Done {
                    text: format!("Could not paste: {}", work::describe(&err)),
                    error: true,
                    select: None,
                },
            }
        });
    }

    pub fn ask_trash(&mut self) {
        let paths = self.targets();
        if !paths.is_empty() {
            self.mode = Mode::Confirm(paths);
        }
    }

    pub fn trash(&mut self, paths: Vec<PathBuf>) {
        let what = describe_items(&paths);
        self.mode = Mode::Normal;
        for p in &paths {
            self.marks.remove(p);
        }
        work::run_op(&self.tx, move || match fsops::trash(&paths) {
            Ok(()) => Done {
                text: format!("Moved {what} to the Trash"),
                error: false,
                select: None,
            },
            Err(err) => Done {
                text: format!("Could not move to the Trash: {}", work::describe(&err)),
                error: true,
                select: None,
            },
        });
    }

    pub fn on_done(&mut self, done: Done) {
        self.status = Some(Status {
            text: done.text,
            error: done.error,
        });
        if done.select.is_some() {
            self.pending = done.select;
        }
        self.load(self.cwd.clone());
    }

    pub fn start_rename(&mut self) {
        let Some(e) = self.current() else { return };
        let text: Vec<char> = e.name.chars().collect();
        // Like Finder, put the cursor before the extension.
        let cursor = match e.name.rfind('.') {
            Some(dot) if dot > 0 && !e.is_dir => e.name[..dot].chars().count(),
            _ => text.len(),
        };
        self.mode = Mode::Rename(Rename {
            path: e.path.clone(),
            text,
            cursor,
        });
    }

    pub fn commit_rename(&mut self) {
        let Mode::Rename(r) = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        let name: String = r.text.iter().collect();
        let name = name.trim();
        let unchanged = r.path.file_name().is_some_and(|n| n == name);
        if name.is_empty() || unchanged {
            return;
        }
        match fsops::rename(&r.path, name) {
            Ok(new) => self.upsert(Some(&r.path), &new),
            Err(err) => self.fail(format!("Could not rename: {}", work::describe(&err))),
        }
    }

    pub fn new_folder(&mut self) {
        match fsops::new_folder(&self.cwd) {
            Ok(path) => {
                if !self.filter.is_empty() {
                    self.set_filter(String::new());
                }
                self.upsert(None, &path);
                self.start_rename();
            }
            Err(err) => self.fail(format!("Could not make a folder: {}", work::describe(&err))),
        }
    }

    /// Shows a renamed or new item right away instead of waiting for the reload.
    fn upsert(&mut self, old: Option<&Path>, new: &Path) {
        let mut entries = self.list.entries.clone();
        entries.retain(|e| Some(e.path.as_path()) != old && e.path != new);
        if let Ok(e) = Entry::from_path(new) {
            entries.push(e);
        }
        entry::sort(&mut entries, self.sort);
        self.pending = new.file_name().map(|n| n.to_string_lossy().into_owned());
        let list = Rc::new(Listing {
            entries,
            mtime: self.list.mtime,
            sort: self.sort,
        });
        self.dirs.insert(self.cwd.clone(), list.clone());
        self.set_list(list);
    }
}

/// Indices of entries that pass the hidden and name filters. The name filter
/// is a case-insensitive substring match.
pub fn visible(entries: &[Entry], hidden: bool, filter: &str) -> Vec<usize> {
    let needle = filter.to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| hidden || !e.is_hidden())
        .filter(|(_, e)| needle.is_empty() || e.name.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

/// One step of `rows` rows in a grid of `cols` columns holding `len` items.
pub fn grid_step(sel: usize, rows: isize, cols: usize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let target = sel as isize + rows * cols as isize;
    if target < 0 {
        return None;
    }
    let target = target as usize;
    if target < len {
        return Some(target);
    }
    // Into the short last row, if there is one below us.
    let last_row = (len - 1) / cols;
    (rows > 0 && sel / cols < last_row).then_some(len - 1)
}

/// Keeps `sel_row` inside a window of `page` rows starting at `scroll`.
pub fn follow(scroll: usize, sel_row: usize, page: usize, total: usize) -> usize {
    let page = page.max(1);
    let mut scroll = scroll.min(total.saturating_sub(page));
    if sel_row < scroll {
        scroll = sel_row;
    } else if sel_row >= scroll + page {
        scroll = sel_row + 1 - page;
    }
    scroll
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn same_sort(a: Sort, b: Sort) -> bool {
    a.key == b.key && a.reverse == b.reverse && a.dirs_first == b.dirs_first
}

pub fn sort_label(sort: Sort) -> &'static str {
    match (sort.key, sort.reverse) {
        (SortKey::Name, false) => "Name, A to Z",
        (SortKey::Name, true) => "Name, Z to A",
        (SortKey::Kind, false) => "Kind, A to Z",
        (SortKey::Kind, true) => "Kind, Z to A",
        (SortKey::Date, false) => "Date Modified, newest first",
        (SortKey::Date, true) => "Date Modified, oldest first",
        (SortKey::Size, false) => "Size, largest first",
        (SortKey::Size, true) => "Size, smallest first",
    }
}

/// “photo.png” or "3 items".
fn describe_items(paths: &[PathBuf]) -> String {
    match paths {
        [one] => format!(
            "“{}”",
            one.file_name().unwrap_or_default().to_string_lossy()
        ),
        many => format!("{} items", many.len()),
    }
}

/// OSC 52 sets the system clipboard. In tmux it is also wrapped in a
/// passthrough, which works even when tmux's own `set-clipboard` is off.
fn clipboard_osc(text: &str) -> Vec<u8> {
    let seq = format!("\x1b]52;c;{}\x07", B64.encode(text));
    let mut out = seq.clone().into_bytes();
    if std::env::var_os("TMUX").is_some() {
        out.extend_from_slice(b"\x1bPtmux;");
        out.extend_from_slice(seq.replace('\x1b', "\x1b\x1b").as_bytes());
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from("/x").join(name),
            is_dir: false,
            is_symlink: false,
            size: 0,
            modified: None,
            created: None,
            mode: 0o644,
        }
    }

    #[test]
    fn filter_skips_hidden_and_matches_any_case() {
        let entries: Vec<Entry> = ["Photo.png", ".hidden-photo", "notes.txt", "PHOTOS"]
            .into_iter()
            .map(entry)
            .collect();
        assert_eq!(visible(&entries, false, ""), [0, 2, 3]);
        assert_eq!(visible(&entries, false, "photo"), [0, 3]);
        assert_eq!(visible(&entries, true, "photo"), [0, 1, 3]);
    }

    #[test]
    fn grid_moves_like_finder() {
        // 10 items in rows of 4: the last row holds 8 and 9.
        assert_eq!(grid_step(1, 1, 4, 10), Some(5));
        assert_eq!(grid_step(6, 1, 4, 10), Some(9)); // into the short row
        assert_eq!(grid_step(9, 1, 4, 10), None);
        assert_eq!(grid_step(2, -1, 4, 10), None);
        assert_eq!(grid_step(9, -1, 4, 10), Some(5));
    }

    #[test]
    fn follow_keeps_selection_in_view() {
        assert_eq!(follow(0, 3, 5, 20), 0);
        assert_eq!(follow(0, 7, 5, 20), 3);
        assert_eq!(follow(10, 4, 5, 20), 4);
        // Clamped when the list shrank.
        assert_eq!(follow(18, 2, 5, 6), 1);
    }

    #[test]
    fn history_goes_back_and_forward() {
        let dir = std::env::temp_dir().join(format!("lsd-tui-history-{}", std::process::id()));
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = App::new(dir.clone(), None, Sort::default(), false, tx);
        app.go(a.clone());
        app.go(b.clone());
        app.go_back();
        assert_eq!(app.cwd, a);
        app.go_back();
        assert_eq!(app.cwd, dir);
        app.go_forward();
        assert_eq!(app.cwd, a);
        // Going somewhere new drops the forward history.
        app.go(b.clone());
        assert!(app.fwd.is_empty());
        assert_eq!(app.back, [dir.clone(), a]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
