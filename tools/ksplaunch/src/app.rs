//! Windows implementation of ksplaunch; see main.rs for usage.

use crate::report::{self, LogLine};
use crate::{telemetry, win};
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// KSP folder when neither --ksp nor a command is given: the folder ksplaunch.exe is in, if KSP is there.
fn default_ksp() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    if dir.join("KSP_x64.exe").is_file() { Some(dir) } else { None }
}

/// Where BootProfiler-style reports live: FastBoot's diagnostics folder, else the research BootProfiler's.
fn profiler_dir(ksp: &Path) -> PathBuf {
    let fastboot = ksp.join("GameData").join("FastBoot").join("PluginData").join("Diagnostics");
    let research = ksp.join("GameData").join("BootProfiler").join("PluginData");
    if !fastboot.is_dir() && research.is_dir() { research } else { fastboot }
}

struct Options {
    ksp: Option<PathBuf>,
    out: Option<PathBuf>,
    env: Vec<(String, String)>,
    gc_log: bool,
    prewarm: bool,
    dry_run: bool,
    gpu_top: bool,
    command: Vec<String>,
}

fn parse_args() -> Result<Options, String> {
    let mut o = Options { ksp: None, out: None, env: Vec::new(), gc_log: true, prewarm: false, dry_run: false, gpu_top: false, command: Vec::new() };
    let mut args = std::env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{} needs a value", name));
        match a.as_str() {
            "--ksp" => o.ksp = Some(value("--ksp")?.into()),
            "--out" => o.out = Some(value("--out")?.into()),
            "--gc-heap" => o.env.push(("GC_INITIAL_HEAP_SIZE".into(), value("--gc-heap")?)),
            "--gc-divisor" => o.env.push(("GC_FREE_SPACE_DIVISOR".into(), value("--gc-divisor")?)),
            "--env" => {
                let kv = value("--env")?;
                let (k, v) = kv.split_once('=').ok_or("--env needs KEY=VALUE")?;
                o.env.push((k.into(), v.into()));
            }
            "--no-gc-log" => o.gc_log = false,
            "--prewarm" => o.prewarm = true,
            "--dry-run" => o.dry_run = true,
            "--gpu-top" => o.gpu_top = true,
            "--" => {
                o.command.extend(args.by_ref());
            }
            "-h" | "--help" => return Err(String::new()),
            _ if a.starts_with("--") => return Err(format!("unknown option {}", a)),
            _ => {
                // First bare argument starts the command (how Steam's %command% arrives).
                o.command.push(a);
                o.command.extend(args.by_ref());
            }
        }
    }
    Ok(o)
}

