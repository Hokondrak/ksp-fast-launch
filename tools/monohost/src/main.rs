//! monohost: run a .NET program inside KSP's own embedded Mono runtime, without starting the game.
//!
//! Desktop .NET Framework is only a proxy for Unity's Mono: string comparison, collation tables, trimming and
//! file decoding can differ. monohost loads KSP's `mono-2.0-bdwgc.dll`, points it at KSP's `Managed` folder
//! (Unity's own mscorlib) and runs a program's Main, so FastBoot's offline tests can run on the exact runtime the
//! game uses. Nothing Unity-native is available, so only code that does not call into the engine works.
//!
//! Usage: monohost --ksp <KSP folder> [--preload <dll>]... [--aot-path <dir>]... [--aot-asm <out.s>] -- <program.exe> [args...]
//!
//! `--aot-asm` runs the runtime's own AOT compiler on the program (`--aot=asmonly`) instead of running it, and
//! `--aot-path` adds a folder Mono searches for AOT images; with aotlink these make the native-code cache testable
//! without the game.
//!
//! Mono's functions are resolved through kernel32's `GetProcAddress` from monohost's own import table, as
//! Unity's player does, so `--preload` with FastBoot's `version.dll` (and `FASTBOOT_EARLY_TARGET=monohost.exe`)
//! runs the whole early-start chain without the game.

#[cfg(windows)]
mod host;

#[cfg(windows)]
fn main() {
    std::process::exit(host::main());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("monohost currently supports Windows KSP installs only.");
    std::process::exit(1);
}
