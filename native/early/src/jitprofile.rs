//! Which assemblies the boot's JIT time goes to, through Mono's profiler API: a `jit_begin` / `jit_done` callback
//! pair per compiled method, attributed to the method's image (assembly). Time is exclusive: a compile that runs
//! managed code which compiles other methods (a static constructor, say) is charged only its own part.
//!
//! This decides what to precompile first: KSP's own Assembly-CSharp, Unity's and Mono's libraries, or the mods.

use crate::win;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

type Gpa = unsafe extern "system" fn(win::Hmodule, *const c_char) -> *mut c_void;
type MethodCb = unsafe extern "C" fn(*mut c_void, *mut c_void);
type DoneCb = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void);

#[derive(Default, Clone, Copy)]
struct Totals {
    methods: u64,
    nanos: u64,
}

/// Image pointer -> totals. Images live for the whole process, so the pointer is a stable key.
static TOTALS: Mutex<Option<HashMap<usize, Totals>>> = Mutex::new(None);
static METHOD_GET_CLASS: AtomicUsize = AtomicUsize::new(0);
static CLASS_GET_IMAGE: AtomicUsize = AtomicUsize::new(0);
static IMAGE_GET_NAME: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Compiles in progress on this thread: (start, time spent in nested compiles).
    static STACK: RefCell<Vec<(Instant, u64)>> = const { RefCell::new(Vec::new()) };
}

unsafe fn get(mono: win::Hmodule, gpa: Gpa, name: &CStr) -> usize {
    gpa(mono, name.as_ptr()) as usize
}

/// Registers the callbacks. Call before `mono_jit_init_version`, so nothing compiled during startup is missed.
pub unsafe fn install(mono: win::Hmodule, gpa: Gpa) -> bool {
    let create = get(mono, gpa, c"mono_profiler_create");
    let set_begin = get(mono, gpa, c"mono_profiler_set_jit_begin_callback");
    let set_done = get(mono, gpa, c"mono_profiler_set_jit_done_callback");
    let set_failed = get(mono, gpa, c"mono_profiler_set_jit_failed_callback");
    METHOD_GET_CLASS.store(get(mono, gpa, c"mono_method_get_class"), Ordering::Release);
    CLASS_GET_IMAGE.store(get(mono, gpa, c"mono_class_get_image"), Ordering::Release);
    IMAGE_GET_NAME.store(get(mono, gpa, c"mono_image_get_name"), Ordering::Release);
    if [create, set_begin, set_done, set_failed].contains(&0)
        || [&METHOD_GET_CLASS, &CLASS_GET_IMAGE, &IMAGE_GET_NAME].iter().any(|f| f.load(Ordering::Acquire) == 0)
    {
        return false;
    }
    if let Ok(mut t) = TOTALS.lock() {
        *t = Some(HashMap::new());
    }
    let create: unsafe extern "C" fn(*mut c_void) -> *mut c_void = std::mem::transmute(create);
    let set_begin: unsafe extern "C" fn(*mut c_void, MethodCb) = std::mem::transmute(set_begin);
    let set_done: unsafe extern "C" fn(*mut c_void, DoneCb) = std::mem::transmute(set_done);
    let set_failed: unsafe extern "C" fn(*mut c_void, MethodCb) = std::mem::transmute(set_failed);
    // The profiler struct is opaque to Mono; this one carries nothing.
    let handle = create(std::ptr::null_mut());
    if handle.is_null() {
        return false;
    }
    set_begin(handle, on_begin);
    set_done(handle, on_done);
    set_failed(handle, on_failed);
    true
}

unsafe extern "C" fn on_begin(_prof: *mut c_void, _method: *mut c_void) {
    let _ = STACK.try_with(|s| s.borrow_mut().push((Instant::now(), 0)));
}

unsafe extern "C" fn on_failed(_prof: *mut c_void, _method: *mut c_void) {
    let _ = STACK.try_with(|s| s.borrow_mut().pop());
}

unsafe extern "C" fn on_done(_prof: *mut c_void, method: *mut c_void, _jinfo: *mut c_void) {
    let Ok(Some(nanos)) = STACK.try_with(|s| {
        let mut s = s.borrow_mut();
        let (start, nested) = s.pop()?;
        let total = start.elapsed().as_nanos() as u64;
        if let Some(parent) = s.last_mut() {
            parent.1 += total;
        }
        Some(total.saturating_sub(nested))
    }) else {
        return;
    };
    let get_class: unsafe extern "C" fn(*mut c_void) -> *mut c_void = std::mem::transmute(METHOD_GET_CLASS.load(Ordering::Acquire));
    let get_image: unsafe extern "C" fn(*mut c_void) -> *mut c_void = std::mem::transmute(CLASS_GET_IMAGE.load(Ordering::Acquire));
    let class = get_class(method);
    let image = if class.is_null() { 0 } else { get_image(class) as usize };
    if let Ok(mut t) = TOTALS.lock() {
        if let Some(map) = t.as_mut() {
            let e = map.entry(image).or_default();
            e.methods += 1;
            e.nanos += nanos;
        }
    }
}

/// "assembly: methods, seconds" lines, most expensive first, plus a total line. Empty if not installed.
pub unsafe fn report() -> Vec<String> {
    let Ok(t) = TOTALS.lock() else { return Vec::new() };
    let Some(map) = t.as_ref() else { return Vec::new() };
    let name_of: unsafe extern "C" fn(*mut c_void) -> *const c_char = std::mem::transmute(IMAGE_GET_NAME.load(Ordering::Acquire));
    let mut rows: Vec<(String, Totals)> = map
        .iter()
        .map(|(&image, &tot)| {
            let name = if image == 0 {
                "(no image)".to_string()
            } else {
                let n = name_of(image as *mut c_void);
                if n.is_null() { "?".into() } else { CStr::from_ptr(n).to_string_lossy().into_owned() }
            };
            (name, tot)
        })
        .collect();
    rows.sort_by(|a, b| b.1.nanos.cmp(&a.1.nanos));
    let methods: u64 = rows.iter().map(|r| r.1.methods).sum();
    let nanos: u64 = rows.iter().map(|r| r.1.nanos).sum();
    let mut out = vec![format!("JIT by assembly: {} methods, {:.3} s exclusive, {} assemblies", methods, nanos as f64 / 1e9, rows.len())];
    for (name, t) in rows {
        out.push(format!("JIT by assembly: {:>8.3} s {:>6} methods  {}", t.nanos as f64 / 1e9, t.methods, name));
    }
    out
}
