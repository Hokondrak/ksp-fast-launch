# FastBoot offline AOT compiler driver

Experimental Windows x64 tool that compiles copies of installed managed assemblies with **that installation's Mono**, then invokes FastBoot's `aotlink`. It does not run Unity, KSP's entry point, or the target assembly's `Main`. Nothing is installed automatically. All generated files go into the explicitly selected cache.

The driver requires no .NET SDK, Rust, LLVM, or external Mono on the player's machine. The developer builds `aot-compile.exe` with Rust and ships it alongside `aotlink.exe`. It uses Windows BCrypt for SHA-256 and reads assembly metadata directly.

## Build and use

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release

./target/release/aot-compile.exe plan --ksp 'D:\Games\KSP-Jool-Odyssey'

./target/release/aot-compile.exe build `
  --ksp 'D:\Games\KSP-Jool-Odyssey' `
  --cache 'C:\Users\Maxim\AppData\Local\FastBoot\AotCache' `
  --linker 'C:\Tools\FastBoot\aotlink.exe' `
  --assembly 'D:\Games\KSP-Jool-Odyssey\KSP_x64_Data\Managed\Assembly-CSharp.dll'

./target/release/aot-compile.exe verify `
  --ksp 'D:\Games\KSP-Jool-Odyssey' `
  --cache 'C:\Users\Maxim\AppData\Local\FastBoot\AotCache' `
  --linker 'C:\Tools\FastBoot\aotlink.exe'
```

These are examples, not installation actions performed by this handoff. `build` requires one or more explicit `--assembly` arguments. `plan` lists method counts and dependency eligibility; counts are **not** a ranking of startup cost. Use the measured per-assembly startup JIT profile to select targets. Assembly-CSharp is a useful first integration test; framework/Harmony/mod assemblies need their own compatibility testing.

Exit codes: 0 success/cache hit; 3 partial build with usable images and per-assembly failures; 1 rejected/deferred/error; 2 may be returned by Mono. `verify` returns 0 and prints the verified image directory on success. It returns nonzero on stale, missing, damaged or unsupported state. A partial generation is valid only for its successful outputs. Failed targets use JIT. Reusing an unchanged partial generation does not automatically retry failed targets; use a new cache root to retry in this prototype.

## Scheduling and isolation

Launch the builder as a separate hidden, below-normal-priority process **after game exit**, following a successful profiled boot. It refuses to build while any KSP/KSP_x64 process exists or while physical/commit headroom is below 3 GiB. It checks again between assemblies and while children run. Low priority does not protect gameplay from memory or I/O contention, so “after reaching the main menu” is not the recommended automatic trigger.

One child compiles one assembly; a second child links it. Each has a default 180-second deadline (`--timeout` overrides), a 2 GiB process memory limit, and a kill-on-controller-exit Windows job. Workers have no visible windows. A compiler assertion, crash, timeout, missing dependency or rejected assembly syntax does not abort other targets. Per-assembly compile/link logs remain inspectable. An exclusive Windows file handle prevents concurrent cache writers and is released automatically after a crash; the persistent `build.lock` filename is harmless.

The compiler worker calls `mono_main` with:

```
--optimize=-inline
--aot=asmonly,bind-to-runtime-version,threads=1,no-direct-calls,outfile=image.s
```

This is normal partial AOT with JIT fallback. It does not set `full`, `hybrid`, `static`, LLVM or direct P/Invoke options. Explicit `hybrid` has additional generic-sharing behavior in this Mono version. The relative output filename avoids Mono's comma-separated option parser misreading arbitrary user paths. Inherited `MONO_*` environment variables are removed in the compiler child.

## Dependency resolution

The inventory covers managed binaries, assembly configuration/debug sidecars under `Managed` and `GameData`, Mono configuration, the actual Mono DLL, UnityPlayer and the game executable. Explicit external test targets and their config files are also included. Managed assembly names, references, method counts and MVID bytes are read through a bounded PE/CLI metadata parser.

Every unique assembly provider is copied into a private reference directory under its **assembly name**, so filenames such as `ModuleManager.4.2.3.dll` resolve as `ModuleManager.dll`. Identical duplicate providers collapse by content. Ambiguous differing providers or missing transitive references make a target ineligible, conservatively; this can exclude mods whose unused references never load during ordinary gameplay. Reparse points under the scanned trees are rejected rather than silently traversed or omitted. These are explicit prototype limitations, not proof those mods are broken.

