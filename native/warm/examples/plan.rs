//! Prints the warm-up plan for a KSP install without reading any file contents:
//!
//!     cargo run --release -p fastboot-warm --example plan -- <KSP folder> [read list]
//!
//! Shows how much would be read, in which order, against this machine's memory budget.

use fastboot_warm::{plan, readlist, system};
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next().map(PathBuf::from) else {
        eprintln!("usage: plan <KSP folder> [read list]");
        std::process::exit(2);
    };
    let list = args.next().map(|p| readlist::load(p.as_ref())).unwrap_or_default();
    let t = std::time::Instant::now();
    let p = plan::build(&root, &list);
    let ms = t.elapsed().as_millis();
    let mut total = 0u64;
    let mut by_ext: std::collections::BTreeMap<String, (usize, u64)> = Default::default();
    for f in &p.files {
        let len = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        total += len;
        let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let e = by_ext.entry(ext).or_default();
        e.0 += 1;
        e.1 += len;
    }
    println!("{} files, {:.0} MiB ({} from the read list); plan built in {} ms", p.files.len(), total as f64 / 1048576.0, p.from_readlist, ms);
    println!("budget {:.0} MiB, {} reader thread(s)", system::budget() as f64 / 1048576.0, system::reader_threads(&root));
    for (ext, (n, bytes)) in by_ext {
        println!("  .{:<10} {:6} files {:8.0} MiB", ext, n, bytes as f64 / 1048576.0);
    }
    for f in p.files.iter().take(5) {
        println!("  first: {}", f.display());
    }
}
