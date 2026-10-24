//! The hook chain: player's `GetProcAddress` -> `mono_jit_init_version` -> `FastBoot.Early.Bootstrap.Start`.

use crate::{boot_reads, counters, iat, jitprofile, log, win};
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

type GetProcAddressFn = unsafe extern "system" fn(win::Hmodule, *const c_char) -> *mut c_void;
type JitInitFn = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void;

static REAL_GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static REAL_JIT_INIT: AtomicUsize = AtomicUsize::new(0);
static TARGET: AtomicUsize = AtomicUsize::new(0);
static MONO: AtomicUsize = AtomicUsize::new(0);
static CONFIG: OnceLock<Config> = OnceLock::new();

/// Where things are. Environment overrides exist for the offline tests, which run the same chain under
/// monohost instead of Unity's player.
struct Config {
    /// KSP folder, passed to Bootstrap.Start. `FASTBOOT_EARLY_ROOT`, else the executable's folder.
    root: PathBuf,
    /// `FASTBOOT_EARLY_ASSEMBLY`, else `<root>/GameData/FastBoot/Plugins/FastBoot.dll`.
    assembly: PathBuf,
    /// Module whose imports are hooked. `FASTBOOT_EARLY_TARGET`, else `UnityPlayer.dll`.
    target: String,
    /// `MonoCounters = true` in FastBoot.cfg (or `FASTBOOT_MONO_COUNTERS=1`): record Mono's runtime counters.
    mono_counters: bool,
}

impl Config {
    unsafe fn from_env() -> Option<Config> {
        let root = match std::env::var_os("FASTBOOT_EARLY_ROOT") {
            Some(r) => PathBuf::from(r),
            None => win::module_path(std::ptr::null_mut())?.parent()?.to_path_buf(),
        };
        let assembly = match std::env::var_os("FASTBOOT_EARLY_ASSEMBLY") {
            Some(a) => PathBuf::from(a),
            None => root.join("GameData").join("FastBoot").join("Plugins").join("FastBoot.dll"),
        };
        let target = std::env::var("FASTBOOT_EARLY_TARGET").unwrap_or_else(|_| "UnityPlayer.dll".into());
        let cfg = assembly.parent().and_then(Path::parent).unwrap_or(&root).join("FastBoot.cfg");
        let mono_counters = std::env::var("FASTBOOT_MONO_COUNTERS").is_ok_and(|v| v == "1")
            || fastboot_warm::settings::read_bool(&cfg, "MonoCounters", false);
        Some(Config { root, assembly, target, mono_counters })
    }

    /// GameData/FastBoot/PluginData, next to the assembly's Plugins folder.
    fn plugin_data(&self) -> PathBuf {
        let home = self.assembly.parent().and_then(Path::parent).unwrap_or(&self.root);
        home.join("PluginData")
    }

    fn disabled(&self) -> Option<String> {
        if std::env::var("FASTBOOT_EARLY_DISABLE").is_ok_and(|v| !v.is_empty() && v != "0") {
            return Some("FASTBOOT_EARLY_DISABLE is set".into());
        }
        let marker = self.plugin_data().join("disable-early");
        if marker.exists() {
            return Some(format!("{} exists", marker.display()));
        }
        if !self.assembly.is_file() {
            return Some(format!("{} not found (FastBoot is not installed)", self.assembly.display()));
        }
        None
    }
}

/// Runs under the loader lock from DllMain: reads the environment and one settings file, opens the log, starts
/// the warm-up thread and rewrites import slots.
pub unsafe fn attach() {
    let Some(cfg) = Config::from_env() else { return };
    // Other programs in the KSP folder load this file too (Unity's crash handler imports VERSION.dll). Outside the
    // game, behave exactly like the system version.dll: no log, no warm-up, no hooks.
    let target = win::GetModuleHandleW(win::wide(cfg.target.as_ref()).as_ptr());
    if target.is_null() {
        return;
    }
    log::open(&cfg.plugin_data().join("early.log"));
    log::line(&format!("FastBoot early start {}; KSP folder {}", env!("CARGO_PKG_VERSION"), cfg.root.display()));
    if let Some(why) = cfg.disabled() {
        log::line(&format!("off: {}", why));
        return;
    }
    boot_reads::start(&cfg.root, &cfg.plugin_data());
    boot_reads::record_opens_in(target, &cfg.target);
    let gpa = win::kernel32_get_proc_address();
    REAL_GET_PROC_ADDRESS.store(gpa, Ordering::Release);
    TARGET.store(target as usize, Ordering::Release);
    let _ = CONFIG.set(cfg);
    match iat::redirect(target as *const u8, gpa, get_proc_address as usize) {
        Ok(0) => log::line("off: the target does not import kernel32's GetProcAddress"),
        Ok(n) => log::line(&format!("waiting for mono_jit_init_version ({} import slot(s) hooked)", n)),
        Err(e) => log::line(&format!("off: could not hook the import table: {}", e)),
    }
}

/// Mono's module, once the player asked for `mono_jit_init_version`; null before.
pub(crate) fn mono_module() -> win::Hmodule {
    MONO.load(Ordering::Acquire) as win::Hmodule
}

pub(crate) unsafe fn real_get_proc_address() -> GetProcAddressFn {
    std::mem::transmute(REAL_GET_PROC_ADDRESS.load(Ordering::Acquire))
}

