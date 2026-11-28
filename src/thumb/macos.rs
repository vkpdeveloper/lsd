//! QuickLook thumbnails, generated in process with QLThumbnailGenerator, so
//! they come from the same generators and get the same decorations as Finder.
//!
//! Requests are asynchronous: many are kept in flight and their completion
//! handlers (on QuickLook's own queues) draw the CGImage into RGBA and write
//! the PNG. The calling thread only submits, waits and enforces timeouts.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::objc::{self, Block, Id, Pool, msg};
use super::{Class, Icon, Job, Mode, What};

/// QuickLook has a generator or at least an icon for everything.
pub fn renders(_class: Class, _ext: &str) -> bool {
    true
}

/// Cache name prefix for type icons, which all come from QuickLook.
pub fn icon_kind(_icon: &Icon) -> String {
    "type".into()
}

/// QuickLook runs a few generators at a time; this keeps it busy without
/// letting a long queue eat into later items' timeouts.
const IN_FLIGHT: usize = 16;
const TIMEOUT: Duration = Duration::from_secs(5);

/// `QLThumbnailGenerationRequestRepresentationTypes`.
const REP_ICON: usize = 1;
const REP_ALL: usize = usize::MAX;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct CGRect {
    pub origin: CGSize,
    pub size: CGSize,
}

pub(super) type Ref = *mut c_void;

struct Ql {
    generator: Id,
}

// SAFETY: the shared generator is documented as thread safe.
unsafe impl Send for Ql {}
unsafe impl Sync for Ql {}

/// Loads QuickLookThumbnailing (and with it Foundation) on first use. None if
/// anything is missing.
fn ql() -> Option<&'static Ql> {
    static QL: OnceLock<Option<Ql>> = OnceLock::new();
    QL.get_or_init(|| unsafe { load() }).as_ref()
}

unsafe fn load() -> Option<Ql> {
    if objc::dlopen(
        c"/System/Library/Frameworks/QuickLookThumbnailing.framework/QuickLookThumbnailing",
    )
    .is_null()
    {
        return None;
    }
    cg()?;
    let generator = unsafe { msg!(objc::class(c"QLThumbnailGenerator"), c"sharedGenerator"; Id) };
    (!generator.is_null()).then_some(Ql { generator })
}

/// The CoreGraphics calls that turn a CGImage into a PNG.
pub(super) struct Cg {
    srgb: Ref,
    image_width: unsafe extern "C" fn(Ref) -> usize,
    image_height: unsafe extern "C" fn(Ref) -> usize,
    bitmap_context: unsafe extern "C" fn(Ref, usize, usize, usize, usize, Ref, u32) -> Ref,
    set_interpolation: unsafe extern "C" fn(Ref, i32),
    draw_image: unsafe extern "C" fn(Ref, CGRect, Ref),
    release_context: unsafe extern "C" fn(Ref),
}

// SAFETY: the color space is immutable and thread safe; the rest are function
// pointers.
unsafe impl Send for Cg {}
unsafe impl Sync for Cg {}

/// Loads CoreGraphics on first use. None if anything is missing.
pub(super) fn cg() -> Option<&'static Cg> {
    static CG: OnceLock<Option<Cg>> = OnceLock::new();
    CG.get_or_init(|| unsafe { load_cg() }).as_ref()
}

unsafe fn load_cg() -> Option<Cg> {
    let cg = objc::dlopen(c"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics");
    if cg.is_null() {
        return None;
    }
    unsafe {
        let srgb_name: *const Ref = objc::sym(cg, c"kCGColorSpaceSRGB")?;
        let create_space: unsafe extern "C" fn(Ref) -> Ref =
            objc::sym(cg, c"CGColorSpaceCreateWithName")?;
        let srgb = create_space(*srgb_name);
        if srgb.is_null() {
            return None;
        }
        Some(Cg {
            srgb,
            image_width: objc::sym(cg, c"CGImageGetWidth")?,
            image_height: objc::sym(cg, c"CGImageGetHeight")?,
            bitmap_context: objc::sym(cg, c"CGBitmapContextCreate")?,
            set_interpolation: objc::sym(cg, c"CGContextSetInterpolationQuality")?,
            draw_image: objc::sym(cg, c"CGContextDrawImage")?,
            release_context: objc::sym(cg, c"CGContextRelease")?,
        })
    }
}

/// Where a finished request reports to. Keyed by the token its block carries.
struct Pending {
    job: usize,
    out: PathBuf,
    tx: Sender<(usize, Option<(u32, u32)>)>,
}

fn pending() -> &'static Mutex<HashMap<usize, Pending>> {
    static PENDING: OnceLock<Mutex<HashMap<usize, Pending>>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}

struct Flight {
    token: usize,
    request: Id,
    deadline: Instant,
    grace: bool,
}

