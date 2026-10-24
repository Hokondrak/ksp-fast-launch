//! What the machine allows: how much memory the warm-up may fill, and whether the disk prefers one reader.

use std::path::Path;

/// Without a reading of free memory, warm at most this much.
const FALLBACK_BUDGET: u64 = 4 << 30;

/// Bytes the warm-up may read: half the physical memory free right now. The OS file cache lives in memory
/// nobody else is using, and the game will need much of the rest; past this point warming would only evict what
/// it warmed earlier.
pub fn budget() -> u64 {
    available_memory().map(|m| m / 2).unwrap_or(FALLBACK_BUDGET)
}

/// Reader threads: several for SSDs, which need many requests in flight to reach their speed; one for disks
/// that seek, where parallel readers would fight over the head. Unknown drives are treated as SSDs: almost
/// every machine that runs a heavily modded KSP today boots from one.
pub fn reader_threads(root: &Path) -> usize {
    match seeks(root) {
        Some(true) => 1,
        _ => 8,
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct MemoryStatusEx {
        pub length: u32,
        pub memory_load: u32,
        pub total_phys: u64,
        pub avail_phys: u64,
        pub total_page_file: u64,
        pub avail_page_file: u64,
        pub total_virtual: u64,
        pub avail_virtual: u64,
        pub avail_extended_virtual: u64,
    }

    #[repr(C)]
    pub struct StoragePropertyQuery {
        pub property_id: u32,
        pub query_type: u32,
        pub additional: [u8; 1],
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct SeekPenaltyDescriptor {
        pub version: u32,
        pub size: u32,
        pub incurs_seek_penalty: u8,
    }

    pub const STORAGE_DEVICE_SEEK_PENALTY_PROPERTY: u32 = 7;
    pub const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
    pub const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
    pub const OPEN_EXISTING: u32 = 3;
    pub const INVALID_HANDLE: isize = -1;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
        pub fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *mut c_void, disposition: u32, flags: u32, template: *mut c_void) -> isize;
        pub fn DeviceIoControl(
            device: isize, code: u32, input: *const c_void, input_len: u32, output: *mut c_void, output_len: u32,
            returned: *mut u32, overlapped: *mut c_void,
        ) -> i32;
        pub fn CloseHandle(handle: isize) -> i32;
    }
}

#[cfg(windows)]
pub fn available_memory() -> Option<u64> {
    let mut s = win::MemoryStatusEx {
        length: std::mem::size_of::<win::MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    (unsafe { win::GlobalMemoryStatusEx(&mut s) } != 0).then_some(s.avail_phys)
}

#[cfg(not(windows))]
pub fn available_memory() -> Option<u64> {
    None
}

/// Whether the drive holding `path` has a seek penalty (spinning disk). None if Windows cannot tell, or elsewhere.
#[cfg(windows)]
pub fn seeks(path: &Path) -> Option<bool> {
    use std::os::windows::ffi::OsStrExt;
    let s = path.to_str()?;
    let s = s.strip_prefix(r"\\?\").unwrap_or(s);
    let drive = s.get(..2).filter(|d| d.ends_with(':'))?;
    let volume: Vec<u16> = std::ffi::OsStr::new(&format!(r"\\.\{}", drive)).encode_wide().chain(Some(0)).collect();
    unsafe {
        // No access rights are needed to query storage properties, so this works without elevation.
        let h = win::CreateFileW(volume.as_ptr(), 0, win::FILE_SHARE_READ_WRITE, std::ptr::null_mut(), win::OPEN_EXISTING, 0, std::ptr::null_mut());
        if h == win::INVALID_HANDLE {
            return None;
        }
        let query = win::StoragePropertyQuery { property_id: win::STORAGE_DEVICE_SEEK_PENALTY_PROPERTY, query_type: 0, additional: [0] };
        let mut out = win::SeekPenaltyDescriptor::default();
        let mut returned = 0u32;
        let ok = win::DeviceIoControl(
            h,
            win::IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const _ as *const _,
            std::mem::size_of::<win::StoragePropertyQuery>() as u32,
            &mut out as *mut _ as *mut _,
            std::mem::size_of::<win::SeekPenaltyDescriptor>() as u32,
            &mut returned,
            std::ptr::null_mut(),
        );
        win::CloseHandle(h);
        (ok != 0 && returned as usize >= std::mem::size_of::<win::SeekPenaltyDescriptor>()).then_some(out.incurs_seek_penalty != 0)
    }
}

#[cfg(not(windows))]
pub fn seeks(_path: &Path) -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_is_positive() {
        assert!(budget() > 0);
        if cfg!(windows) {
            assert!(available_memory().is_some());
        }
    }

    #[test]
    fn threads_are_sane() {
        let n = reader_threads(&std::env::temp_dir());
        assert!(n == 1 || n == 8);
        assert_eq!(seeks(Path::new("not a drive path")), None);
    }
}
