//! The interactive browser (`lsd -i`): a Finder window in the terminal.
//!
//! One thread draws. Input, folder reads, thumbnails and metadata arrive as
//! messages on one channel; each burst of messages leads to at most one frame,
//! and nothing redraws unless something changed.

mod app;
mod input;
mod pics;
mod theme;
mod ui;
mod views;
mod work;

use std::io::{self, BufWriter, Stdout, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::Context;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::terminal::{
    self as cterm, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{cursor, execute};
use ratatui::layout::Rect;

use crate::entry::Sort;
use crate::glyph::Icons;
use crate::term::Term;
use app::App;
use pics::Pics;
use theme::Theme;
use work::{Loader, Msg};

pub struct Options {
    pub start: PathBuf,
    pub show_hidden: bool,
    pub sort: Sort,
    /// When set, the directory the user ended in is written here on quit, so a
    /// shell function can `cd` to it.
    pub cwd_file: Option<PathBuf>,
}

type Backend = CrosstermBackend<BufWriter<Stdout>>;

pub fn run(term: &Term, opts: Options) -> anyhow::Result<()> {
    let start =
        std::fs::canonicalize(&opts.start).with_context(|| format!("{}", opts.start.display()))?;
    let (dir, select) = if start.is_dir() {
        (start, None)
    } else {
        let name = start.file_name().map(|n| n.to_string_lossy().into_owned());
        (start.parent().map(PathBuf::from).unwrap_or(start), name)
    };

    // Before raw mode and before anything reads input.
    let theme = Theme::detect(term.truecolor);

    let (tx, rx) = mpsc::channel();
    let app = App::new(dir, select, opts.sort, opts.show_hidden, tx.clone());
    let load = Loader::new(&tx);
    let input_tx = tx.clone();
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if input_tx.send(Msg::Input(ev)).is_err() {
                break;
            }
        }
    });

    let mut terminal = setup()?;
    let mut tui = Tui {
        app,
        load,
        pics: Pics::new(term),
        theme,
        icons: Icons::resolve(None),
        clicks: input::Clicks::default(),
        area: Rect::default(),
        frames: Vec::new(),
    };
    let result = tui.run(&mut terminal, rx);
    let cleanup = tui.pics.cleanup();
    restore(&mut terminal, &cleanup);

    if std::env::var_os("LSD_FRAME_STATS").is_some() {
        tui.report_frames();
    }
    if let Some(file) = &opts.cwd_file {
        std::fs::write(file, tui.app.cwd.as_os_str().as_encoded_bytes())
            .with_context(|| format!("{}", file.display()))?;
    }
    result
}

fn setup() -> anyhow::Result<Terminal<Backend>> {
    enable_raw_mode()?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            cursor::Show
        );
        let _ = disable_raw_mode();
        hook(info);
    }));
    let mut out = BufWriter::with_capacity(1 << 16, io::stdout());
    execute!(out, EnterAlternateScreen, EnableMouseCapture, cursor::Hide)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

fn restore(terminal: &mut Terminal<Backend>, cleanup: &[u8]) {
    let out = terminal.backend_mut();
    let _ = out.write_all(cleanup);
    let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen, cursor::Show);
    let _ = disable_raw_mode();
}

struct Tui {
    app: App,
    load: Loader,
    pics: Pics,
    theme: Theme,
    icons: Icons,
    clicks: input::Clicks,
    /// Size of the last frame, to notice resizes.
    area: Rect,
    /// Time to build each frame, and to build and write it.
    frames: Vec<(Duration, Duration)>,
}

const TICK: Duration = Duration::from_secs(1);

impl Tui {
    fn run(&mut self, terminal: &mut Terminal<Backend>, rx: Receiver<Msg>) -> anyhow::Result<()> {
        let mut dirty = true;
        let mut next_tick = Instant::now() + TICK;
        loop {
            let now = Instant::now();
            let wait = match (dirty, self.app.defer) {
                (true, Some(until)) if until > now => until - now,
                (true, _) => Duration::ZERO,
                (false, _) => next_tick.saturating_duration_since(now),
            };
            match rx.recv_timeout(wait) {
                Ok(msg) => dirty |= self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            while let Ok(msg) = rx.try_recv() {
                dirty |= self.handle(msg);
            }
            if self.app.quit {
                break;
            }
            let now = Instant::now();
            if now >= next_tick {
                dirty |= self.app.tick();
                next_tick = now + TICK;
            }
            if self.app.defer.is_some_and(|until| until <= now) {
                self.app.defer = None;
            }
            if dirty && self.app.defer.is_none() {
                self.draw(terminal)?;
                dirty = false;
            }
        }
        Ok(())
    }

    fn handle(&mut self, msg: Msg) -> bool {
        let Some(msg) = self.load.receive(msg) else {
            return true;
        };
        match msg {
            Msg::Input(ev) => input::handle(&mut self.app, &mut self.clicks, ev),
            Msg::Dir(path, result) => {
                self.app.on_dir(path, result);
                true
            }
            Msg::Done(done) => {
                self.app.on_done(done);
                true
            }
            Msg::Thumb(..) | Msg::Half(..) | Msg::Info(..) => true,
        }
    }

    fn draw(&mut self, terminal: &mut Terminal<Backend>) -> io::Result<()> {
        let started = Instant::now();
        terminal.autoresize()?;
        let mut frame = terminal.get_frame();
        let area = frame.area();
        if area != self.area {
            // A resize clears the screen, and the cell size may have changed too.
            self.area = area;
            self.pics.invalidate();
            if let Ok(size) = cterm::window_size()
                && size.width > 0
                && size.columns > 0
            {
                self.pics.cell = (size.width / size.columns, size.height / size.rows.max(1));
            }
        }
        let buf = frame.buffer_mut();
        let mut cx = ui::Ctx {
            th: &self.theme,
            pics: &mut self.pics,
            load: &mut self.load,
            icons: self.icons,
        };
        ui::draw(buf, area, &mut self.app, &mut cx);
        let post = self.pics.finish(buf);
        let built = started.elapsed();

        let out = terminal.backend_mut();
        out.write_all(&std::mem::take(&mut self.app.osc))?;
        out.write_all(&self.pics.take_pre())?;
        terminal.flush()?;
        terminal.backend_mut().write_all(&post)?;
        terminal.swap_buffers();
        terminal.backend_mut().flush()?;

        self.load.commit();
        for path in std::mem::take(&mut self.app.want_dirs) {
            self.app.load(path);
        }
        self.frames.push((built, started.elapsed()));
        Ok(())
    }

    /// Writing can block on a slow terminal, so it is reported apart from our
    /// own work.
    fn report_frames(&self) {
        if self.frames.is_empty() {
            return;
        }
        let (built, total): (Vec<_>, Vec<_>) = self.frames.iter().copied().unzip();
        eprintln!("lsd: {} frames", self.frames.len());
        for (name, mut times) in [("build", built), ("build and write", total)] {
            times.sort();
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            let at = |q: f64| ms(times[((times.len() - 1) as f64 * q) as usize]);
            let sum: Duration = times.iter().sum();
            eprintln!(
                "  {name}: mean {:.2} ms, median {:.2} ms, p95 {:.2} ms, max {:.2} ms",
                ms(sum) / times.len() as f64,
                at(0.5),
                at(0.95),
                at(1.0),
            );
        }
    }
}
