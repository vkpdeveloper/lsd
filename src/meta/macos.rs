//! Finder's kind string from LaunchServices and details from Spotlight, with
//! ImageIO and CGPDFDocument for what Spotlight does not know (unindexed
//! folders such as /tmp, or attributes its importer left empty). Frameworks
//! are loaded on first use, see `thumb::objc`.

use std::ffi::{CStr, c_void};
use std::sync::OnceLock;
use std::time::{Duration, UNIX_EPOCH};

use super::Info;
use crate::entry::Entry;
use crate::thumb::objc::{self, Id, Pool, msg};
use crate::thumb::{Class, class_of};

type Ref = *mut c_void;

struct Libs {
    /// `NSURLLocalizedTypeDescriptionKey`.
    kind_key: Id,
    cf_release: unsafe extern "C" fn(Ref),
    image_source: unsafe extern "C" fn(Ref, Ref) -> Ref,
    image_properties: unsafe extern "C" fn(Ref, usize, Ref) -> Ref,
    pdf_open: unsafe extern "C" fn(Ref) -> Ref,
    pdf_pages: unsafe extern "C" fn(Ref) -> usize,
    pdf_release: unsafe extern "C" fn(Ref),
}

// SAFETY: an immutable constant string and function pointers.
unsafe impl Send for Libs {}
unsafe impl Sync for Libs {}

fn libs() -> Option<&'static Libs> {
    static LIBS: OnceLock<Option<Libs>> = OnceLock::new();
    LIBS.get_or_init(|| unsafe { load() }).as_ref()
}

unsafe fn load() -> Option<Libs> {
    let open = |path: &CStr| Some(objc::dlopen(path)).filter(|h| !h.is_null());
    let foundation = open(c"/System/Library/Frameworks/Foundation.framework/Foundation")?;
    let cf = open(c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")?;
    let imageio = open(c"/System/Library/Frameworks/ImageIO.framework/ImageIO")?;
    let cg = open(c"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")?;
    unsafe {
        let kind_key: *const Id = objc::sym(foundation, c"NSURLLocalizedTypeDescriptionKey")?;
        Some(Libs {
            kind_key: *kind_key,
            cf_release: objc::sym(cf, c"CFRelease")?,
            image_source: objc::sym(imageio, c"CGImageSourceCreateWithURL")?,
            image_properties: objc::sym(imageio, c"CGImageSourceCopyPropertiesAtIndex")?,
            pdf_open: objc::sym(cg, c"CGPDFDocumentCreateWithURL")?,
            pdf_pages: objc::sym(cg, c"CGPDFDocumentGetNumberOfPages")?,
            pdf_release: objc::sym(cg, c"CGPDFDocumentRelease")?,
        })
    }
}

pub fn fill(entry: &Entry, info: &mut Info) {
    let Some(libs) = libs() else { return };
    // Describe what a link points at, like the rest of the entry does;
    // LaunchServices would call every symlink "Alias".
    let path = if entry.is_symlink {
        std::fs::canonicalize(&entry.path)
    } else {
        std::path::absolute(&entry.path)
    };
    let Ok(path) = path else { return };
    let _pool = Pool::new();
    unsafe {
        let url = objc::file_url(&path, entry.is_dir);
        if url.is_null() {
            return;
        }
        let mut kind: Id = std::ptr::null_mut();
        let found = msg!(url, c"getResourceValue:forKey:error:",
            &mut kind => *mut Id, libs.kind_key => Id, std::ptr::null_mut() => *mut Id; bool);
        if let Some(kind) = found.then(|| objc::string(kind)).flatten() {
            info.kind = kind;
        }
        spotlight(url, info);
        if entry.is_dir {
            return;
        }
        match class_of(&entry.ext()) {
            Some(Class::Image) if info.dimensions.is_none() => {
                info.dimensions = image_size(libs, url);
            }
            Some(Class::Pdf) if info.pages.is_none() => info.pages = pdf_pages(libs, url),
            _ => {}
        }
    }
}

/// Fills what Spotlight has indexed for `url`.
unsafe fn spotlight(url: Id, info: &mut Info) {
    unsafe {
        let item = msg!(objc::class(c"NSMetadataItem"), c"alloc"; Id);
        let item = msg!(item, c"initWithURL:", url => Id; Id);
        if item.is_null() {
            return;
        }
        let value = |name: &CStr| msg!(item, c"valueForAttribute:", objc::nsstring(name) => Id; Id);
        let number = |name: &CStr| {
            let v = value(name);
            objc::is_kind(v, c"NSNumber").then(|| msg!(v, c"doubleValue"; f64))
        };
        if let (Some(w), Some(h)) = (number(c"kMDItemPixelWidth"), number(c"kMDItemPixelHeight"))
            && w >= 1.0
            && h >= 1.0
        {
            info.dimensions = Some((w as u32, h as u32));
        }
        info.pages = number(c"kMDItemNumberOfPages")
            .filter(|&p| p >= 1.0)
            .map(|p| p as u32);
        info.duration =
            number(c"kMDItemDurationSeconds").and_then(|s| Duration::try_from_secs_f64(s).ok());
        let used = value(c"kMDItemLastUsedDate");
        if objc::is_kind(used, c"NSDate") {
            let secs = msg!(used, c"timeIntervalSince1970"; f64);
            info.last_opened = Duration::try_from_secs_f64(secs)
                .ok()
                .map(|d| UNIX_EPOCH + d);
        }
        objc::objc_release(item);
    }
}

/// Pixel size from the image header, as displayed (EXIF quarter turns swap
/// width and height, like Finder).
unsafe fn image_size(libs: &Libs, url: Id) -> Option<(u32, u32)> {
    unsafe {
        let source = (libs.image_source)(url, std::ptr::null_mut());
        if source.is_null() {
            return None;
        }
        let props = (libs.image_properties)(source, 0, std::ptr::null_mut());
        (libs.cf_release)(source);
        if props.is_null() {
            return None;
        }
        // The properties dictionary is toll-free bridged to NSDictionary.
        let get = |key: &CStr| {
            let v = msg!(props, c"objectForKey:", objc::nsstring(key) => Id; Id);
            objc::is_kind(v, c"NSNumber").then(|| msg!(v, c"unsignedIntValue"; u32))
        };
        let size = get(c"PixelWidth").zip(get(c"PixelHeight"));
        let turned = get(c"Orientation").is_some_and(|o| (5..=8).contains(&o));
        (libs.cf_release)(props);
        size.map(|(w, h)| if turned { (h, w) } else { (w, h) })
    }
}

unsafe fn pdf_pages(libs: &Libs, url: Id) -> Option<u32> {
    unsafe {
        let doc = (libs.pdf_open)(url);
        if doc.is_null() {
            return None;
        }
        let pages = (libs.pdf_pages)(doc);
        (libs.pdf_release)(doc);
        u32::try_from(pages).ok().filter(|&p| p > 0)
    }
}
