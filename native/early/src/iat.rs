//! Reading and rewriting a loaded module's import address table (PE32+ only).

use crate::win;
use std::ffi::c_void;

/// Every bound import slot of the module mapped at `base`, in table order.
pub unsafe fn import_slots(base: *const u8) -> Result<Vec<*mut usize>, String> {
    if base.is_null() || read::<u16>(base, 0) != 0x5A4D {
        return Err("not a PE image (no MZ header)".into());
    }
    let nt = read::<i32>(base, 0x3C) as usize;
    if read::<u32>(base, nt) != 0x0000_4550 {
        return Err("not a PE image (no PE signature)".into());
    }
    let optional = nt + 24;
    if read::<u16>(base, optional) != 0x20B {
        return Err("not a 64-bit (PE32+) image".into());
    }
    // PE32+ data directories start 112 bytes into the optional header; imports are entry 1.
    let directories = optional + 112;
    let count = read::<u32>(base, optional + 108) as usize;
    if count < 2 {
        return Ok(Vec::new());
    }
    let imports_rva = read::<u32>(base, directories + 8) as usize;
    let mut slots = Vec::new();
    if imports_rva == 0 {
        return Ok(slots);
    }
    // IMAGE_IMPORT_DESCRIPTOR: OriginalFirstThunk, TimeDateStamp, ForwarderChain, Name, FirstThunk.
    let mut desc = imports_rva;
    loop {
        let name = read::<u32>(base, desc + 12);
        let first_thunk = read::<u32>(base, desc + 16) as usize;
        if name == 0 && first_thunk == 0 {
            break;
        }
        let mut slot = first_thunk;
        while first_thunk != 0 && read::<u64>(base, slot) != 0 {
            slots.push(base.add(slot) as *mut usize);
            slot += 8;
        }
        desc += 20;
    }
    Ok(slots)
}

/// Points every import slot of `base` that currently holds `old` at `new`. Returns how many changed.
pub unsafe fn redirect(base: *const u8, old: usize, new: usize) -> Result<usize, String> {
    if old == 0 || new == 0 {
        return Err("null function address".into());
    }
    let mut changed = 0;
    for slot in import_slots(base)? {
        if slot.read_volatile() != old {
            continue;
        }
        let mut previous = 0u32;
        if win::VirtualProtect(slot as *mut c_void, 8, win::PAGE_READWRITE, &mut previous) == 0 {
            return Err(format!("VirtualProtect failed after {} slot(s)", changed));
        }
        slot.write_volatile(new);
        win::VirtualProtect(slot as *mut c_void, 8, previous, &mut previous);
        changed += 1;
    }
    Ok(changed)
}

unsafe fn read<T: Copy>(base: *const u8, offset: usize) -> T {
    (base.add(offset) as *const T).read_unaligned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_char;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    // Tests rewrite this binary's own import table, so they must not overlap.
    static IAT: Mutex<()> = Mutex::new(());

    unsafe fn own_image() -> *const u8 {
        win::GetModuleHandleW(std::ptr::null()) as *const u8
    }

    #[test]
    fn rejects_non_images() {
        let junk = [0u8; 256];
        assert!(unsafe { import_slots(junk.as_ptr()) }.is_err());
        assert!(unsafe { import_slots(std::ptr::null()) }.is_err());
    }

    #[test]
    fn finds_get_proc_address_in_own_imports() {
        let _guard = IAT.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            // Call through the import table once so the test binary is sure to import it.
            let k32 = win::GetModuleHandleW(win::wide("kernel32.dll".as_ref()).as_ptr());
            assert!(!win::GetProcAddress(k32, c"Sleep".as_ptr()).is_null());
            let gpa = win::kernel32_get_proc_address();
            assert_ne!(gpa, 0);
            let slots = import_slots(own_image()).unwrap();
            assert!(slots.iter().any(|s| s.read() == gpa));
        }
    }

    static HOOK_CALLS: AtomicUsize = AtomicUsize::new(0);
    // Captured before redirecting: looking it up inside the hook would go through the hook again.
    static REAL: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn counting_hook(module: win::Hmodule, name: *const c_char) -> *mut c_void {
        HOOK_CALLS.fetch_add(1, Ordering::SeqCst);
        let real: unsafe extern "system" fn(win::Hmodule, *const c_char) -> *mut c_void =
            std::mem::transmute(REAL.load(Ordering::SeqCst));
        real(module, name)
    }

    #[test]
    fn redirect_routes_calls_and_restores() {
        let _guard = IAT.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let gpa = win::kernel32_get_proc_address();
            REAL.store(gpa, Ordering::SeqCst);
            let hook = counting_hook as usize;
            let k32 = win::GetModuleHandleW(win::wide("kernel32.dll".as_ref()).as_ptr());
            let n = redirect(own_image(), gpa, hook).unwrap();
            assert!(n >= 1);
            let p = win::GetProcAddress(k32, c"Sleep".as_ptr());
            let calls = HOOK_CALLS.load(Ordering::SeqCst);
            assert_eq!(redirect(own_image(), hook, gpa).unwrap(), n);
            assert!(!p.is_null());
            assert!(calls >= 1, "the call went through the hook");
            win::GetProcAddress(k32, c"Sleep".as_ptr());
            assert_eq!(HOOK_CALLS.load(Ordering::SeqCst), calls, "restored: the hook is no longer called");
        }
    }
}
