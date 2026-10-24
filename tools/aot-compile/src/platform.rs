use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
use std::{
    ffi::{c_char, c_void},
    fs::File,
    io::Read,
    path::Path,
};
type Handle = *mut c_void;
pub fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}
#[link(name = "kernel32")]
extern "system" {
    pub fn LoadLibraryW(p: *const u16) -> Handle;
    pub fn GetProcAddress(h: Handle, p: *const c_char) -> Handle;
    pub fn SetDllDirectoryW(p: *const u16) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn SetPriorityClass(h: Handle, priority: u32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn Process32FirstW(h: Handle, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(h: Handle, entry: *mut ProcessEntry) -> i32;
    fn GlobalMemoryStatusEx(info: *mut Memory) -> i32;
    fn CreateJobObjectW(attr: Handle, name: *const u16) -> Handle;
    fn SetInformationJobObject(h: Handle, class: i32, info: *const c_void, len: u32) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
}
#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    exe: [u16; 260],
}
#[repr(C)]
struct Memory {
    length: u32,
    load: u32,
    total: u64,
    available: u64,
    total_commit: u64,
    available_commit: u64,
    total_virtual: u64,
    available_virtual: u64,
    extended: u64,
}
pub fn low_priority() -> Result<(), String> {
    if unsafe { SetPriorityClass(GetCurrentProcess(), 0x4000) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}
pub fn idle_guard() -> Result<(), String> {
    unsafe {
        let h = CreateToolhelp32Snapshot(2, 0);
        if h as isize == -1 {
            return Err("cannot check whether KSP is running".into());
        }
        let mut e: ProcessEntry = std::mem::zeroed();
        e.size = std::mem::size_of_val(&e) as u32;
        let mut more = Process32FirstW(h, &mut e);
        let mut running = false;
        while more != 0 {
            let n = e.exe.iter().position(|x| *x == 0).unwrap_or(260);
            let name = String::from_utf16_lossy(&e.exe[..n]);
            if name.eq_ignore_ascii_case("KSP_x64.exe") || name.eq_ignore_ascii_case("KSP.exe") {
                running = true;
                break;
            }
            more = Process32NextW(h, &mut e);
        }
        CloseHandle(h);
        if running {
            return Err("deferred: KSP is running; compile after game exit".into());
        }
        let mut m: Memory = std::mem::zeroed();
        m.length = std::mem::size_of_val(&m) as u32;
        if GlobalMemoryStatusEx(&mut m) == 0 {
            return Err("cannot measure available memory".into());
        }
        if m.available < 3 * 1024 * 1024 * 1024 || m.available_commit < 3 * 1024 * 1024 * 1024 {
            return Err("deferred: less than 3 GiB free physical memory or commit headroom".into());
        }
    }
    Ok(())
}
#[repr(C)]
struct BasicLimits {
    process_time: i64,
    job_time: i64,
    flags: u32,
    min_ws: usize,
    max_ws: usize,
    active: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
struct IoCounters {
    read_ops: u64,
    write_ops: u64,
    other_ops: u64,
    read_bytes: u64,
    write_bytes: u64,
    other_bytes: u64,
}
#[repr(C)]
struct Limits {
    basic: BasicLimits,
    io: IoCounters,
    process_memory: usize,
    job_memory: usize,
    peak_process: usize,
    peak_job: usize,
}
pub struct Job(Handle);
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
impl Job {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let h = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if h.is_null() {
                return Err("CreateJobObject failed".into());
            }
            let job = Job(h);
            let mut limits: Limits = std::mem::zeroed();
            limits.basic.flags = 0x2000 | 0x100; // KILL_ON_JOB_CLOSE | PROCESS_MEMORY
            limits.process_memory = 2 * 1024 * 1024 * 1024;
            if SetInformationJobObject(
                h,
                9,
                &limits as *const _ as *const c_void,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err("cannot set worker resource limits".into());
            }
            Ok(job)
        }
    }
    pub fn assign(&self, child: &std::process::Child) -> Result<(), String> {
        if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) } == 0 {
            return Err(format!(
                "cannot isolate worker: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}
pub fn replace(from: &Path, to: &Path) -> Result<(), String> {
    if unsafe { MoveFileExW(wide(from).as_ptr(), wide(to).as_ptr(), 1 | 8) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

#[link(name = "bcrypt")]
extern "system" {
    fn BCryptOpenAlgorithmProvider(
        h: *mut Handle,
        name: *const u16,
        implementation: *const u16,
        flags: u32,
    ) -> i32;
    fn BCryptCreateHash(
        alg: Handle,
        hash: *mut Handle,
        object: *mut u8,
        object_len: u32,
        secret: *const u8,
        secret_len: u32,
        flags: u32,
    ) -> i32;
    fn BCryptHashData(hash: Handle, data: *const u8, len: u32, flags: u32) -> i32;
    fn BCryptFinishHash(hash: Handle, output: *mut u8, len: u32, flags: u32) -> i32;
    fn BCryptDestroyHash(hash: Handle) -> i32;
    fn BCryptCloseAlgorithmProvider(alg: Handle, flags: u32) -> i32;
}
struct Hash {
    alg: Handle,
    hash: Handle,
}
impl Drop for Hash {
    fn drop(&mut self) {
        unsafe {
            if !self.hash.is_null() {
                BCryptDestroyHash(self.hash);
            }
            BCryptCloseAlgorithmProvider(self.alg, 0);
        }
    }
}
impl Hash {
    fn new() -> Result<Self, String> {
        unsafe {
            let mut h = Hash {
                alg: std::ptr::null_mut(),
                hash: std::ptr::null_mut(),
            };
            let alg: Vec<u16> = "SHA256\0".encode_utf16().collect();
            if BCryptOpenAlgorithmProvider(&mut h.alg, alg.as_ptr(), std::ptr::null(), 0) < 0 {
                return Err("SHA256 provider unavailable".into());
            }
            if BCryptCreateHash(
                h.alg,
                &mut h.hash,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                0,
                0,
            ) < 0
            {
                return Err("SHA256 allocation failed".into());
            }
            Ok(h)
        }
    }
    fn update(&self, b: &[u8]) -> Result<(), String> {
        if unsafe {
            BCryptHashData(
                self.hash,
                b.as_ptr(),
                u32::try_from(b.len()).map_err(|_| "hash buffer too large")?,
                0,
            )
        } < 0
        {
            return Err("SHA256 update failed".into());
        }
        Ok(())
    }
    fn finish(self) -> Result<String, String> {
        let mut b = [0u8; 32];
        if unsafe { BCryptFinishHash(self.hash, b.as_mut_ptr(), 32, 0) } < 0 {
            return Err("SHA256 finish failed".into());
        }
        Ok(b.iter().map(|b| format!("{b:02x}")).collect())
    }
}
pub fn hash_bytes(b: &[u8]) -> Result<String, String> {
    let h = Hash::new()?;
    h.update(b)?;
    h.finish()
}
pub fn hash_file(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let h = Hash::new()?;
    let mut b = [0u8; 65536];
    loop {
        let n = f.read(&mut b).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&b[..n])?;
    }
    h.finish()
}
pub fn cpu_identity() -> String {
    use std::arch::x86_64::{__cpuid, __cpuid_count, _xgetbv};
    unsafe {
        let vendor = __cpuid(0);
        let a = __cpuid(1);
        let b = if vendor.eax >= 7 {
            __cpuid_count(7, 0)
        } else {
            __cpuid_count(0, 0)
        };
        let max = __cpuid(0x80000000).eax;
        let ext = if max >= 0x80000001 {
            __cpuid(0x80000001)
        } else {
            __cpuid(0)
        };
        let xcr = if a.ecx & (1 << 27) != 0 {
            _xgetbv(0)
        } else {
            0
        };
        format!(
            "x86_64-windows-bdwgc:{:x}-{:x}-{:x}:{:x}:{:x}-{:x}:{:x}-{:x}-{:x}:{:x}-{:x}:{:x}",
            vendor.ebx,
            vendor.edx,
            vendor.ecx,
            a.eax,
            a.ecx,
            a.edx,
            b.ebx,
            b.ecx,
            b.edx,
            ext.ecx,
            ext.edx,
            xcr
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha_known_vectors() {
        assert_eq!(
            hash_bytes(b"").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hash_bytes(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
