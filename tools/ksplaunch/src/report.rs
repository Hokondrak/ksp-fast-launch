//! Turns the recorded timeline into a summary, and lines it up with BootProfiler's freezes.
//! All times are seconds since ksplaunch started KSP.

use crate::telemetry::Sample;
use std::fmt::Write as _;

#[derive(Clone, Debug)]
pub struct LogLine {
    pub t: f64,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PauseKind {
    /// "World-stopped marking took N msecs": every thread, the main thread included, is stopped.
    StopTheWorld,
    /// "Complete collection took N msecs": the whole collection, marking plus sweeping.
    Collection,
}

#[derive(Clone, Copy, Debug)]
pub struct Pause {
    pub end: f64,
    pub ms: f64,
    pub kind: PauseKind,
}

impl Pause {
    pub fn start(&self) -> f64 {
        self.end - self.ms / 1000.0
    }
}

fn number_after(text: &str, marker: &str) -> Option<f64> {
    let i = text.find(marker)? + marker.len();
    let digits: String = text[i..].chars().skip_while(|c| *c == ' ').take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// The pause a Boehm GC_PRINT_STATS line reports, if any. The line is printed when the phase ends.
pub fn parse_pause(line: &LogLine) -> Option<Pause> {
    let kind = if line.text.contains("World-stopped marking took") {
        PauseKind::StopTheWorld
    } else if line.text.contains("Complete collection took") {
        PauseKind::Collection
    } else {
        return None;
    };
    number_after(&line.text, "took").map(|ms| Pause { end: line.t, ms, kind })
}

#[derive(Clone, Debug)]
pub struct Freeze {
    pub start: f64,
    pub seconds: f64,
    pub after: String,
}

#[derive(Clone, Debug, Default)]
pub struct BootProfile {
    pub path: String,
    pub process_age: f64,
    pub clock_at_write: f64,
    pub status: String,
    pub freezes: Vec<Freeze>,
}

/// Reads the parts of a BootProfiler report this summary needs. Its clock starts when BootProfiler loaded,
/// `process_age` seconds after the process started.
pub fn parse_boot_profile(path: &std::path::Path) -> Option<BootProfile> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut p = BootProfile { path: path.display().to_string(), ..Default::default() };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Status: **") {
            p.status = rest.trim_end_matches(|c| c == '*' || c == ' ').to_string();
        }
        if line.starts_with("Profiler clock:") {
            p.clock_at_write = number_after_float(line, "Profiler clock:").unwrap_or(0.0);
            p.process_age = number_after_float(line, "process was").unwrap_or(0.0);
        }
        // "- **73.71s** @144.6s after: <log line>  → next: ..."
        if let Some(rest) = line.strip_prefix("- **") {
            let secs = rest.split('s').next().and_then(|s| s.parse::<f64>().ok());
            let at = rest.find('@').and_then(|i| rest[i + 1..].split('s').next()).and_then(|s| s.parse::<f64>().ok());
            if let (Some(seconds), Some(at)) = (secs, at) {
                let after = rest.split("after: ").nth(1).unwrap_or("").split("  \u{2192}").next().unwrap_or("").to_string();
                p.freezes.push(Freeze { start: at + p.process_age, seconds, after });
            }
        }
    }
    Some(p)
}

fn number_after_float(text: &str, marker: &str) -> Option<f64> {
    let i = text.find(marker)? + marker.len();
    let s: String = text[i..].chars().skip_while(|c| *c == ' ').take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    s.parse().ok()
}

/// Formats a value, or an en dash when there is none.
pub fn num(v: f64, decimals: usize) -> String {
    if v.is_finite() { format!("{:.*}", decimals, v + 0.0) } else { "\u{2013}".to_string() }
}

fn overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

fn window<'a>(samples: &'a [Sample], t0: f64, t1: f64) -> impl Iterator<Item = &'a Sample> {
    samples.iter().filter(move |s| s.t >= t0 - 0.3 && s.t <= t1 + 0.3)
}

fn max_of(it: impl Iterator<Item = f64>) -> f64 {
    it.filter(|v| v.is_finite()).fold(f64::NAN, f64::max)
}

fn min_of(it: impl Iterator<Item = f64>) -> f64 {
    it.filter(|v| v.is_finite()).fold(f64::NAN, f64::min)
}

pub struct Inputs<'a> {
    pub started: String,
    pub command: String,
    pub env: Vec<(String, String)>,
    pub exit: String,
    pub gc_lines: &'a [LogLine],
    pub samples: &'a [Sample],
    pub boot: Option<BootProfile>,
}

