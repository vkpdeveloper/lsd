//! Web pages (`.html`) as pictures: the page as a browser draws it, for the
//! preview pane, Quick Look and the gallery, like Finder's preview.
//!
//! macOS draws it with the system WebKit (WKWebView) in a helper: lsd runs
//! itself again with [`HELPER`], because WebKit needs the main thread and its
//! run loop, which the browser's UI owns. That costs about 0.4 s. Linux drives
//! an installed headless Chromium-family browser, about 0.45 s with
//! chrome-headless-shell and 0.8 s with Chrome.
//!
//! Rendering never reaches the network: a page someone sent you must not call
//! out just because you selected it. Only the page and other local files load,
//! so pages that pull scripts or fonts from the web show without them.
//! - macOS: a content rule list blocks every load that is not a `file:`,
//!   `data:`, `blob:` or `about:` URL, which covers navigations, fetch,
//!   WebSocket and beacons. File reads are limited to the page's folder, the
//!   data store keeps nothing, and WebRTC (which sends UDP that rule lists do
//!   not see) is switched off. If WebKit ever drops that private switch,
//!   scripts are switched off instead.
//! - Linux: the browser runs in a new network namespace with no interfaces up
//!   (`unshare --net`) when the system allows it, and always with a proxy that
//!   is not there, names that never resolve and WebRTC kept off UDP.

use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;

/// The page is laid out in a window this big (CSS pixels), a desktop width.
/// Pictures keep its shape, so they are 4:5.
const VIEW_W: u32 = 1200;
const VIEW_H: u32 = 1500;

/// First argument that makes lsd render one page and exit: `lsd <HELPER>
/// <page.html> <out.png> <px>`. Exit status 0 when the PNG was written, 1 when
/// rendering failed, 2 when this machine has no renderer.
pub const HELPER: &str = "--render-page";

/// Picture size for `px`, the longest side wanted. Chromium sizes its own
/// screenshot from the window and scale.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn size(px: u32) -> (u32, u32) {
    ((px * VIEW_W / VIEW_H).max(1), px)
}

/// True when pages can be rendered here.
pub fn available() -> bool {
    #[cfg(target_os = "macos")]
    return true;
    #[cfg(not(target_os = "macos"))]
    chromium::browser().is_some()
}

/// Renders the page at `src` (absolute) into `out`, no larger than `px`.
/// Blocks for as long as that takes, killed after [`super::TIMEOUT`].
pub fn render(src: &Path, out: &Path, px: u32) -> Option<(u32, u32)> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let exe = std::env::current_exe().ok()?;
        let ok = super::run(
            Command::new(exe)
                .arg(HELPER)
                .arg(src)
                .arg(out)
                .arg(px.to_string()),
        );
        ok.then(|| super::png_size(out)).flatten()
    }
    #[cfg(not(target_os = "macos"))]
    drawn(out, chromium::render(src, out, px))
}

/// Keeps a picture that shows something. A page that drew one flat color,
/// like an app whose scripts come from the web or a page that navigated away,
/// says less than its source, so it counts as a failure.
fn drawn(out: &Path, dims: Option<(u32, u32)>) -> Option<(u32, u32)> {
    let dims = dims?;
    let flat = image::open(out).map(|img| {
        let img = img.to_rgba8();
        let mut pixels = img.pixels();
        let first = pixels.next().copied();
        pixels.all(|p| Some(*p) == first)
    });
    if flat.unwrap_or(true) {
        let _ = std::fs::remove_file(out);
        return None;
    }
    Some(dims)
}

/// Runs the helper: `args` are what follows [`HELPER`].
pub fn helper(args: &[OsString]) -> ExitCode {
    let [src, out, px] = args else {
        return ExitCode::FAILURE;
    };
    let Some(px) = px.to_str().and_then(|p| p.parse().ok()) else {
        return ExitCode::FAILURE;
    };
    if !available() {
        return ExitCode::from(2);
    }
    let Ok(src) = std::path::absolute(src) else {
        return ExitCode::FAILURE;
    };
    #[cfg(target_os = "macos")]
    let done = drawn(Path::new(out), webkit::snapshot(&src, Path::new(out), px));
    #[cfg(not(target_os = "macos"))]
    let done = render(&src, Path::new(out), px);
    match done {
        Some(_) => ExitCode::SUCCESS,
        None => ExitCode::FAILURE,
    }
}