The target is compiled from a private copy retaining its original filename. Dependency-copy hashes must match the inventory. No installation file is rewritten. Generated native filenames are `Original.dll.dll` or `Original.exe.dll` to match Mono's lookup convention. Only validated successful images enter the `images` directory; failed linker output remains outside it.

## Cache identity and publication

The key covers exact contents of all inventoried input files, canonical source paths, runtime binary, driver binary, linker binary, compiler options, CPU feature bits, OS-enabled extended processor state and target selection. This is deliberately a conservative whole-install epoch. A mod dependency change may invalidate more images than strictly necessary. Size, timestamp, runtime version text and MVID alone are insufficient.

After compilation, the driver re-enumerates and hashes the complete inputs. If anything changed, it refuses to publish. A generation is renamed from a private staging directory, then an atomic replace updates `current.txt`. Existing published images are never overwritten. Partial builds publish only successes. Unpublished diagnostic directories and old generations are retained; automatic garbage collection is not implemented, so this prototype can consume substantial disk space.

`verify` reconstructs the identity from current inputs, checks all native output hashes, rejects unlisted image files and unsafe paths, and checks the exact selected target set recorded by the manifest. It took roughly 1.1–1.4 seconds on the warm local installation during testing. Include verification and native-image loading costs in the startup budget.

Manifest format is UTF-8 TSV, first line `FASTBOOT-AOT-1`. `key` is the identity hash; `target` contains a hex-encoded UTF-8 source path; `output` contains hex-encoded filename plus native SHA-256; `failed` contains hex-encoded path and reason. `identity.tsv` contains the exact human-auditable hash input. Unknown manifest records are ignored. The manifest is local cache state, not a signed trust boundary.

## Game-side integration contract

**Do not pass `--aot-path` to `mono_jit_parse_options`.** The shipped runtime exits on it. That option belongs to the executable driver's `mono_main` parser, which Unity does not use for its normal embedding initialization.

The separately supplied `aot-loader.rs` demonstrates `mono_dl_fallback_register`, registered before runtime init. Supply only mappings from the verified manifest: exact original assembly path plus `.dll` → exact native image path. Keep the callbacks/context alive for the Mono runtime lifetime. Decline every other lookup so normal native plugin resolution is unaffected. A missing image or unavailable callback export should leave ordinary JIT in place. Do not copy these DLLs into GameData or alter global DLL search paths to expose the cache.

Mono still validates the assembly MVID, dependency MVIDs, format version, GC/safepoints, supported CPU/SIMD options, debugging compatibility and (with `bind-to-runtime-version`) its build string. Our exact runtime/content keys supplement those checks. The loader must resolve managed dependencies at image-load time; a resolver installed only after `Main` can cause AOT rejection. Tests that merely exit successfully can therefore be false positives. Require actual AOT counters and `FOUND method` evidence.

## Compatibility boundary

Default AOT inlined an unmarked target before Harmony patched it: JIT returned the patched value 11, while an AOT caller returned the stale value 7. `--optimize=-inline` fixes that reproduced case. `no-direct-calls` also preserves runtime method lookup for this conservative prototype.

Disabling inlining across an entire assembly can reduce gameplay performance. Do not enable whole-assembly AOT by default on the strength of these offline checks. Measure complete startup **and** representative flight performance; consider Mono's `profile=`/`profile-only` mode to limit AOT to startup methods, leaving other methods to normal optimized JIT. Profile selection and full-game testing are not implemented here.

The companion evidence includes actual loaded native methods, generics/exceptions/PInvoke, Harmony prefix/postfix/transpiler/finalizer/unpatch tests, real ConfigNode operations from Assembly-CSharp, source/output/tool mutation checks, and worker failure isolation. It does not prove every Unity internal call, Harmony patch combination, mod load order or gameplay path.

## Integrating into FastBoot's workspace

Copy this crate to `tools/aot-compile`, remove its standalone `[workspace]` table, and add it to the root workspace members. Existing explicit package metadata can stay. Build the binary in the normal packaging pipeline and keep its exact hash in cache identity. The native loader prototype and offline tests are separate files in the round-5 handoff. The current linker still needs the fail-closed corrections in the accompanying review before release.