fn local_stamp() -> String {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTimeW {
        year: u16,
        month: u16,
        dow: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        ms: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(t: *mut SystemTimeW);
    }
    let mut t = SystemTimeW::default();
    unsafe { GetLocalTime(&mut t) };
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

/// Follows files as they grow and timestamps every complete line on arrival. `start` holds each file's size
/// before KSP was launched (0 for files created by this run), so nothing written early is skipped.
fn tail(paths: Vec<(PathBuf, u64, &'static str)>, t0: Instant, lines: Arc<Mutex<Vec<LogLine>>>, stop: Arc<AtomicBool>) {
    let mut pos: Vec<u64> = paths.iter().map(|p| p.1).collect();
    // The first bytes already read from each file. If they change, the file was replaced and is re-read from the
    // start. Only the captured prefix is compared, since a short file legitimately grows.
    let mut head: Vec<Vec<u8>> = paths.iter().map(|p| if p.1 > 0 { first_bytes(&p.0) } else { Vec::new() }).collect();
    let mut partial: Vec<String> = vec![String::new(); paths.len()];
    let mut buf = Vec::new();
    loop {
        let stopping = stop.load(Ordering::Relaxed);
        for (i, (p, _, source)) in paths.iter().enumerate() {
            let Ok(mut f) = std::fs::File::open(p) else { continue };
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            let replaced = pos[i] > 0 && !head[i].is_empty() && !first_bytes(p).starts_with(&head[i]);
            if len < pos[i] || replaced {
                pos[i] = 0; // truncated, recreated or overwritten
                partial[i].clear();
                head[i].clear();
            }
            if len == pos[i] || f.seek(SeekFrom::Start(pos[i])).is_err() {
                continue;
            }
            buf.clear();
            if f.read_to_end(&mut buf).is_err() {
                continue;
            }
            if pos[i] == 0 || head[i].len() < 256 {
                head[i] = first_bytes(p);
            }
            pos[i] += buf.len() as u64;
            let t = t0.elapsed().as_secs_f64();
            partial[i].push_str(&String::from_utf8_lossy(&buf));
            let mut out = lines.lock().unwrap();
            while let Some(nl) = partial[i].find('\n') {
                let line: String = partial[i].drain(..=nl).collect();
                let line = line.trim_end().to_string();
                if !line.is_empty() {
                    out.push(LogLine { t, text: if *source == "gc" { line } else { format!("[{}] {}", source, line) } });
                }
            }
        }
        if stopping {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn first_bytes(p: &Path) -> Vec<u8> {
    let mut buf = vec![0u8; 256];
    match std::fs::File::open(p).and_then(|mut f| f.read(&mut buf)) {
        Ok(n) => {
            buf.truncate(n);
            buf
        }
        Err(_) => Vec::new(),
    }
}

/// Reads GameData's large asset files once, at background CPU/IO priority, so a cold boot finds them cached.
fn prewarm(game_data: PathBuf) {
    win::background_thread();
    let mut files = Vec::new();
    let mut dirs = vec![game_data];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if matches!(p.extension().and_then(|x| x.to_str()).map(|x| x.to_ascii_lowercase()).as_deref(),
                Some("dds" | "mu" | "cfg" | "png" | "truecolor" | "mbm" | "wav" | "ogg" | "dll")) {
                files.push(p);
            }
        }
    }
    let mut buf = vec![0u8; 1 << 20];
    for f in files {
        if let Ok(mut h) = std::fs::File::open(&f) {
            while matches!(h.read(&mut buf), Ok(n) if n > 0) {}
        }
    }
}

fn newest_boot_profile(dir: &Path, since: SystemTime) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            n.starts_with("boot-") && n.ends_with(".md")
        })
        .filter_map(|p| std::fs::metadata(&p).and_then(|m| m.modified()).ok().filter(|m| *m >= since).map(|m| (m, p)))
        .max_by_key(|(m, _)| *m)
        .map(|(_, p)| p)
}

