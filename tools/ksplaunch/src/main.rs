//! ksplaunch: start KSP with the Boehm GC's own statistics switched on, and record from outside the game what
//! garbage collection, video memory and system memory were doing while it loaded.
//!
//! Usage:
//!   ksplaunch.exe [options]                       launches <ksp>\KSP_x64.exe
//!   ksplaunch.exe [options] -- <command ...>      launches the command (Steam: `"...\ksplaunch.exe" %command%`)
//! Options:
//!   --ksp <dir>          KSP folder (default: the command's folder, else the folder ksplaunch.exe is in)
//!   --out <dir>          where reports go (default: <ksp>\GameData\FastBoot\PluginData\Diagnostics)
//!   --gc-heap <size>     GC_INITIAL_HEAP_SIZE, e.g. 4G: start with a large heap so far fewer collections run
//!   --gc-divisor <n>     GC_FREE_SPACE_DIVISOR (default 3; lower grows the heap more eagerly between collections)
//!   --no-gc-log          do not enable GC statistics
//!   --env KEY=VALUE      any extra environment variable for KSP
//!   --prewarm            read GameData into the OS file cache in the background while KSP starts (cold boots)
//!   --dry-run            print what would be launched and exit
//!   --gpu-top            list processes by GPU memory right now (read-only) and exit
//!
//! Nothing is injected into KSP and no game file is changed; the variables only affect this launch.

// The report is platform-neutral so its tests run everywhere; only the Windows launcher uses it.
#[cfg_attr(not(windows), allow(dead_code))]
mod report;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod telemetry;
#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    app::main();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("ksplaunch only runs on Windows: it reads Windows GPU and memory performance counters.");
    std::process::exit(1);
}
