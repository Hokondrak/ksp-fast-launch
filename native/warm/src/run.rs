//! Reading the plan into the OS file cache on background threads.

use crate::{plan, readlist, system};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Progress, readable while the warm-up runs.
#[derive(Default)]
pub struct Stats {
    pub planned: AtomicUsize,
    pub from_readlist: AtomicUsize,
    pub files: AtomicUsize,
    pub bytes: AtomicU64,
    pub failed: AtomicUsize,
    pub budget: AtomicU64,
    pub threads: AtomicUsize,
    /// Milliseconds from start until the plan was built, and until the last read finished.
    pub plan_ms: AtomicU64,
    pub done_ms: AtomicU64,
    pub done: AtomicBool,
    pub hit_budget: AtomicBool,
}

impl Stats {
    pub fn summary(&self) -> String {
        let mib = self.bytes.load(Ordering::Relaxed) as f64 / (1u64 << 20) as f64;
        let state = if !self.done.load(Ordering::Acquire) {
            "still running".to_string()
        } else {
            format!("finished after {:.1} s", self.done_ms.load(Ordering::Relaxed) as f64 / 1000.0)
        };
        format!(
            "warm-up {}: {} of {} files ({} from the read list), {:.0} MiB on {} threads; plan took {} ms{}{}",
            state,
            self.files.load(Ordering::Relaxed),
            self.planned.load(Ordering::Relaxed),
            self.from_readlist.load(Ordering::Relaxed),
            mib,
            self.threads.load(Ordering::Relaxed),
            self.plan_ms.load(Ordering::Relaxed),
            if self.hit_budget.load(Ordering::Relaxed) {
                format!("; stopped at the memory budget ({} MiB)", self.budget.load(Ordering::Relaxed) >> 20)
            } else {
                String::new()
            },
            match self.failed.load(Ordering::Relaxed) {
                0 => String::new(),
                n => format!("; {} unreadable", n),
            },
        )
    }
}

/// A running (or finished) warm-up. Dropping it does not stop it; `cancel` does.
pub struct Warmup {
    pub stats: Arc<Stats>,
    cancel: Arc<AtomicBool>,
}

impl Warmup {
    /// Stops issuing new reads. Reads in progress finish.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// Starts warming the install at `root` on a background thread and returns at once. Building the plan, which
/// walks GameData, happens on that thread too, so this is safe to call from a DLL entry point. `budget` overrides
/// the memory budget from [`system::budget`] (tests use it to keep the reads small).
pub fn spawn(root: PathBuf, readlist_path: PathBuf, budget: Option<u64>) -> std::io::Result<Warmup> {
    let stats = Arc::new(Stats::default());
    let cancel = Arc::new(AtomicBool::new(false));
    let (s, c) = (stats.clone(), cancel.clone());
    std::thread::Builder::new().name("FastBoot warm-up".into()).spawn(move || {
        let started = Instant::now();
        let p = plan::build(&root, &readlist::load(&readlist_path));
        s.plan_ms.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
        s.planned.store(p.files.len(), Ordering::Relaxed);
        s.from_readlist.store(p.from_readlist, Ordering::Relaxed);
        let threads = system::reader_threads(&root);
        let budget = budget.unwrap_or_else(system::budget);
        read_all(p.files, threads, budget, &s, &c);
        s.done_ms.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
        s.done.store(true, Ordering::Release);
    })?;
    Ok(Warmup { stats, cancel })
}

/// Reads `files` front to back on `threads` threads until all are read, `budget` bytes are read, or `cancel`.
pub fn read_all(files: Vec<PathBuf>, threads: usize, budget: u64, stats: &Stats, cancel: &AtomicBool) {
    stats.threads.store(threads, Ordering::Relaxed);
    stats.budget.store(budget, Ordering::Relaxed);
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                lower_priority();
                let mut buf = vec![0u8; 1 << 20];
                loop {
                    if cancel.load(Ordering::Acquire) {
                        return;
                    }
                    if stats.bytes.load(Ordering::Relaxed) >= budget {
                        stats.hit_budget.store(true, Ordering::Relaxed);
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { return };
                    match read_through(path, &mut buf) {
                        Ok(n) => {
                            stats.bytes.fetch_add(n, Ordering::Relaxed);
                            stats.files.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(_) => {
                            stats.failed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            });
        }
    });
}

/// Reads the whole file and discards it; the point is the OS cache.
fn read_through(path: &PathBuf, buf: &mut [u8]) -> std::io::Result<u64> {
    let mut f = open_sequential(path)?;
    let mut total = 0u64;
    loop {
        match f.read(buf)? {
            0 => return Ok(total),
            n => total += n as u64,
        }
    }
}

#[cfg(windows)]
fn open_sequential(path: &PathBuf) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;
    // Share everything: the game may open, rename or replace these files while they are being read.
    std::fs::OpenOptions::new().read(true).share_mode(0x7).custom_flags(FILE_FLAG_SEQUENTIAL_SCAN).open(path)
}

#[cfg(not(windows))]
fn open_sequential(path: &PathBuf) -> std::io::Result<File> {
    File::open(path)
}

/// The game's main thread must win any contest for a CPU core.
#[cfg(windows)]
fn lower_priority() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }
    const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[cfg(not(windows))]
fn lower_priority() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn files(n: usize, size: usize) -> (PathBuf, Vec<PathBuf>) {
        let dir = std::env::temp_dir().join(format!("fastboot-run-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let list = (0..n)
            .map(|i| {
                let p = dir.join(format!("f{}.bin", i));
                fs::write(&p, vec![7u8; size]).unwrap();
                p
            })
            .collect();
        (dir, list)
    }

    #[test]
    fn reads_everything_and_counts_failures() {
        let (dir, mut list) = files(20, 3 << 20 | 5);
        list.push(dir.join("missing.bin"));
        let stats = Stats::default();
        read_all(list, 4, u64::MAX, &stats, &AtomicBool::new(false));
        assert_eq!(stats.files.load(Ordering::Relaxed), 20);
        assert_eq!(stats.failed.load(Ordering::Relaxed), 1);
        assert_eq!(stats.bytes.load(Ordering::Relaxed), 20 * (3 << 20 | 5) as u64);
        assert!(!stats.hit_budget.load(Ordering::Relaxed));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stops_at_the_budget() {
        let (dir, list) = files(50, 1 << 20);
        let stats = Stats::default();
        read_all(list, 1, 10 << 20, &stats, &AtomicBool::new(false));
        assert_eq!(stats.files.load(Ordering::Relaxed), 10);
        assert!(stats.hit_budget.load(Ordering::Relaxed));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_stops_new_reads() {
        let (dir, list) = files(5, 10);
        let stats = Stats::default();
        read_all(list, 2, u64::MAX, &stats, &AtomicBool::new(true));
        assert_eq!(stats.files.load(Ordering::Relaxed), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spawn_finishes_and_summarises() {
        let (dir, _) = files(3, 100);
        let w = spawn(dir.clone(), dir.join("readlist.txt"), None).unwrap();
        let t = Instant::now();
        while !w.stats.done.load(Ordering::Acquire) {
            assert!(t.elapsed().as_secs() < 30, "warm-up did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Not a KSP folder, so the rules find nothing to warm.
        assert_eq!(w.stats.planned.load(Ordering::Relaxed), 0);
        assert!(w.stats.summary().starts_with("warm-up finished"));
        let _ = fs::remove_dir_all(&dir);
    }
}
