//! aot-compile: builds FastBoot's partial-AOT cache with the KSP installation's own Mono, in a separate process,
//! and links each image with aotlink. Written by Codex; see README.md for the CLI, cache format and scheduling rules.

#[cfg(windows)]
mod app;

#[cfg(windows)]
fn main() {
    app::main()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("aot-compile supports Windows KSP installs only.");
    std::process::exit(1);
}
