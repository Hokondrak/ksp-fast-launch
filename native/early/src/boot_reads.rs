//! Cold-start warm-up and the read list it learns from.
//!
//! At process start a background warm-up ([`fastboot_warm`]) reads the files the game is about to load into the
//! OS file cache. Meanwhile every file Unity and Mono open for reading is recorded through a `CreateFileW` hook in
//! their import tables. At the main menu FastBoot's addon calls [`fastboot_early_boot_finished`], which removes the
//! hooks and saves what was opened as `readlist.txt`, the exact order for the next launch's warm-up.

use crate::{iat, log, win};
use fastboot_warm::{readlist, record::Recorder, run, settings};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

type CreateFileWFn = unsafe extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, *mut c_void) -> isize;

static REAL_CREATE_FILE: AtomicUsize = AtomicUsize::new(0);
static RECORDER: OnceLock<Recorder> = OnceLock::new();
static WARMUP: OnceLock<run::Warmup> = OnceLock::new();
static READLIST: OnceLock<PathBuf> = OnceLock::new();
/// Modules whose CreateFileW slots point at the hook.
static HOOKED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Starts the warm-up and recording, unless `Warmup = false` in FastBoot.cfg or `FASTBOOT_WARMUP_DISABLE` is set.
/// Called from DllMain: only spawns a thread and reads one small file.
pub unsafe fn start(root: &Path, plugin_data: &Path) {
    let cfg = plugin_data.parent().unwrap_or(root).join("FastBoot.cfg");
    if std::env::var("FASTBOOT_WARMUP_DISABLE").is_ok_and(|v| !v.is_empty() && v != "0") {
        log::line("warm-up off: FASTBOOT_WARMUP_DISABLE is set");
        return;
    }
    if !settings::read_bool(&cfg, "Warmup", true) {
        log::line("warm-up off: Warmup = false in FastBoot.cfg");
        return;
    }
    let list = plugin_data.join("readlist.txt");
    let budget = std::env::var("FASTBOOT_WARMUP_BUDGET_MB").ok().and_then(|v| v.parse::<u64>().ok()).map(|mb| mb << 20);
    match run::spawn(root.to_path_buf(), list.clone(), budget) {
        Ok(w) => {
            let _ = WARMUP.set(w);
            log::line(&format!("warm-up started ({})", if list.is_file() { "read list from the last boot" } else { "no read list yet: by loading order" }));
        }
        Err(e) => log::line(&format!("warm-up not started: {}", e)),
    }
    let _ = READLIST.set(list);
    let _ = RECORDER.set(Recorder::new(root));
    let k32 = win::GetModuleHandleW(win::wide("kernel32.dll".as_ref()).as_ptr());
    REAL_CREATE_FILE.store(win::GetProcAddress(k32, c"CreateFileW".as_ptr()) as usize, Ordering::Release);
}

/// Records the files `module` opens, by pointing its CreateFileW import slots at the hook.
pub unsafe fn record_opens_in(module: win::Hmodule, name: &str) {
    if RECORDER.get().is_none() || module.is_null() {
        return;
    }
    let mut n = 0;
    // A module may import CreateFileW from kernel32 or, through an API set, straight from kernelbase.
    for owner in ["kernel32.dll", "kernelbase.dll"] {
        let m = win::GetModuleHandleW(win::wide(owner.as_ref()).as_ptr());
        let f = if m.is_null() { 0 } else { win::GetProcAddress(m, c"CreateFileW".as_ptr()) as usize };
        if f != 0 {
            n += iat::redirect(module as *const u8, f, create_file_w as usize).unwrap_or(0);
        }
    }
    if n > 0 {
        if let Ok(mut h) = HOOKED.lock() {
            h.push(module as usize);
        }
    }
    log::line(&format!("recording file opens in {} ({} import slot(s))", name, n));
}

unsafe extern "system" fn create_file_w(
    name: *const u16, access: u32, share: u32, sa: *mut c_void, disposition: u32, flags: u32, template: *mut c_void,
) -> isize {
    const OPEN_EXISTING: u32 = 3;
    // GENERIC_WRITE, GENERIC_ALL, FILE_WRITE_DATA, FILE_APPEND_DATA.
    const WRITES: u32 = 0x4000_0000 | 0x1000_0000 | 0x2 | 0x4;
    // Directories are opened with backup semantics; they are not worth recording.
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const INVALID_HANDLE_VALUE: isize = -1;
    let real: CreateFileWFn = std::mem::transmute(REAL_CREATE_FILE.load(Ordering::Acquire));
    let handle = real(name, access, share, sa, disposition, flags, template);
    // Only opens that succeeded: Mono probes many names that do not exist while resolving assemblies.
    if handle != INVALID_HANDLE_VALUE && !name.is_null() && disposition == OPEN_EXISTING && access & WRITES == 0
        && flags & FILE_FLAG_BACKUP_SEMANTICS == 0
    {
        if let Some(r) = RECORDER.get() {
            let mut len = 0;
            while len < 32768 && *name.add(len) != 0 {
                len += 1;
            }
            // Recording must not change what the caller sees, including the error code of the open.
            let error = win::GetLastError();
            r.note(&String::from_utf16_lossy(std::slice::from_raw_parts(name, len)));
            win::SetLastError(error);
        }
    }
    handle
}

/// Called by FastBoot's addon at the main menu: stops recording, restores the import tables, saves the read
/// list and logs the warm-up's result. Files FastBoot's own parser reads are not in the list (kspcfg's imports
/// are not hooked); the loading-order rules still warm them, and the parser's prefetch reads them in parallel. Returns how many files were recorded, or -1 if nothing was recording.
#[no_mangle]
pub extern "C" fn fastboot_early_boot_finished() -> i32 {
    std::panic::catch_unwind(|| unsafe { boot_finished() }).unwrap_or(-1)
}

unsafe fn boot_finished() -> i32 {
    let Some(r) = RECORDER.get() else { return -1 };
    let real = REAL_CREATE_FILE.load(Ordering::Acquire);
    // Every hooked slot goes back to kernel32's CreateFileW; slots that held kernelbase's land in the same place.
    for m in HOOKED.lock().map(|mut h| std::mem::take(&mut *h)).unwrap_or_default() {
        let _ = iat::redirect(m as *const u8, create_file_w as usize, real);
    }
    let entries = r.finish();
    let n = entries.len();
    if let Some(path) = READLIST.get() {
        match readlist::save(path, &entries) {
            Ok(()) => log::line(&format!("boot finished: {} files opened while booting saved to {}", n, path.display())),
            Err(e) => log::line(&format!("boot finished: could not save the read list: {}", e)),
        }
    }
    log::line(&warmup_summary());
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn warmup_summary() -> String {
    WARMUP.get().map(|w| w.stats.summary()).unwrap_or_else(|| "warm-up did not run".into())
}

/// The warm-up's progress as UTF-16 into `out` (up to `cap` units). Returns the full length, so a caller with a
/// small buffer can retry.
#[no_mangle]
pub unsafe extern "C" fn fastboot_early_warmup_summary(out: *mut u16, cap: usize) -> usize {
    let s: Vec<u16> = warmup_summary().encode_utf16().collect();
    if !out.is_null() {
        std::ptr::copy_nonoverlapping(s.as_ptr(), out, s.len().min(cap));
    }
    s.len()
}
