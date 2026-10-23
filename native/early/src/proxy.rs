//! The 17 exports of the system `version.dll`, each forwarding to the real one.
//!
//! The real DLL is loaded by full path from the system directory on first use, never during `DllMain`. All
//! arguments of these functions are pointers or 32-bit integers, which the x64 calling convention passes in
//! 64-bit slots, so a forwarder that passes `usize` arguments through unchanged is exact; the 32-bit results
//! come back in the low half of the return register, as the caller expects.

use crate::win;
use std::sync::atomic::{AtomicUsize, Ordering};

static REAL_MODULE: AtomicUsize = AtomicUsize::new(0);

unsafe fn real_module() -> usize {
    let m = REAL_MODULE.load(Ordering::Acquire);
    if m != 0 {
        return m;
    }
    let Some(dir) = win::system_directory() else { return 0 };
    let m = win::LoadLibraryW(win::wide(dir.join("version.dll").as_os_str()).as_ptr()) as usize;
    // Racing first calls both load the same module; LoadLibrary is reference counted, so either value is fine.
    REAL_MODULE.store(m, Ordering::Release);
    m
}

/// The real export `name` (NUL-terminated), cached in `slot`; 0 if unavailable.
unsafe fn resolve(slot: &AtomicUsize, name: &'static str) -> usize {
    let f = slot.load(Ordering::Acquire);
    if f != 0 {
        return f;
    }
    let m = real_module();
    if m == 0 {
        return 0;
    }
    let f = win::GetProcAddress(m as win::Hmodule, name.as_ptr().cast()) as usize;
    slot.store(f, Ordering::Release);
    f
}

macro_rules! forward {
    ($($name:ident($($arg:ident),*);)*) => {$(
        #[no_mangle]
        pub unsafe extern "system" fn $name($($arg: usize),*) -> usize {
            static SLOT: AtomicUsize = AtomicUsize::new(0);
            let f = resolve(&SLOT, concat!(stringify!($name), "\0"));
            if f == 0 {
                win::SetLastError(win::ERROR_PROC_NOT_FOUND);
                return 0;
            }
            let f: unsafe extern "system" fn($($arg: usize),*) -> usize = std::mem::transmute(f);
            f($($arg),*)
        }
    )*};
}

forward! {
    GetFileVersionInfoA(file, handle, len, data);
    GetFileVersionInfoW(file, handle, len, data);
    GetFileVersionInfoExA(flags, file, handle, len, data);
    GetFileVersionInfoExW(flags, file, handle, len, data);
    // Undocumented; four arguments in every Windows 10/11 build.
    GetFileVersionInfoByHandle(flags, file, data, len);
    GetFileVersionInfoSizeA(file, handle);
    GetFileVersionInfoSizeW(file, handle);
    GetFileVersionInfoSizeExA(flags, file, handle);
    GetFileVersionInfoSizeExW(flags, file, handle);
    VerFindFileA(flags, file, win_dir, app_dir, cur_dir, cur_len, dest_dir, dest_len);
    VerFindFileW(flags, file, win_dir, app_dir, cur_dir, cur_len, dest_dir, dest_len);
    VerInstallFileA(flags, src_file, dest_file, src_dir, dest_dir, cur_dir, tmp_file, tmp_len);
    VerInstallFileW(flags, src_file, dest_file, src_dir, dest_dir, cur_dir, tmp_file, tmp_len);
    VerLanguageNameA(lang, buf, size);
    VerLanguageNameW(lang, buf, size);
    VerQueryValueA(block, sub_block, buffer, len);
    VerQueryValueW(block, sub_block, buffer, len);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_to_the_system_dll() {
        unsafe {
            let k32 = win::system_directory().unwrap().join("kernel32.dll");
            let path = win::wide(k32.as_os_str());
            let size = GetFileVersionInfoSizeW(path.as_ptr() as usize, 0);
            assert!(size > 0, "kernel32.dll has version info");
            let mut data = vec![0u8; size];
            assert_ne!(GetFileVersionInfoW(path.as_ptr() as usize, 0, size, data.as_mut_ptr() as usize), 0);
            let mut value = 0usize;
            let mut len = 0u32;
            let root = win::wide("\\".as_ref());
            let ok = VerQueryValueW(data.as_ptr() as usize, root.as_ptr() as usize,
                &mut value as *mut usize as usize, &mut len as *mut u32 as usize);
            assert_ne!(ok, 0);
            // VS_FIXEDFILEINFO starts with its signature.
            assert_eq!(*(value as *const u32), 0xFEEF04BD);
        }
    }

    #[test]
    fn matches_the_real_function() {
        // Language names are localised, so compare with the system DLL rather than a fixed string.
        unsafe {
            let real = win::GetProcAddress(real_module() as win::Hmodule, c"VerLanguageNameW".as_ptr());
            assert!(!real.is_null());
            let real: unsafe extern "system" fn(u32, *mut u16, u32) -> u32 = std::mem::transmute(real);
            let (mut ours, mut theirs) = ([0u16; 64], [0u16; 64]);
            let n = VerLanguageNameW(0x0409, ours.as_mut_ptr() as usize, ours.len()) as u32;
            let m = real(0x0409, theirs.as_mut_ptr(), theirs.len() as u32);
            assert!(n > 0);
            assert_eq!((n, ours), (m, theirs));
        }
    }
}