pub fn main() {
    let o = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("ksplaunch: {}", e);
            }
            eprintln!("{}", include_str!("main.rs").lines().take(22).filter_map(|l| l.strip_prefix("//!")).collect::<Vec<_>>().join("\n"));
            std::process::exit(2);
        }
    };
    if o.gpu_top {
        println!("{:>7}  {:<34} {:>10} {:>10}", "pid", "process", "dedicated", "shared");
        for (pid, name, d, sh) in telemetry::gpu_top().into_iter().filter(|r| r.2 >= 1.0 || r.3 >= 50.0).take(25) {
            println!("{:>7}  {:<34} {:>7.0} MB {:>7.0} MB", pid, name, d, sh);
        }
        return;
    }
    let command: Vec<String> = if o.command.is_empty() {
        let Some(ksp) = o.ksp.clone().or_else(default_ksp) else {
            eprintln!("ksplaunch: pass --ksp <KSP folder>, or put ksplaunch.exe next to KSP_x64.exe");
            std::process::exit(2);
        };
        vec![ksp.join("KSP_x64.exe").display().to_string()]
    } else {
        o.command.clone()
    };
    let exe = PathBuf::from(&command[0]);
    let ksp = o.ksp.clone().or_else(|| exe.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."));
    let profiler_dir = profiler_dir(&ksp);
    let out = o.out.clone().unwrap_or_else(|| profiler_dir.clone());
    let stamp = local_stamp();
    let gc_log = out.join(format!("gc-{}.log", stamp));

    let mut env = o.env.clone();
    if o.gc_log {
        env.push(("GC_PRINT_STATS".into(), "1".into()));
        env.push(("GC_LOG_FILE".into(), gc_log.display().to_string()));
    }
    if o.dry_run {
        println!("would run: {:?}\n  in: {}\n  reports: {}", command, ksp.display(), out.display());
        for (k, v) in &env {
            println!("  env {}={}", k, v);
        }
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("ksplaunch: cannot create {}: {}", out.display(), e);
        std::process::exit(1);
    }

    // Some Boehm builds write to gc.log or <exe>.gc.log in the working directory instead of GC_LOG_FILE, and
    // stderr is kept as a last resort: follow all of them, from where they ended before launch.
    let workdir = exe.parent().unwrap_or(&ksp).to_path_buf();
    let stderr_path = out.join(format!("ksp-stderr-{}.log", stamp));
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let exe_gc_log = workdir.join(format!("{}.gc.log", exe.file_stem().and_then(|s| s.to_str()).unwrap_or("KSP_x64")));
    let watch: Vec<(PathBuf, u64, &'static str)> = vec![
        (gc_log.clone(), 0, "gc"),
        (workdir.join("gc.log"), size(&workdir.join("gc.log")), "gc"),
        (exe_gc_log.clone(), size(&exe_gc_log), "gc"),
        (stderr_path.clone(), 0, "stderr"),
    ];
    let stderr_file = std::fs::File::create(&stderr_path).ok();

    let started_wall = SystemTime::now();
    let t0 = Instant::now();
    let mut child = match Command::new(&exe)
        .args(&command[1..])
        .current_dir(exe.parent().unwrap_or(&ksp))
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr_file.map(Stdio::from).unwrap_or_else(Stdio::null))
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ksplaunch: cannot start {}: {}", exe.display(), e);
            std::process::exit(1);
        }
    };
    println!("ksplaunch: started {} (pid {}); recording until it exits. Reports: {}", exe.display(), child.id(), out.display());

    let stop = Arc::new(AtomicBool::new(false));
    let lines = Arc::new(Mutex::new(Vec::<LogLine>::new()));
    let samples = Arc::new(Mutex::new(Vec::<telemetry::Sample>::new()));

    // Clock anchor: the launcher's timer starts just before CreateProcess; BootProfiler measures from the
    // process creation time. Shift every launcher timestamp by the difference.
    let creation_offset = unsafe {
        let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
        if win::GetProcessTimes(child.as_raw_handle() as win::HANDLE, &mut c, &mut e, &mut k, &mut u) != 0 {
            // FILETIME: 100 ns since 1601-01-01; UNIX epoch is 11644473600 s later.
            let created = c as f64 / 1e7 - 11_644_473_600.0;
            let launched = started_wall.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(created);
            created - launched
        } else {
            0.0
        }
    };
    let tailer = {
        let (lines, stop) = (lines.clone(), stop.clone());
        std::thread::spawn(move || tail(watch, t0, lines, stop))
    };
    let sampler = {
        let (samples, stop) = (samples.clone(), stop.clone());
        let handle = child.as_raw_handle() as usize;
        let pid = child.id();
        std::thread::spawn(move || {
            let mut tel = telemetry::Telemetry::new(handle as *mut std::ffi::c_void, pid);
            while !stop.load(Ordering::Relaxed) {
                let s = tel.sample(t0.elapsed().as_secs_f64());
                samples.lock().unwrap().push(s);
                std::thread::sleep(Duration::from_millis(250));
            }
        })
    };
    if o.prewarm {
        let gd = ksp.join("GameData");
        std::thread::spawn(move || prewarm(gd));
    }

    let status = child.wait();
    stop.store(true, Ordering::Relaxed);
    let _ = tailer.join();
    let _ = sampler.join();

    let exit = match status {
        Ok(s) => format!("{} after {:.1} s", s, t0.elapsed().as_secs_f64()),
        Err(e) => format!("wait failed: {}", e),
    };
    let shift = |t: f64| t - creation_offset;
    let lines: Vec<LogLine> = lines.lock().unwrap().iter().map(|l| LogLine { t: shift(l.t), text: l.text.clone() }).collect();
    let samples: Vec<telemetry::Sample> = samples.lock().unwrap().iter().map(|s| telemetry::Sample { t: shift(s.t), ..*s }).collect();
    let boot = newest_boot_profile(&profiler_dir, started_wall).and_then(|p| report::parse_boot_profile(&p));

    let mut tsv = String::from("t_s\tsource\ttext\n");
    for l in &lines {
        tsv.push_str(&format!("{:.3}\tgc\t{}\n", l.t, l.text.replace('\t', " ")));
    }
    tsv.push_str("\nt_s\tworking_set_mb\tprivate_mb\tpage_faults\tsys_avail_mb\tsys_commit_mb\tpages_input_per_s\tgpu_dedicated_mb\tgpu_shared_mb\tadapter_dedicated_mb\n");
    for s in &samples {
        tsv.push_str(&format!("{:.3}\t{:.0}\t{:.0}\t{}\t{:.0}\t{:.0}\t{:.1}\t{:.0}\t{:.0}\t{:.0}\n",
            s.t, s.working_set_mb, s.private_mb, s.page_faults, s.sys_avail_mb, s.sys_commit_mb,
            s.pages_input_per_s, s.gpu_dedicated_mb, s.gpu_shared_mb, s.adapter_dedicated_mb));
    }
    let md = report::build(&report::Inputs {
        started: stamp.clone(),
        command: command.join(" "),
        env: env.clone(),
        exit: format!("{}; process created {:.3} s after the launcher started its clock, times below are since creation", exit, creation_offset),
        gc_lines: &lines,
        samples: &samples,
        boot,
    });
    let md_path = out.join(format!("ksplaunch-{}.md", stamp));
    let tsv_path = out.join(format!("ksplaunch-{}.tsv", stamp));
    let _ = std::fs::write(&tsv_path, tsv);
    match std::fs::write(&md_path, md) {
        Ok(()) => println!("ksplaunch: wrote {}", md_path.display()),
        Err(e) => eprintln!("ksplaunch: cannot write {}: {}", md_path.display(), e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_tail(path: &Path, start: u64, write: impl FnOnce()) -> Vec<String> {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let t = {
            let (lines, stop, p) = (lines.clone(), stop.clone(), path.to_path_buf());
            std::thread::spawn(move || tail(vec![(p, start, "gc")], Instant::now(), lines, stop))
        };
        std::thread::sleep(Duration::from_millis(80));
        write();
        std::thread::sleep(Duration::from_millis(200));
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
        let out = lines.lock().unwrap().iter().map(|l| l.text.clone()).collect();
        out
    }

    #[test]
    fn new_log_is_read_once() {
        let p = std::env::temp_dir().join("ksplaunch-tail-new.log");
        let _ = std::fs::remove_file(&p);
        let got = run_tail(&p, 0, || {
            std::fs::write(&p, "a\n").unwrap();
            std::thread::sleep(Duration::from_millis(60));
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            std::io::Write::write_all(&mut f, b"b\n").unwrap();
        });
        assert_eq!(got, vec!["a", "b"]);
    }

    #[test]
    fn overwritten_existing_log_is_reread_from_start() {
        let p = std::env::temp_dir().join("ksplaunch-tail-old.log");
        std::fs::write(&p, "old\n").unwrap();
        let got = run_tail(&p, 4, || std::fs::write(&p, "new one\nnew two, longer than before\n").unwrap());
        assert_eq!(got, vec!["new one", "new two, longer than before"]);
    }
}