#[cfg(target_os = "macos")]
mod webkit {
    use std::ffi::c_void;
    use std::path::Path;
    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use std::time::{Duration, Instant};

    use super::{VIEW_H, VIEW_W, size};
    use crate::thumb::macos::{CGRect, CGSize, Ref, save_image};
    use crate::thumb::objc::{self, Block, Id, Pool, msg};

    /// Everything that is not local is blocked. WebKit's rule regexes have no
    /// `|`, hence one exception per scheme.
    const RULES: &std::ffi::CStr = cr#"[
        {"trigger": {"url-filter": ".*"}, "action": {"type": "block"}},
        {"trigger": {"url-filter": "^file:"}, "action": {"type": "ignore-previous-rules"}},
        {"trigger": {"url-filter": "^data:"}, "action": {"type": "ignore-previous-rules"}},
        {"trigger": {"url-filter": "^blob:"}, "action": {"type": "ignore-previous-rules"}},
        {"trigger": {"url-filter": "^about:"}, "action": {"type": "ignore-previous-rules"}}
    ]"#;

    /// Time for scripts to finish after the page has loaded (timers, layout
    /// done in script). Local scripts only, so this is short.
    const SETTLE: Duration = Duration::from_millis(100);

    /// Gives up before the parent's kill.
    const DEADLINE: Duration = Duration::from_secs(4);

    // Completion handlers run on the main thread while it pumps the run loop.
    static RULE_LIST: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
    static RULES_DONE: AtomicBool = AtomicBool::new(false);
    static IMAGE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
    static IMAGE_DONE: AtomicBool = AtomicBool::new(false);

    /// `void (^)(WKContentRuleList *, NSError *)`.
    unsafe extern "C" fn on_rules(_block: *mut Block, list: Id, _err: Id) {
        if !list.is_null() {
            unsafe { objc::objc_retain(list) };
        }
        RULE_LIST.store(list, Ordering::SeqCst);
        RULES_DONE.store(true, Ordering::SeqCst);
    }

    /// `void (^)(NSImage *, NSError *)`.
    unsafe extern "C" fn on_image(_block: *mut Block, image: Id, _err: Id) {
        if !image.is_null() {
            unsafe { objc::objc_retain(image) };
        }
        IMAGE.store(image, Ordering::SeqCst);
        IMAGE_DONE.store(true, Ordering::SeqCst);
    }

    struct RunLoop {
        run_in_mode: unsafe extern "C" fn(Ref, f64, bool) -> i32,
        mode: Ref,
        deadline: Instant,
    }

    impl RunLoop {
        /// Runs the main run loop until `done` or the deadline. False on timeout.
        fn until(&self, done: impl Fn() -> bool) -> bool {
            while !done() {
                if Instant::now() >= self.deadline {
                    return false;
                }
                unsafe { (self.run_in_mode)(self.mode, 0.005, true) };
            }
            true
        }
    }

    /// Loads the page in an offscreen WKWebView and writes its picture. Must
    /// run on the main thread.
    pub fn snapshot(src: &Path, out: &Path, px: u32) -> Option<(u32, u32)> {
        let deadline = Instant::now() + DEADLINE;
        let _pool = Pool::new();
        unsafe {
            let cf =
                objc::dlopen(c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation");
            for framework in [
                c"/System/Library/Frameworks/AppKit.framework/AppKit",
                c"/System/Library/Frameworks/WebKit.framework/WebKit",
            ] {
                if objc::dlopen(framework).is_null() {
                    return None;
                }
            }
            let mode: *const Ref = objc::sym(cf, c"kCFRunLoopDefaultMode")?;
            let run_loop = RunLoop {
                run_in_mode: objc::sym(cf, c"CFRunLoopRunInMode")?,
                mode: *mode,
                deadline,
            };

            // No Dock icon, no menu bar, never activates.
            let app = msg!(objc::class(c"NSApplication"), c"sharedApplication"; Id);
            msg!(app, c"setActivationPolicy:", 2isize => isize; bool);

            let config = msg!(objc::class(c"WKWebViewConfiguration"), c"new"; Id);
            let store = msg!(objc::class(c"WKWebsiteDataStore"), c"nonPersistentDataStore"; Id);
            msg!(config, c"setWebsiteDataStore:", store => Id; ());
            let prefs = msg!(config, c"preferences"; Id);
            let peer = objc::sel_registerName(c"_setPeerConnectionEnabled:".as_ptr());
            if msg!(prefs, c"respondsToSelector:", peer => objc::Sel; bool) {
                msg!(prefs, c"_setPeerConnectionEnabled:", false => bool; ());
            } else {
                let page_prefs = msg!(config, c"defaultWebpagePreferences"; Id);
                msg!(page_prefs, c"setAllowsContentJavaScript:", false => bool; ());
            }

            // Compiled rule lists live in lsd's cache, not ~/Library/WebKit.
            let rules_dir = objc::file_url(&super::super::cache_dir().join("webkit"), true);
            let rule_store = msg!(objc::class(c"WKContentRuleListStore"), c"storeWithURL:",
                rules_dir => Id; Id);
            if rule_store.is_null() {
                return None;
            }
            let block = Block::new(on_rules as *const c_void, 0);
            msg!(rule_store,
                c"compileContentRuleListForIdentifier:encodedContentRuleList:completionHandler:",
                objc::nsstring(c"lsd-offline-1") => Id, objc::nsstring(RULES) => Id,
                block.as_id() => Id; ());
            run_loop.until(|| RULES_DONE.load(Ordering::SeqCst));
            // Never render without the network blocked.
            let rules = RULE_LIST.load(Ordering::SeqCst);
            if rules.is_null() {
                return None;
            }
            let content = msg!(config, c"userContentController"; Id);
            msg!(content, c"addContentRuleList:", rules => Id; ());

            let frame = CGRect {
                origin: CGSize {
                    width: 0.0,
                    height: 0.0,
                },
                size: CGSize {
                    width: VIEW_W as f64,
                    height: VIEW_H as f64,
                },
            };
            let view = msg!(objc::class(c"WKWebView"), c"alloc"; Id);
            let view =
                msg!(view, c"initWithFrame:configuration:", frame => CGRect, config => Id; Id);
            if view.is_null() {
                return None;
            }
            let url = objc::file_url(src, false);
            let folder = objc::file_url(src.parent()?, true);
            if url.is_null() || folder.is_null() {
                return None;
            }
            msg!(view, c"loadFileURL:allowingReadAccessToURL:", url => Id, folder => Id; Id);
            let loaded = run_loop.until(|| {
                !msg!(view, c"isLoading"; bool) && msg!(view, c"estimatedProgress"; f64) >= 1.0
            });
            if !loaded {
                return None;
            }
            let settled = Instant::now() + SETTLE;
            run_loop.until(|| Instant::now() >= settled);

            let (w, h) = size(px);
            let snap = msg!(objc::class(c"WKSnapshotConfiguration"), c"new"; Id);
            // Points; on a Retina screen the image is twice that, drawn down below.
            let width = msg!(objc::class(c"NSNumber"), c"numberWithDouble:", w as f64 => f64; Id);
            msg!(snap, c"setSnapshotWidth:", width => Id; ());
            let block = Block::new(on_image as *const c_void, 0);
            msg!(view, c"takeSnapshotWithConfiguration:completionHandler:",
                snap => Id, block.as_id() => Id; ());
            run_loop.until(|| IMAGE_DONE.load(Ordering::SeqCst));
            let image = IMAGE.load(Ordering::SeqCst);
            if image.is_null() {
                return None;
            }
            let cg = msg!(image, c"CGImageForProposedRect:context:hints:",
                null_mut::<CGRect>() => *mut CGRect, null_mut() => Id, null_mut() => Id; Ref);
            save_image(cg, Some((w, h)), out)
        }
    }
}

