//! Samples, from outside the game: the child's memory counters, system memory, hard page-ins, and GPU memory
//! (the child's own dedicated/shared usage and every adapter's dedicated usage) via Windows' PDH counters.

use crate::win::*;
use std::ffi::c_void;

#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub t: f64,
    pub working_set_mb: f64,
    pub private_mb: f64,
    pub page_faults: u64,
    pub sys_avail_mb: f64,
    pub sys_commit_mb: f64,
    pub sys_commit_limit_mb: f64,
    pub pages_input_per_s: f64,
    pub gpu_dedicated_mb: f64,
    pub gpu_shared_mb: f64,
    /// Dedicated usage of all processes on the adapter KSP uses most (matched by LUID).
    pub adapter_dedicated_mb: f64,
}

/// Every field starts unavailable; a failed reading stays NaN instead of looking like zero.
fn unavailable(t: f64) -> Sample {
    let n = f64::NAN;
    Sample {
        t,
        working_set_mb: n,
        private_mb: n,
        page_faults: 0,
        sys_avail_mb: n,
        sys_commit_mb: n,
        sys_commit_limit_mb: n,
        pages_input_per_s: n,
        gpu_dedicated_mb: n,
        gpu_shared_mb: n,
        adapter_dedicated_mb: n,
    }
}

/// "pid_1234_luid_0x00000000_0x0000C1F3_phys_0" -> "luid_0x00000000_0x0000C1F3"; adapter instances
/// "luid_0x00000000_0x0000C1F3_phys_0" -> the same key.
fn luid_of(instance: &str) -> Option<String> {
    let i = instance.find("luid_")?;
    let rest = &instance[i..];
    let end = rest.find("_phys").unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

pub struct Telemetry {
    process: HANDLE,
    query: PDH_HQUERY,
    gpu_dedicated: PDH_HCOUNTER,
    gpu_shared: PDH_HCOUNTER,
    adapter_dedicated: PDH_HCOUNTER,
    pages_input: PDH_HCOUNTER,
    buffer: Vec<u8>,
}

// The raw handles are only used from the sampling thread that owns the Telemetry.
unsafe impl Send for Telemetry {}

impl Telemetry {
    pub fn new(process: *mut c_void, pid: u32) -> Telemetry {
        let mut t = Telemetry {
            process,
            query: 0,
            gpu_dedicated: 0,
            gpu_shared: 0,
            adapter_dedicated: 0,
            pages_input: 0,
            buffer: vec![0u8; 64 * 1024],
        };
        unsafe {
            if PdhOpenQueryW(std::ptr::null(), 0, &mut t.query) == 0 {
                let add = |path: &str, out: &mut PDH_HCOUNTER| {
                    let w = wide(path);
                    if PdhAddEnglishCounterW(t.query, w.as_ptr(), 0, out) != 0 {
                        *out = 0;
                    }
                };
                // "pid_N_*", not "pid_N*": the latter also matches pid N0, N1, ...
                add(&format!("\\GPU Process Memory(pid_{}_*)\\Dedicated Usage", pid), &mut t.gpu_dedicated);
                add(&format!("\\GPU Process Memory(pid_{}_*)\\Shared Usage", pid), &mut t.gpu_shared);
                add("\\GPU Adapter Memory(*)\\Dedicated Usage", &mut t.adapter_dedicated);
                add("\\Memory\\Pages Input/sec", &mut t.pages_input);
                PdhCollectQueryData(t.query);
            }
        }
        t
    }

    /// Every valid instance of a counter with its name, in the counter's raw unit.
    fn read_named(&mut self, counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
        if counter == 0 {
            return Vec::new();
        }
        unsafe {
            let mut size = self.buffer.len() as u32;
            let mut count = 0u32;
            let mut status = PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE | PDH_FMT_NOCAP100,
                &mut size,
                &mut count,
                self.buffer.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W,
            );
            if status == PDH_MORE_DATA {
                self.buffer.resize(size as usize + 1024, 0);
                size = self.buffer.len() as u32;
                status = PdhGetFormattedCounterArrayW(
                    counter,
                    PDH_FMT_DOUBLE | PDH_FMT_NOCAP100,
                    &mut size,
                    &mut count,
                    self.buffer.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W,
                );
            }
            if status != 0 || count == 0 {
                return Vec::new();
            }
            let items = std::slice::from_raw_parts(self.buffer.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W, count as usize);
            items.iter()
                .filter(|i| i.FmtValue.CStatus == 0 || i.FmtValue.CStatus == 1)
                .map(|i| {
                    let mut n = 0;
                    while *i.szName.add(n) != 0 {
                        n += 1;
                    }
                    (String::from_utf16_lossy(std::slice::from_raw_parts(i.szName, n)), i.FmtValue.doubleValue)
                })
                .collect()
        }
    }

    /// Sum over valid instances, or NaN when there is none.
    fn read_sum(&mut self, counter: PDH_HCOUNTER) -> f64 {
        let v = self.read_named(counter);
        if v.is_empty() { f64::NAN } else { v.iter().map(|x| x.1).sum() }
    }

    pub fn sample(&mut self, t: f64) -> Sample {
        const MB: f64 = 1024.0 * 1024.0;
        let mut s = unavailable(t);
        unsafe {
            let mut pmc = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
            if K32GetProcessMemoryInfo(self.process, &mut pmc, pmc.cb) != 0 {
                s.working_set_mb = pmc.WorkingSetSize as f64 / MB;
                s.private_mb = pmc.PrivateUsage as f64 / MB;
                s.page_faults = pmc.PageFaultCount as u64;
            }
            let mut ms = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
            if GlobalMemoryStatusEx(&mut ms) != 0 {
                s.sys_avail_mb = ms.ullAvailPhys as f64 / MB;
                s.sys_commit_mb = (ms.ullTotalPageFile - ms.ullAvailPageFile) as f64 / MB;
                s.sys_commit_limit_mb = ms.ullTotalPageFile as f64 / MB;
            }
            if self.query != 0 {
                PdhCollectQueryData(self.query);
            }
        }
        s.pages_input_per_s = self.read_sum(self.pages_input);
        let ksp_dedicated = self.read_named(self.gpu_dedicated);
        if !ksp_dedicated.is_empty() {
            s.gpu_dedicated_mb = ksp_dedicated.iter().map(|x| x.1).sum::<f64>() / MB;
        }
        s.gpu_shared_mb = self.read_sum(self.gpu_shared) / MB;
        // The adapter where KSP holds the most dedicated memory, by LUID.
        let ksp_luid = ksp_dedicated.iter()
            .filter_map(|(n, v)| luid_of(n).map(|l| (l, *v)))
            .fold(None::<(String, f64)>, |best, (l, v)| match best { Some((_, bv)) if bv >= v => best, _ => Some((l, v)) })
            .map(|x| x.0);
        if let Some(luid) = ksp_luid {
            let on_adapter: Vec<f64> = self.read_named(self.adapter_dedicated).into_iter()
                .filter(|(n, _)| luid_of(n).as_deref() == Some(luid.as_str())).map(|x| x.1).collect();
            if !on_adapter.is_empty() {
                s.adapter_dedicated_mb = on_adapter.iter().sum::<f64>() / MB;
            }
        }
        s
    }
}