/// Stands in for kernel32's GetProcAddress in the target's import table until Mono is initialised.
unsafe extern "system" fn get_proc_address(module: win::Hmodule, name: *const c_char) -> *mut c_void {
    let p = real_get_proc_address()(module, name);
    // Names below 0x10000 are ordinals, not strings.
    if p.is_null() || (name as usize) >> 16 == 0 || CStr::from_ptr(name).to_bytes() != b"mono_jit_init_version" {
        return p;
    }
    REAL_JIT_INIT.store(p as usize, Ordering::Release);
    MONO.store(module as usize, Ordering::Release);
    log::line("intercepted mono_jit_init_version");
    // Mono was just loaded: from here on it opens every managed assembly and every file the game reads.
    boot_reads::record_opens_in(module, "Mono");
    jit_init_version as *mut c_void
}

unsafe extern "C" fn jit_init_version(name: *const c_char, version: *const c_char) -> *mut c_void {
    let real: JitInitFn = std::mem::transmute(REAL_JIT_INIT.load(Ordering::Acquire));
    // Mono registers its counters while it initialises and skips sections not enabled by then.
    if CONFIG.get().is_some_and(|c| c.mono_counters) {
        if counters::enable(mono_module(), real_get_proc_address()) {
            log::line("Mono counters enabled");
        }
        if jitprofile::install(mono_module(), real_get_proc_address()) {
            log::line("JIT profiler callbacks installed");
        }
    }
    let domain = real(name, version);
    unhook();
    if domain.is_null() {
        log::line("mono_jit_init_version failed; not starting FastBoot");
        return domain;
    }
    match std::panic::catch_unwind(|| start_fastboot(domain)) {
        Ok(Ok(())) => log::line("FastBoot.Early.Bootstrap.Start returned; see KSP.log for [FastBoot] lines"),
        Ok(Err(e)) => log::line(&format!("not started: {}", e)),
        Err(_) => log::line("not started: internal error (panic)"),
    }
    domain
}

/// Puts the target's import slots back to kernel32's GetProcAddress.
unsafe fn unhook() {
    let target = TARGET.load(Ordering::Acquire) as *const u8;
    match iat::redirect(target, get_proc_address as usize, REAL_GET_PROC_ADDRESS.load(Ordering::Acquire)) {
        Ok(n) => log::line(&format!("import table restored ({} slot(s))", n)),
        Err(e) => log::line(&format!("could not restore the import table: {}", e)),
    }
}

/// The part of Mono's embedding API used here.
struct MonoApi {
    assembly_open: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    get_image: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    class_from_name: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> *mut c_void,
    method_from_name: unsafe extern "C" fn(*mut c_void, *const c_char, i32) -> *mut c_void,
    string_new: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    runtime_invoke: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut *mut c_void, *mut *mut c_void) -> *mut c_void,
}

impl MonoApi {
    unsafe fn resolve(module: win::Hmodule) -> Result<MonoApi, String> {
        let get = |name: &CStr| -> Result<*mut c_void, String> {
            let p = real_get_proc_address()(module, name.as_ptr());
            if p.is_null() {
                Err(format!("Mono has no export {}", name.to_string_lossy()))
            } else {
                Ok(p)
            }
        };
        Ok(MonoApi {
            assembly_open: std::mem::transmute(get(c"mono_domain_assembly_open")?),
            get_image: std::mem::transmute(get(c"mono_assembly_get_image")?),
            class_from_name: std::mem::transmute(get(c"mono_class_from_name")?),
            method_from_name: std::mem::transmute(get(c"mono_class_get_method_from_name")?),
            string_new: std::mem::transmute(get(c"mono_string_new")?),
            runtime_invoke: std::mem::transmute(get(c"mono_runtime_invoke")?),
        })
    }
}

fn utf8(p: &Path) -> Result<CString, String> {
    let s = p.to_str().ok_or_else(|| format!("{} is not valid Unicode", p.display()))?;
    CString::new(s).map_err(|_| format!("{} contains a NUL", p.display()))
}

/// Opens FastBoot.dll in the root domain (the same path KSP loads it from later, so KSP gets this same
/// assembly) and calls Bootstrap.Start(kspRoot).
unsafe fn start_fastboot(domain: *mut c_void) -> Result<(), String> {
    let cfg = CONFIG.get().ok_or("no configuration")?;
    let mono = MonoApi::resolve(MONO.load(Ordering::Acquire) as win::Hmodule)?;
    let path = utf8(&cfg.assembly)?;
    let assembly = (mono.assembly_open)(domain, path.as_ptr());
    if assembly.is_null() {
        return Err(format!("Mono could not open {}", cfg.assembly.display()));
    }
    let image = (mono.get_image)(assembly);
    let class = (mono.class_from_name)(image, c"FastBoot.Early".as_ptr(), c"Bootstrap".as_ptr());
    if class.is_null() {
        return Err("FastBoot.dll has no FastBoot.Early.Bootstrap (version mismatch with version.dll?)".into());
    }
    let start = (mono.method_from_name)(class, c"Start".as_ptr(), 1);
    if start.is_null() {
        return Err("FastBoot.Early.Bootstrap has no Start(string)".into());
    }
    let root = utf8(&cfg.root)?;
    // Reference-type arguments are passed as the object pointer itself.
    let mut args = [(mono.string_new)(domain, root.as_ptr())];
    let mut exception: *mut c_void = std::ptr::null_mut();
    (mono.runtime_invoke)(start, std::ptr::null_mut(), args.as_mut_ptr(), &mut exception);
    if !exception.is_null() {
        return Err("FastBoot.Early.Bootstrap.Start threw".into());
    }
    log::line(&format!("started {}", cfg.assembly.display()));
    Ok(())
}