/// Renders every job, calling `done` once per job index as results arrive.
/// Returns when all jobs have finished or timed out.
pub fn generate(jobs: &[Job], done: &mut dyn FnMut(usize, Option<(u32, u32)>)) {
    let Some(ql) = ql() else {
        (0..jobs.len()).for_each(|i| done(i, None));
        return;
    };
    static TOKENS: AtomicUsize = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel();
    let mut flights: HashMap<usize, Flight> = HashMap::new();
    let mut next = 0;
    loop {
        while flights.len() < IN_FLIGHT && next < jobs.len() {
            // QuickLook draws only the Books icon for an EPUB.
            if matches!(jobs[next].what, What::Content(Class::Book)) {
                done(next, super::book_thumb(&jobs[next]));
                next += 1;
                continue;
            }
            let token = TOKENS.fetch_add(1, Ordering::Relaxed);
            let pend = Pending {
                job: next,
                out: jobs[next].out.clone(),
                tx: tx.clone(),
            };
            match unsafe { submit(ql, &jobs[next], token, pend) } {
                Some(request) => {
                    let deadline = Instant::now() + TIMEOUT;
                    let flight = Flight {
                        token,
                        request,
                        deadline,
                        grace: false,
                    };
                    flights.insert(next, flight);
                }
                None => done(next, None),
            }
            next += 1;
        }
        let Some(deadline) = flights.values().map(|f| f.deadline).min() else {
            break;
        };
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((job, dims)) => {
                // Results for jobs that already timed out are dropped here.
                if let Some(f) = flights.remove(&job) {
                    unsafe { objc::objc_release(f.request) };
                    done(job, dims);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                let expired: Vec<usize> = flights
                    .iter()
                    .filter(|(_, f)| f.deadline <= now)
                    .map(|(&job, _)| job)
                    .collect();
                for job in expired {
                    let Some(f) = flights.get_mut(&job) else {
                        continue;
                    };
                    let claimed = pending()
                        .lock()
                        .is_ok_and(|mut p| p.remove(&f.token).is_none());
                    if claimed && !f.grace {
                        // The handler is already writing this one; give it a moment.
                        f.grace = true;
                        f.deadline = now + Duration::from_secs(1);
                        continue;
                    }
                    let Some(f) = flights.remove(&job) else {
                        continue;
                    };
                    unsafe {
                        msg!(ql.generator, c"cancelRequest:", f.request => Id; ());
                        objc::objc_release(f.request);
                    }
                    done(job, None);
                }
            }
            // `tx` lives in this frame, so the channel cannot disconnect.
            Err(RecvTimeoutError::Disconnected) => unreachable!(),
        }
    }
}

/// Starts one request and returns it retained, or None if it could not start.
unsafe fn submit(ql: &Ql, job: &Job, token: usize, pend: Pending) -> Option<Id> {
    let _pool = Pool::new();
    unsafe {
        let url = objc::file_url(&job.src, false);
        if url.is_null() {
            return None;
        }
        let (reps, icon_mode) = match job.what {
            What::Content(_) => (REP_ALL, job.mode == Mode::Icon),
            What::Icon(_) => (REP_ICON, true),
        };
        let px = job.px as f64;
        let size = CGSize {
            width: px,
            height: px,
        };
        let request = msg!(objc::class(c"QLThumbnailGenerationRequest"), c"alloc"; Id);
        let request = msg!(request, c"initWithFileAtURL:size:scale:representationTypes:",
            url => Id, size => CGSize, 1.0 => f64, reps => usize; Id);
        if request.is_null() {
            return None;
        }
        msg!(request, c"setIconMode:", icon_mode => bool; ());

        // Registered first: the handler can run before this call returns.
        pending().lock().ok()?.insert(token, pend);
        let block = Block::new(on_done as *const c_void, token);
        msg!(ql.generator, c"generateBestRepresentationForRequest:completionHandler:",
            request => Id, block.as_id() => Id; ());
        Some(request)
    }
}

/// Completion handler: `void (^)(QLThumbnailRepresentation *, NSError *)`.
unsafe extern "C" fn on_done(block: *mut Block, rep: Id, _err: Id) {
    let token = unsafe { (*block).token };
    let Some(pend) = pending().lock().ok().and_then(|mut p| p.remove(&token)) else {
        return;
    };
    let dims = unsafe { save(rep, &pend.out) };
    let _ = pend.tx.send((pend.job, dims));
}

/// Writes the representation's CGImage as a PNG.
unsafe fn save(rep: Id, out: &Path) -> Option<(u32, u32)> {
    if rep.is_null() {
        return None;
    }
    unsafe { save_image(msg!(rep, c"CGImage"; Ref), None, out) }
}

/// Draws a CGImage into straight RGBA, scaled to `size` if given, and writes
/// the PNG.
///
/// # Safety
/// `image` must be null or a CGImage.
pub(super) unsafe fn save_image(
    image: Ref,
    size: Option<(u32, u32)>,
    out: &Path,
) -> Option<(u32, u32)> {
    let cg = cg()?;
    if image.is_null() {
        return None;
    }
    let (w, h) = match size {
        Some((w, h)) => (w as usize, h as usize),
        None => unsafe { ((cg.image_width)(image), (cg.image_height)(image)) },
    };
    if w == 0 || h == 0 {
        return None;
    }
    let mut rgba = vec![0u8; w * h * 4];
    unsafe {
        // kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big: R, G, B, A bytes.
        let ctx = (cg.bitmap_context)(
            rgba.as_mut_ptr().cast(),
            w,
            h,
            8,
            w * 4,
            cg.srgb,
            1 | 4 << 12,
        );
        if ctx.is_null() {
            return None;
        }
        // kCGInterpolationHigh, for pictures drawn smaller than they are.
        (cg.set_interpolation)(ctx, 3);
        let rect = CGRect {
            origin: CGSize {
                width: 0.0,
                height: 0.0,
            },
            size: CGSize {
                width: w as f64,
                height: h as f64,
            },
        };
        (cg.draw_image)(ctx, rect, image);
        (cg.release_context)(ctx);
    }
    unpremultiply(&mut rgba);
    super::save_png(out, w as u32, h as u32, &rgba)
}

/// PNG stores straight alpha; CoreGraphics only draws premultiplied.
fn unpremultiply(rgba: &mut [u8]) {
    for [r, g, b, a] in rgba.as_chunks_mut::<4>().0 {
        let a = *a as u32;
        if a != 0 && a != 255 {
            for c in [r, g, b] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}