pub fn build(i: &Inputs) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# ksplaunch session\n");
    let _ = writeln!(s, "Started: {}  \nCommand: `{}`  \nExit: {}  ", i.started, i.command, i.exit);
    for (k, v) in &i.env {
        let _ = writeln!(s, "Env: `{}={}`  ", k, v);
    }
    let pauses: Vec<Pause> = i.gc_lines.iter().filter(|l| !l.text.starts_with("[stderr]")).filter_map(parse_pause).collect();
    let stw: Vec<&Pause> = pauses.iter().filter(|p| p.kind == PauseKind::StopTheWorld).collect();
    let full: Vec<&Pause> = pauses.iter().filter(|p| p.kind == PauseKind::Collection).collect();
    let boot_end = i.boot.as_ref().filter(|b| b.status == "main menu reached").map(|b| b.clock_at_write + b.process_age);

    let _ = writeln!(s, "\n## Garbage collection (from the GC's own statistics log)\n");
    if !i.gc_lines.iter().any(|l| !l.text.starts_with("[stderr]")) {
        let _ = writeln!(s, "No GC statistics lines were captured. Either this Mono build ignores GC_LOG_FILE, or it wrote \
            them elsewhere (check gc.log in the KSP folder). GC conclusions below are therefore unavailable.");
    } else {
        let in_boot = |p: &&Pause| boot_end.map_or(true, |e| p.end <= e);
        let sum = |v: &[&Pause]| v.iter().map(|p| p.ms).sum::<f64>() / 1000.0;
        let boot_stw: Vec<&Pause> = stw.iter().copied().filter(in_boot).collect();
        let boot_full: Vec<&Pause> = full.iter().copied().filter(in_boot).collect();
        let _ = writeln!(s, "| | Count | Total (s) | Longest (ms) |\n|---|---:|---:|---:|");
        let _ = writeln!(s, "| World-stopped marking, whole session | {} | {} | {} |", stw.len(), num(sum(&stw), 1), num(max_of(stw.iter().map(|p| p.ms)), 0));
        let _ = writeln!(s, "| Complete collections, whole session | {} | {} | {} |", full.len(), num(sum(&full), 1), num(max_of(full.iter().map(|p| p.ms)), 0));
        if let Some(e) = boot_end {
            let _ = writeln!(s, "| World-stopped marking, boot (to {:.0} s) | {} | {} | {} |", e, boot_stw.len(), num(sum(&boot_stw), 1), num(max_of(boot_stw.iter().map(|p| p.ms)), 0));
            let _ = writeln!(s, "| Complete collections, boot | {} | {} | {} |", boot_full.len(), num(sum(&boot_full), 1), num(max_of(boot_full.iter().map(|p| p.ms)), 0));
        }
        let mut worst: Vec<&Pause> = pauses.iter().collect();
        worst.sort_by(|a, b| b.ms.partial_cmp(&a.ms).unwrap());
        let _ = writeln!(s, "\nLongest pauses:\n");
        for p in worst.iter().take(12) {
            let _ = writeln!(s, "- {:.0} ms {:?}, ended at {:.1} s", p.ms, p.kind, p.end);
        }
        let grows = i.gc_lines.iter().filter(|l| l.text.contains("Grow heap to")).count();
        let _ = writeln!(s, "\nHeap growth events: {}. Last GC heap line: {}", grows,
            i.gc_lines.iter().rev().find(|l| l.text.contains("heap ")).map(|l| l.text.as_str()).unwrap_or("-"));
    }

    let _ = writeln!(s, "\n## Memory and GPU (sampled every 250 ms from outside the game)\n");
    if let Some(last) = i.samples.last() {
        let _ = writeln!(s, "| Peak | Value |\n|---|---:|");
        let _ = writeln!(s, "| KSP commit (private) | {} MB |", num(max_of(i.samples.iter().map(|x| x.private_mb)), 0));
        let _ = writeln!(s, "| KSP working set | {} MB |", num(max_of(i.samples.iter().map(|x| x.working_set_mb)), 0));
        let _ = writeln!(s, "| KSP dedicated GPU memory | {} MB |", num(max_of(i.samples.iter().map(|x| x.gpu_dedicated_mb)), 0));
        let _ = writeln!(s, "| KSP shared GPU memory (GPU-visible system RAM) | {} MB |", num(max_of(i.samples.iter().map(|x| x.gpu_shared_mb)), 0));
        let _ = writeln!(s, "| KSP's adapter, dedicated, all processes | {} MB |", num(max_of(i.samples.iter().map(|x| x.adapter_dedicated_mb)), 0));
        let _ = writeln!(s, "| Page-ins, whole system (not KSP-specific) | {} /s |", num(max_of(i.samples.iter().map(|x| x.pages_input_per_s)), 0));
        let _ = writeln!(s, "| Lowest available RAM | {} MB |", num(min_of(i.samples.iter().map(|x| x.sys_avail_mb)), 0));
        let _ = writeln!(s, "| System commit | {:.0} of {:.0} MB |", max_of(i.samples.iter().map(|x| x.sys_commit_mb)), last.sys_commit_limit_mb);
    } else {
        let _ = writeln!(s, "No samples recorded.");
    }

    let _ = writeln!(s, "\n## BootProfiler freezes, with what was happening outside the game\n");
    match &i.boot {
        None => {
            let _ = writeln!(s, "No BootProfiler report from this session was found.");
        }
        Some(b) => {
            let _ = writeln!(s, "Report: `{}` ({}). Times are seconds since KSP started.\n", b.path, b.status);
            let _ = writeln!(s, "| Freeze | Length | GC stop-the-world inside | GC collections inside | KSP dedicated GPU | KSP shared GPU | Adapter dedicated | Page-ins/s (max) | Avail RAM (min) | After |");
            let _ = writeln!(s, "|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|");
            let mut freezes = b.freezes.clone();
            freezes.sort_by(|a, b| b.seconds.partial_cmp(&a.seconds).unwrap());
            for f in freezes.iter().filter(|f| f.seconds >= 1.0).take(20) {
                let (t0, t1) = (f.start, f.start + f.seconds);
                let stw_in: f64 = stw.iter().map(|p| overlap(p.start(), p.end, t0, t1)).sum();
                let full_in: f64 = full.iter().map(|p| overlap(p.start(), p.end, t0, t1)).sum();
                let w: Vec<&Sample> = window(i.samples, t0, t1).collect();
                let _ = writeln!(s, "| {:.1} s | {:.1} s | {:.1} s | {:.1} s | {} MB | {} MB | {} MB | {} | {} MB | {} |",
                    t0, f.seconds, stw_in, full_in,
                    num(max_of(w.iter().map(|x| x.gpu_dedicated_mb)), 0),
                    num(max_of(w.iter().map(|x| x.gpu_shared_mb)), 0),
                    num(max_of(w.iter().map(|x| x.adapter_dedicated_mb)), 0),
                    num(max_of(w.iter().map(|x| x.pages_input_per_s)), 0),
                    num(min_of(w.iter().map(|x| x.sys_avail_mb)), 0),
                    f.after.chars().take(70).collect::<String>().replace('|', "/"));
            }
            let _ = writeln!(s, "\nThese are correlations, not diagnoses. GC intervals are reconstructed from when each statistics line \
                was observed minus the duration it reports; an abandoned incremental marking attempt prints no summary, so missing \
                GC time does not rule GC out. Shared GPU memory is ordinary GPU-visible system RAM, not necessarily evicted video \
                memory, and page-ins are system-wide. A freeze mostly covered by stop-the-world time is consistent with GC; dedicated \
                memory pinned near the adapter's capacity with shared memory rising is consistent with video-memory pressure, which \
                a VidMm ETW trace (GPUView) can confirm.");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_boehm_pause_lines() {
        let l = |t: f64, s: &str| LogLine { t, text: s.to_string() };
        let p = parse_pause(&l(10.0, "World-stopped marking took 734 msecs (12 in average)")).unwrap();
        assert_eq!((p.ms, p.kind), (734.0, PauseKind::StopTheWorld));
        assert!((p.start() - 9.266).abs() < 1e-9);
        assert_eq!(parse_pause(&l(1.0, "Complete collection took 1203 msecs")).unwrap().ms, 1203.0);
        assert!(parse_pause(&l(1.0, "Grow heap to 1048576 KiB after 2000 bytes allocated")).is_none());
    }

    #[test]
    fn parses_bootprofiler_freezes() {
        let dir = std::env::temp_dir().join("ksplaunch-test-bp.md");
        std::fs::write(&dir, "# KSP boot profile\n\nStatus: **main menu reached**  \n\
            Profiler clock: 366.6s since BootProfiler loaded; process was 29.1s old at that point, so desktop-to-now is about 395.7s.  \n\
            - **73.71s** @144.6s after: Load DDS texture: X/y  \u{2192} next: Load DDS texture: X/z\n").unwrap();
        let b = parse_boot_profile(&dir).unwrap();
        assert_eq!(b.status, "main menu reached");
        assert!((b.process_age - 29.1).abs() < 1e-9 && (b.clock_at_write - 366.6).abs() < 1e-9);
        assert_eq!(b.freezes.len(), 1);
        assert!((b.freezes[0].start - 173.7).abs() < 1e-9 && (b.freezes[0].seconds - 73.71).abs() < 1e-9);
        assert_eq!(b.freezes[0].after, "Load DDS texture: X/y");
    }
}