/// Every process's dedicated and shared GPU memory right now, largest first: (pid, name, dedicated MB, shared MB).
/// Read-only: it queries Windows' performance counters and a process snapshot.
pub fn gpu_top() -> Vec<(u32, String, f64, f64)> {
    const MB: f64 = 1024.0 * 1024.0;
    let mut names = std::collections::HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if !snap.is_null() && snap as isize != -1 {
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut e);
            while ok != 0 {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(260);
                names.insert(e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..len]));
                ok = Process32NextW(snap, &mut e);
            }
            CloseHandle(snap);
        }
    }
    let mut per_pid: std::collections::HashMap<u32, (f64, f64)> = std::collections::HashMap::new();
    unsafe {
        let mut q: PDH_HQUERY = 0;
        if PdhOpenQueryW(std::ptr::null(), 0, &mut q) != 0 {
            return Vec::new();
        }
        let mut ded: PDH_HCOUNTER = 0;
        let mut sh: PDH_HCOUNTER = 0;
        PdhAddEnglishCounterW(q, wide("\\GPU Process Memory(*)\\Dedicated Usage").as_ptr(), 0, &mut ded);
        PdhAddEnglishCounterW(q, wide("\\GPU Process Memory(*)\\Shared Usage").as_ptr(), 0, &mut sh);
        PdhCollectQueryData(q);
        for (counter, slot) in [(ded, 0usize), (sh, 1usize)] {
            let mut size = 0u32;
            let mut count = 0u32;
            PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, &mut size, &mut count, std::ptr::null_mut());
            let mut buf = vec![0u8; size as usize + 1024];
            size = buf.len() as u32;
            if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, &mut size, &mut count,
                buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W) != 0 {
                continue;
            }
            let items = std::slice::from_raw_parts(buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W, count as usize);
            for it in items {
                let mut n = 0;
                while *it.szName.add(n) != 0 {
                    n += 1;
                }
                let name = String::from_utf16_lossy(std::slice::from_raw_parts(it.szName, n));
                // Instance names look like "pid_1234_luid_0x00000000_0x0000C1F3_phys_0".
                let pid = name.strip_prefix("pid_").and_then(|r| r.split('_').next()).and_then(|p| p.parse::<u32>().ok());
                if let Some(pid) = pid {
                    let e = per_pid.entry(pid).or_insert((0.0, 0.0));
                    if slot == 0 { e.0 += it.FmtValue.doubleValue / MB } else { e.1 += it.FmtValue.doubleValue / MB }
                }
            }
        }
        PdhCloseQuery(q);
    }
    let mut v: Vec<(u32, String, f64, f64)> = per_pid.into_iter()
        .map(|(pid, (d, s))| (pid, names.get(&pid).cloned().unwrap_or_else(|| "?".into()), d, s))
        .collect();
    v.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    v
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        if self.query != 0 {
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::luid_of;

    #[test]
    fn process_and_adapter_instances_share_a_luid_key() {
        let p = luid_of("pid_22500_luid_0x00000000_0x0000C1F3_phys_0");
        let a = luid_of("luid_0x00000000_0x0000C1F3_phys_0");
        assert_eq!(p.as_deref(), Some("luid_0x00000000_0x0000C1F3"));
        assert_eq!(p, a);
        assert_eq!(luid_of("no luid here"), None);
    }
}
