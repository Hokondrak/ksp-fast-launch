//! An assembler and linker for exactly what Mono's AOT compiler emits on Windows x64 with `--aot=asmonly`.
//!
//! Mono writes machine code as `.byte` data; the only instructions in its output are `call <label>` and
//! `jmp *<symbol>+<n>(%rip)`. Everything else is data directives, labels, label arithmetic, `@IMGREL` and, in the
//! CodeView debug section, `.secrel32`/`.secidx`. That subset is small enough to turn straight into a PE DLL here,
//! so FastBoot can build native-code caches on a player's machine without shipping an assembler or linker.
//!
//! Anything outside the subset is an error with its line number: an image that might be subtly wrong is never
//! written. Mono itself then validates the image when it loads it (assembly and dependency GUIDs, runtime and GC
//! settings) and falls back to its JIT if anything does not match.
//!
//! - [`asm`] parses and assembles the text into sections, symbols and fixups.
//! - [`pe`] lays out the sections and writes the DLL: base relocations for 64-bit addresses, `.pdata` as the
//!   exception directory, and an export table for the global symbols (`mono_aot_file_info`).

pub mod asm;
pub mod expr;
pub mod pe;

/// Assembles `source` (the text of an `--aot=asmonly` output) into the bytes of a DLL named `dll_name`.
pub fn link(source: &str, dll_name: &str) -> Result<Vec<u8>, String> {
    let module = asm::assemble(source)?;
    pe::write(&module, dll_name)
}