#[cfg(any(not(target_os = "macos"), test))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod chromium {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::OnceLock;

    use super::{VIEW_H, VIEW_W};
    use crate::thumb::{linux::which, png_size, run, save_scaled, tmp_path};

    /// Fastest first: the headless shell skips the full browser's startup.
    const BROWSERS: &[&str] = &[
        "chrome-headless-shell",
        "chromium",
        "chromium-browser",
        "google-chrome-stable",
        "google-chrome",
        "brave-browser",
        "brave",
        "microsoft-edge-stable",
        "microsoft-edge",
    ];

    const FLAGS: &[&str] = &[
        "--headless",
        "--disable-gpu",
        "--hide-scrollbars",
        "--mute-audio",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-extensions",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-default-apps",
        "--disable-sync",
        "--no-pings",
        "--password-store=basic",
        "--disable-features=Translate,OptimizationHints,MediaRouter,AutofillServerCommunication",
        // Every request goes to a proxy that is not there, names never
        // resolve, and WebRTC may not send UDP around the proxy.
        "--proxy-server=socks5://127.0.0.1:9",
        "--proxy-bypass-list=<-loopback>",
        "--host-resolver-rules=MAP * ~NOTFOUND",
        "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
        // Runs timers up to a second ahead without waiting, so scripts that
        // draw after a delay finish. Free when nothing is fetched.
        "--virtual-time-budget=1000",
    ];

    pub fn browser() -> Option<&'static PathBuf> {
        static BROWSER: OnceLock<Option<PathBuf>> = OnceLock::new();
        BROWSER
            .get_or_init(|| BROWSERS.iter().find_map(|name| which(name)))
            .as_ref()
    }

    /// `unshare`, when it can give lsd a network namespace of its own: util-linux
    /// 2.38 or later, with unprivileged user namespaces allowed.
    fn unshare() -> Option<&'static PathBuf> {
        static UNSHARE: OnceLock<Option<PathBuf>> = OnceLock::new();
        UNSHARE
            .get_or_init(|| {
                let path = which("unshare")?;
                Command::new(&path)
                    .args(["--map-current-user", "--net", "true"])
                    .status()
                    .is_ok_and(|s| s.success())
                    .then_some(path)
            })
            .as_ref()
    }

    pub fn render(src: &Path, out: &Path, px: u32) -> Option<(u32, u32)> {
        let browser = browser()?;
        let mut cmd = match unshare() {
            Some(unshare) => {
                let mut cmd = Command::new(unshare);
                cmd.args(["--map-current-user", "--net"]).arg(browser);
                cmd
            }
            None => Command::new(browser),
        };
        let profile = tmp_path(&std::env::temp_dir().join("lsd-page"), "profile");
        let shot = tmp_path(out, "png");
        // Chrome draws no smaller than half scale; `fit` takes it the rest of the way.
        let scale = (px as f64 / VIEW_H as f64).max(0.5);
        cmd.args(FLAGS)
            .arg(format!("--window-size={VIEW_W},{VIEW_H}"))
            .arg(format!("--force-device-scale-factor={scale:.4}"))
            .arg(flag("--user-data-dir=", &profile))
            .arg(flag("--screenshot=", &shot))
            .arg(file_url(src))
            // Over SSH and in some sessions, Chrome waits on the desktop bus
            // before loading anything remote. A page has no use for it.
            .env("DBUS_SESSION_BUS_ADDRESS", "disabled:");
        let ok = run(&mut cmd);
        let _ = std::fs::remove_dir_all(&profile);
        let dims = ok.then(|| fit(&shot, px, out)).flatten();
        let _ = std::fs::remove_file(&shot);
        dims
    }

    fn flag(name: &str, path: &Path) -> OsString {
        let mut s = OsString::from(name);
        s.push(path);
        s
    }

    /// Moves the screenshot into place, scaled down to `px` if needed.
    fn fit(shot: &Path, px: u32, out: &Path) -> Option<(u32, u32)> {
        let (w, h) = png_size(shot)?;
        if w <= px && h <= px {
            std::fs::rename(shot, out).ok()?;
            return Some((w, h));
        }
        save_scaled(image::open(shot).ok()?, px, out)
    }

    /// `file://` URL for an absolute path, with everything but unreserved
    /// characters and `/` percent-encoded, so names with spaces, `#` or `?`
    /// load the right file.
    pub fn file_url(path: &Path) -> String {
        use std::os::unix::ffi::OsStrExt;
        let mut url = String::from("file://");
        for &b in path.as_os_str().as_bytes() {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                url.push(b as char);
            } else {
                url.push_str(&format!("%{b:02X}"));
            }
        }
        url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_escape_what_would_change_the_path() {
        assert_eq!(
            chromium::file_url(Path::new("/home/me/My Site/#1 ünï?.html")),
            "file:///home/me/My%20Site/%231%20%C3%BCn%C3%AF%3F.html"
        );
    }

    #[test]
    fn pictures_keep_the_page_shape() {
        assert_eq!(size(1500), (1200, 1500));
        assert_eq!(size(512), (409, 512));
    }
}
