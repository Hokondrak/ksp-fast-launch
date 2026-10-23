//! The few Win32 / PDH declarations ksplaunch needs, declared by hand to avoid crate downloads.
#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::ffi::c_void;

pub type HANDLE = *mut c_void;
pub type PDH_HQUERY = isize;
pub type PDH_HCOUNTER = isize;

pub const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
pub const PDH_FMT_NOCAP100: u32 = 0x0000_8000;
pub const PDH_MORE_DATA: u32 = 0x8000_07D2;
pub const THREAD_MODE_BACKGROUND_BEGIN: i32 = 0x0001_0000;

#[repr(C)]
#[derive(Default)]
pub struct PROCESS_MEMORY_COUNTERS_EX {
    pub cb: u32,
    pub PageFaultCount: u32,
    pub PeakWorkingSetSize: usize,
    pub WorkingSetSize: usize,
    pub QuotaPeakPagedPoolUsage: usize,
    pub QuotaPagedPoolUsage: usize,
    pub QuotaPeakNonPagedPoolUsage: usize,
    pub QuotaNonPagedPoolUsage: usize,
    pub PagefileUsage: usize,
    pub PeakPagefileUsage: usize,
    pub PrivateUsage: usize,
}

#[repr(C)]
#[derive(Default)]
pub struct MEMORYSTATUSEX {
    pub dwLength: u32,
    pub dwMemoryLoad: u32,
    pub ullTotalPhys: u64,
    pub ullAvailPhys: u64,
    pub ullTotalPageFile: u64,
    pub ullAvailPageFile: u64,
    pub ullTotalVirtual: u64,
    pub ullAvailVirtual: u64,
    pub ullAvailExtendedVirtual: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PDH_FMT_COUNTERVALUE {
    pub CStatus: u32,
    pub _pad: u32,
    pub doubleValue: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PDH_FMT_COUNTERVALUE_ITEM_W {
    pub szName: *mut u16,
    pub FmtValue: PDH_FMT_COUNTERVALUE,
}

pub const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;

#[repr(C)]
pub struct PROCESSENTRY32W {
    pub dwSize: u32,
    pub cntUsage: u32,
    pub th32ProcessID: u32,
    pub th32DefaultHeapID: usize,
    pub th32ModuleID: u32,
    pub cntThreads: u32,
    pub th32ParentProcessID: u32,
    pub pcPriClassBase: i32,
    pub dwFlags: u32,
    pub szExeFile: [u16; 260],
}

#[link(name = "kernel32")]
extern "system" {
    pub fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> HANDLE;
    pub fn Process32FirstW(snapshot: HANDLE, entry: *mut PROCESSENTRY32W) -> i32;
    pub fn Process32NextW(snapshot: HANDLE, entry: *mut PROCESSENTRY32W) -> i32;
    pub fn CloseHandle(h: HANDLE) -> i32;
    pub fn GetProcessTimes(process: HANDLE, creation: *mut u64, exit: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
    pub fn K32GetProcessMemoryInfo(process: HANDLE, counters: *mut PROCESS_MEMORY_COUNTERS_EX, cb: u32) -> i32;
    pub fn GlobalMemoryStatusEx(buffer: *mut MEMORYSTATUSEX) -> i32;
    pub fn GetCurrentThread() -> HANDLE;
    pub fn SetThreadPriority(thread: HANDLE, priority: i32) -> i32;
}

#[link(name = "pdh")]
extern "system" {
    pub fn PdhOpenQueryW(source: *const u16, user: usize, query: *mut PDH_HQUERY) -> u32;
    pub fn PdhAddEnglishCounterW(query: PDH_HQUERY, path: *const u16, user: usize, counter: *mut PDH_HCOUNTER) -> u32;
    pub fn PdhCollectQueryData(query: PDH_HQUERY) -> u32;
    pub fn PdhGetFormattedCounterArrayW(
        counter: PDH_HCOUNTER,
        format: u32,
        buffer_size: *mut u32,
        item_count: *mut u32,
        items: *mut PDH_FMT_COUNTERVALUE_ITEM_W,
    ) -> u32;
    pub fn PdhCloseQuery(query: PDH_HQUERY) -> u32;
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Background mode lowers both CPU and I/O priority of the calling thread.
pub fn background_thread() {
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}
