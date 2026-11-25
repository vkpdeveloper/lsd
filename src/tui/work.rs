//! Everything slow happens here, off the UI thread: folder reads, thumbnails,
//! half-block decoding, metadata and file operations. Results come back as
//! [`Msg`]s on the same channel as input, so the UI wakes once per burst.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

use ratatui::crossterm::event::Event;

use crate::entry::{self, Entry, Sort};
use crate::halfblock;
use crate::meta::{self, Info};
use crate::thumb::{Mode, Thumb, Thumbnailer};

pub type Grid = Vec<Vec<halfblock::Cell>>;

pub enum Msg {
    Input(Event),
    Dir(PathBuf, Result<Listing, String>),
    Thumb(ThumbKey, Option<Thumb>),
    Half(HalfKey, Option<Grid>),
    Info(InfoKey, Info),
    Done(Done),
}

/// A folder's entries, hidden ones included, sorted by `sort`.
pub struct Listing {
    pub entries: Vec<Entry>,
    pub mtime: Option<SystemTime>,
    pub sort: Sort,
}

impl Listing {
    pub fn empty(sort: Sort) -> Listing {
        Listing {
            entries: Vec::new(),
            mtime: None,
            sort,
        }
    }
}

pub fn read_listing(path: &Path, sort: Sort) -> Result<Listing, String> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut entries = entry::read_dir(path, true).map_err(|e| describe(&e))?;
    entry::sort(&mut entries, sort);
    Ok(Listing {
        entries,
        mtime,
        sort,
    })
}

pub fn load_dir(tx: &Sender<Msg>, path: PathBuf, sort: Sort) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = read_listing(&path, sort);
        let _ = tx.send(Msg::Dir(path, result));
    });
}

/// "Permission denied", without the "(os error 13)".
pub fn describe(err: &std::io::Error) -> String {
    let text = err.kind().to_string();
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

/// What a file operation reports when it finishes.
pub struct Done {
    pub text: String,
    pub error: bool,
    /// Name to select once the folder reloads.
    pub select: Option<String>,
}

pub fn run_op(tx: &Sender<Msg>, op: impl FnOnce() -> Done + Send + 'static) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(Msg::Done(op()));
    });
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ThumbKey {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
    px: u32,
    mode: Mode,
}

