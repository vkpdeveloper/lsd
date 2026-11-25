//! Just enough of the Objective-C runtime to call system frameworks.
//!
//! lsd links only libobjc, which costs nothing at launch, and dlopens frameworks
//! the first time it needs them. Linking CoreFoundation or Foundation adds about
//! 1 ms of dyld work to every run (measured: 1.6 ms -> 2.7 ms for an empty
//! binary), and the objc2 crates always link Foundation, so this is hand-rolled
//! to keep the all-cached path free of it.

use std::ffi::{CStr, CString, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub type Id = *mut c_void;
pub type Sel = *const c_void;

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    pub fn sel_registerName(name: *const c_char) -> Sel;
    pub fn objc_msgSend();
    pub fn objc_release(obj: Id);
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

// From libSystem (libclosure).
unsafe extern "C" {
    static _NSConcreteStackBlock: [*const c_void; 32];
}

/// Sends an Objective-C message with an explicit C signature:
/// `msg!(obj, c"initWithX:y:", x => Id, y => f64; Id)`.
macro_rules! msg {
    ($recv:expr, $sel:literal $(, $arg:expr => $t:ty)* ; $ret:ty) => {{
        use $crate::thumb::objc::{Id, Sel, objc_msgSend, sel_registerName};
        let send: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        send($recv, sel_registerName($sel.as_ptr()) $(, $arg)*)
    }};
}
pub(crate) use msg;

/// A class by name, or null when its framework is not loaded.
pub fn class(name: &CStr) -> Id {
    unsafe { objc_getClass(name.as_ptr()) }
}

/// Loads a framework (or returns the already loaded one). Null on failure.
pub fn dlopen(path: &CStr) -> *mut c_void {
    unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY) }
}

/// Looks up a function or data symbol. `T` must be a pointer-sized type such
/// as an `extern "C" fn`.
///
/// # Safety
/// `T` must match the real signature of the symbol.
pub unsafe fn sym<T: Copy>(lib: *mut c_void, name: &CStr) -> Option<T> {
    const { assert!(size_of::<T>() == size_of::<*mut c_void>()) };
    let p = unsafe { libc::dlsym(lib, name.as_ptr()) };
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy(&p) })
}

/// Drains autoreleased objects created while it is alive.
pub struct Pool(*mut c_void);

impl Pool {
    pub fn new() -> Pool {
        Pool(unsafe { objc_autoreleasePoolPush() })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}

/// An autoreleased `NSURL` for an absolute path, or null.
///
/// # Safety
/// Foundation must be loaded.
pub unsafe fn file_url(path: &Path, is_dir: bool) -> Id {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
        return std::ptr::null_mut();
    };
    unsafe {
        msg!(class(c"NSURL"),
            c"fileURLWithFileSystemRepresentation:isDirectory:relativeToURL:",
            c.as_ptr() => *const c_char, is_dir => bool, std::ptr::null_mut() => Id; Id)
    }
}

/// An autoreleased `NSString`.
///
/// # Safety
/// Foundation must be loaded.
pub unsafe fn nsstring(s: &CStr) -> Id {
    unsafe { msg!(class(c"NSString"), c"stringWithUTF8String:", s.as_ptr() => *const c_char; Id) }
}

/// Copies an `NSString` into Rust. None for nil.
///
/// # Safety
/// `obj` must be nil or an `NSString`.
pub unsafe fn string(obj: Id) -> Option<String> {
    if obj.is_null() {
        return None;
    }
    let p = unsafe { msg!(obj, c"UTF8String"; *const c_char) };
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// True when `obj` is an instance of the named class (or a subclass).
///
/// # Safety
/// `obj` must be nil or an object.
pub unsafe fn is_kind(obj: Id, name: &CStr) -> bool {
    let cls = class(name);
    !obj.is_null() && !cls.is_null() && unsafe { msg!(obj, c"isKindOfClass:", cls => Id; bool) }
}

#[repr(C)]
struct Descriptor {
    reserved: usize,
    size: usize,
}

static DESCRIPTOR: Descriptor = Descriptor {
    reserved: 0,
    size: size_of::<Block>(),
};

/// A block literal that captures one integer. The callee copies it to the
/// heap with `Block_copy`, which is a plain memcpy of this struct because it
/// has no copy or dispose helpers, so it only has to outlive the call it is
/// passed to.
#[repr(C)]
pub struct Block {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: *const c_void,
    descriptor: &'static Descriptor,
    pub token: usize,
}

impl Block {
    /// `invoke` must be an `unsafe extern "C" fn(*mut Block, ...)` matching the
    /// block type the callee expects.
    pub fn new(invoke: *const c_void, token: usize) -> Block {
        Block {
            isa: (&raw const _NSConcreteStackBlock).cast(),
            flags: 0,
            reserved: 0,
            invoke,
            descriptor: &DESCRIPTOR,
            token,
        }
    }

    pub fn as_id(&self) -> Id {
        (self as *const Block).cast_mut().cast()
    }
}
