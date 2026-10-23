//! The few Win32 functions the loader needs, declared by hand to keep it dependency-free.

use std::ffi::{c_char, c_void, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

pub type Hmodule = *mut c_void;

pub const PAGE_READWRITE: u32 = 0x04;
pub const ERROR_PROC_NOT_FOUND: u32 = 127;

#[link(name = "kernel32")]
extern "system" {
    pub fn LoadLibraryW(path: *const u16) -> Hmodule;
    pub fn GetModuleHandleW(name: *const u16) -> Hmodule;
    pub fn GetModuleFileNameW(module: Hmodule, buf: *mut u16, size: u32) -> u32;
    pub fn GetProcAddress(module: Hmodule, name: *const c_char) -> *mut c_void;
    pub fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    pub fn VirtualProtect(addr: *mut c_void, size: usize, protect: u32, old: *mut u32) -> i32;
    pub fn SetLastError(code: u32);
    pub fn DisableThreadLibraryCalls(module: Hmodule) -> i32;
}

pub fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Full path of `module`, or of the process executable for null.
pub unsafe fn module_path(module: Hmodule) -> Option<PathBuf> {
    let mut buf = vec![0u16; 32768];
    let n = GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buf[..n])))
}

pub unsafe fn system_directory() -> Option<PathBuf> {
    let mut buf = vec![0u16; 1024];
    let n = GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buf[..n])))
}

/// kernel32's own `GetProcAddress`, as the loader binds it into import tables.
pub unsafe fn kernel32_get_proc_address() -> usize {
    let k32 = GetModuleHandleW(wide("kernel32.dll".as_ref()).as_ptr());
    if k32.is_null() {
        return 0;
    }
    GetProcAddress(k32, c"GetProcAddress".as_ptr()) as usize
}