impl ThumbKey {
    fn new(e: &Entry, px: u32, mode: Mode) -> ThumbKey {
        ThumbKey {
            path: e.path.clone(),
            size: e.size,
            modified: e.modified,
            px,
            mode,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HalfKey {
    png: PathBuf,
    cols: u16,
    rows: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InfoKey {
    path: PathBuf,
    modified: Option<SystemTime>,
}

impl InfoKey {
    fn new(e: &Entry) -> InfoKey {
        InfoKey {
            path: e.path.clone(),
            modified: e.modified,
        }
    }
}

enum Job {
    Thumb(ThumbKey, Entry),
    Half(HalfKey),
    Info(InfoKey, Entry),
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum JobKey {
    Thumb(ThumbKey),
    Half(HalfKey),
    Info(InfoKey),
}

impl Job {
    fn key(&self) -> JobKey {
        match self {
            Job::Thumb(k, _) => JobKey::Thumb(k.clone()),
            Job::Half(k) => JobKey::Half(k.clone()),
            Job::Info(k, _) => JobKey::Info(k.clone()),
        }
    }
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// Jobs a worker is running right now, so they are not queued twice.
    busy: HashSet<JobKey>,
}

type Shared = Arc<(Mutex<Queue>, Condvar)>;

fn worker(shared: Shared, thumbs: Thumbnailer, tx: Sender<Msg>) {
    let (lock, cv) = &*shared;
    loop {
        let job = {
            let Ok(mut q) = lock.lock() else { return };
            loop {
                if let Some(job) = q.jobs.pop_front() {
                    q.busy.insert(job.key());
                    break job;
                }
                q = match cv.wait(q) {
                    Ok(q) => q,
                    Err(_) => return,
                };
            }
        };
        let key = job.key();
        let msg = match job {
            Job::Thumb(k, e) => {
                let t = thumbs.get(&e, k.px, k.mode);
                Msg::Thumb(k, t)
            }
            Job::Half(k) => {
                let grid = halfblock::cells(&k.png, k.cols, k.rows);
                Msg::Half(k, grid)
            }
            Job::Info(k, e) => Msg::Info(k, meta::info(&e)),
        };
        if let Ok(mut q) = lock.lock() {
            q.busy.remove(&key);
        }
        if tx.send(msg).is_err() {
            return;
        }
    }
}

pub enum Lookup {
    Ready(Thumb),
    /// The thumbnailer has nothing for this file.
    Failed,
    Pending,
}

/// Caches for slow data, filled by the worker pool. Views ask for what they
/// draw; each frame's requests replace the queue, so work for things that
/// scrolled away or belong to another folder is dropped.
pub struct Loader {
    shared: Shared,
    thumbnailer: Thumbnailer,
    thumbs: HashMap<ThumbKey, Option<Thumb>>,
    halves: HashMap<HalfKey, Option<Rc<Grid>>>,
    halves_used: HashSet<HalfKey>,
    infos: HashMap<InfoKey, Rc<Info>>,
    wanted: Vec<(u8, Job)>,
    wanted_keys: HashSet<JobKey>,
    /// Everything queued by the last frame.
    requested: HashSet<JobKey>,
}

/// Request priorities: lower runs first.
pub const PRIO_INFO: u8 = 0;
pub const PRIO_PREVIEW: u8 = 1;
pub const PRIO_VISIBLE: u8 = 2;
pub const PRIO_AHEAD: u8 = 3;

impl Loader {
    pub fn new(tx: &Sender<Msg>) -> Loader {
        let shared: Shared = Arc::default();
        let thumbnailer = Thumbnailer::new();
        let workers = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(2, 6);
        for _ in 0..workers {
            let (shared, thumbs, tx) = (shared.clone(), thumbnailer.clone(), tx.clone());
            std::thread::spawn(move || worker(shared, thumbs, tx));
        }
        Loader {
            shared,
            thumbnailer,
            thumbs: HashMap::new(),
            halves: HashMap::new(),
            halves_used: HashSet::new(),
            infos: HashMap::new(),
            wanted: Vec::new(),
            wanted_keys: HashSet::new(),
            requested: HashSet::new(),
        }
    }

    pub fn thumb(&mut self, e: &Entry, px: u32, mode: Mode, prio: u8) -> Lookup {
        let key = ThumbKey::new(e, px, mode);
        if let Some(t) = self.thumbs.get(&key) {
            return match t {
                Some(t) => Lookup::Ready(t.clone()),
                None => Lookup::Failed,
            };
        }
        let job = JobKey::Thumb(key.clone());
        // Only look on disk the first time; after that the job is in flight.
        if !self.requested.contains(&job)
            && let Some(t) = self.thumbnailer.cached(e, px, mode)
        {
            self.thumbs.insert(key, Some(t.clone()));
            return Lookup::Ready(t);
        }
        self.want(prio, Job::Thumb(key, e.clone()));
        Lookup::Pending
    }

    /// Queues a thumbnail that is not on screen yet, at the lowest priority.
    pub fn prefetch(&mut self, e: &Entry, px: u32, mode: Mode) {
        let key = ThumbKey::new(e, px, mode);
        if !self.thumbs.contains_key(&key) {
            self.want(PRIO_AHEAD, Job::Thumb(key, e.clone()));
        }
    }

    /// Half-block cells for a PNG at exactly `cols` x `rows`.
    pub fn half(&mut self, png: &Path, cols: u16, rows: u16) -> Option<Rc<Grid>> {
        let key = HalfKey {
            png: png.to_path_buf(),
            cols,
            rows,
        };
        self.halves_used.insert(key.clone());
        if let Some(grid) = self.halves.get(&key) {
            return grid.clone();
        }
        self.want(PRIO_PREVIEW, Job::Half(key));
        None
    }

    pub fn info(&mut self, e: &Entry) -> Option<Rc<Info>> {
        let key = InfoKey::new(e);
        if let Some(info) = self.infos.get(&key) {
            return Some(info.clone());
        }
        self.want(PRIO_INFO, Job::Info(key, e.clone()));
        None
    }

    fn want(&mut self, prio: u8, job: Job) {
        if self.wanted_keys.insert(job.key()) {
            self.wanted.push((prio, job));
        }
    }

    /// Hands this frame's requests to the workers, replacing older ones.
    pub fn commit(&mut self) {
        let mut wanted = std::mem::take(&mut self.wanted);
        wanted.sort_by_key(|(prio, _)| *prio);
        if let Ok(mut q) = self.shared.0.lock() {
            let jobs: VecDeque<Job> = wanted
                .into_iter()
                .map(|(_, job)| job)
                .filter(|job| !q.busy.contains(&job.key()))
                .collect();
            q.jobs = jobs;
            self.shared.1.notify_all();
        }
        self.requested = std::mem::take(&mut self.wanted_keys);

        // Keep memory bounded. Thumbnails are cheap to look up again on disk.
        if self.thumbs.len() > 20_000 {
            self.thumbs.clear();
        }
        if self.infos.len() > 2_000 {
            self.infos.clear();
        }
        let used = std::mem::take(&mut self.halves_used);
        if self.halves.len() > 24 {
            self.halves.retain(|k, _| used.contains(k));
        }
    }

    /// Stores a worker result, or hands back a message that is not ours.
    pub fn receive(&mut self, msg: Msg) -> Option<Msg> {
        match msg {
            Msg::Thumb(key, thumb) => {
                self.thumbs.insert(key, thumb);
                None
            }
            Msg::Half(key, grid) => {
                self.halves.insert(key, grid.map(Rc::new));
                None
            }
            Msg::Info(key, info) => {
                self.infos.insert(key, Rc::new(info));
                None
            }
            other => Some(other),
        }
    }
}
