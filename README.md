# FastBoot

A Kerbal Space Program 1.12 mod that shortens the time from desktop to main menu on heavily modded installs,
without changing what the game loads.

> **Status: pre-release, one machine.** Everything below passes offline tests against a large real install
> (the Jool Odyssey modpack), including inside KSP's own Mono runtime, and has run in the game on that install.
> It has not been tested on any other machine or install yet.

## Measured

Reference install: KSP 1.12.5, Jool Odyssey modpack (1,489 parts, 4,126 cfg files, 10.8 GB GameData), Ryzen 9
7945HX, 32 GB RAM, NVMe SSD. Desktop to main menu, from BootProfiler:

| Launch | Without FastBoot | With FastBoot (early start) |
| --- | ---: | ---: |
| Cold (first launch after a reboot) | 139 s | **65 s** |
| Warm (files already cached) | 61–62 s | not yet measured cleanly |
| After a mod change | 88–92 s | **77 s** (drag cube restore) |

Part compilation went from 10.8 s to 8.5 s with fast sound lookups (16.7 s to 13.1 s with the profiler on, which
slows it). After a mod change it went from 20–25 s to 8.5 s, because 1,461 of 1,474 parts' drag cubes were restored
instead of rendered again.

The cold launch is now about as fast as a warm one. With FastBoot, the warm-up read 12.6 GB on 8 threads in
13.8 s, and the config parser served 8,032 loads across both config passes in 2.0 s of main-thread time. The
game compiled the same 1,489 parts and 586 internal spaces as without it. One run each, so read the numbers as
indicative.

## What it does

| Part | What | Where |
| --- | --- | --- |
| Config parser | Serves `ConfigNode.Load` for GameData `.cfg` files from a native Rust parser (`kspcfg`), pre-parsing every file on worker threads. Tree building stays KSP's own, so the result is identical to stock; files outside the parser's verified fast path go to stock (or KSPCommunityFixes) unchanged. | `GameData/FastBoot` |
| Fast sound lookups | KSP finds a sound by name by checking every loaded sound in turn, asking the engine for each one's name. FastBoot answers from an index with the same result (fuzz-tested against stock). It matters when mods look up many sounds while parts load: on the reference install that was 30% of part compilation. | `GameData/FastBoot` |
| Cold-start warm-up | On a launch after a reboot, KSP mostly waits for the disk: 139 s against 61 s warm on the reference install, because KSP and Unity read files one at a time. FastBoot reads the same files ahead of the game on 8 threads (1 on a spinning disk), in the order the game will want them, so the game's reads come from memory. With the early-start loader it starts at process start and records which files the boot opens, so the next launch warms exactly those. It reads at most half the free memory and writes nothing but its read list. | `GameData/FastBoot`, best with `EarlyStart/version.dll` |
| Drag cube cache (opt-in, `DragCubeCache = restore`) | After a mod change ModuleManager deletes `PartDatabase.cfg` and KSP renders every part's drag cubes again (+9.4 s here). FastBoot restores the cubes of parts whose inputs are unchanged and that KSP has rendered identically twice; changed, unverified and animated parts are rendered by KSP as usual. | `GameData/FastBoot` |
| Terrain quad pool (`DeferTerrainCache`) | KSP pre-builds 9,216 terrain quads during planet setup. FastBoot builds the same pool after the loading screen, a few milliseconds per frame. Measured saving: about 0.4 s (the quads took 321-423 ms when built later; most of the planet-setup stall is other work). | `GameData/FastBoot` |
| Early start (optional) | A `version.dll` placed next to `KSP_x64.exe` that starts the config parser *before* KSP's first config scan, which runs before any GameData plugin exists. Without it, only the second scan benefits. | `EarlyStart/version.dll` |
| Diagnostics (off by default) | A boot timing report per launch; a stall sampler for freezes; `Profile = parts` or `boot` for continuous main-thread sampling; `MonoCounters = true` for Mono's JIT time and a per-assembly JIT breakdown in `early.log`. | `GameData/FastBoot/PluginData/Diagnostics` |
| `ksplaunch` | A measuring launcher: starts KSP, follows its logs and records CPU, memory, disk and GPU per loading phase. | `tools/ksplaunch.exe` |

### Why configs

A modded KSP parses every GameData config twice, serially, on the main thread: once in the stock loader
before plugins load, and again through KSPCommunityFixes. On the reference install that is about 12 s plus 7 s.
Stock `PreFormatConfig` is quadratic in places and culture-sensitive, so most of that time is text handling,
not disk.

Offline, under KSP's own Mono 5.11, parsing all 4,126 cfg files of the reference install takes 19.4 s with
the stock code and 5.7 s through kspcfg. That is a measurement of the parser alone, not of a game launch.

### How the result is kept identical

- kspcfg returns UTF-16 text and token offsets. The C# side turns them into the `List<string[]>` that stock
  `ConfigNode.RecurseFormat` consumes, so node names, ids and value sanitising are KSP's code.
- The fast path covers an allowlist of Unicode code points, minus a table of 138 code points where Unity's Mono
  disagrees with the parser. The table was derived by testing every allowlisted code point inside KSP's
  runtime. Anything else makes kspcfg decline the file, and stock parses it.
- The test suite compares every cfg of a real install against stock, tree by tree, in three addon orders, on
  desktop .NET and inside KSP's Mono, plus 200,000 fuzzed inputs.

### Compatibility

- **KSPCommunityFixes**: supported in any load order. FastBoot's prefix runs first. KSPCF's own `ConfigNode.Load`
  prefix is patched to step aside for files FastBoot already served, so nothing is parsed twice. KSPCF's other
  ConfigNode patches keep working.
- **KspFastCfg** and **BootProfiler** (the research prototypes this mod grew from): FastBoot detects them and leaves
  the corresponding feature to them. Remove KspFastCfg: it never loaded its native library in game.
- **ModuleManager**: unaffected. `ModuleManager.ConfigCache` loads to the same tree as before.

## Install (once released)

1. Copy `GameData/FastBoot` into KSP's `GameData`. Requires [HarmonyKSP](https://github.com/KSPModdingLibs/HarmonyKSP).
2. Optional, Windows only: copy `EarlyStart/version.dll` next to `KSP_x64.exe`.

To switch the warm-up off, set `Warmup = false`. To switch the early start off, delete `version.dll`, create an empty `GameData/FastBoot/PluginData/disable-early`,
or set `FASTBOOT_EARLY_DISABLE=1`. Each launch's early-start log is `GameData/FastBoot/PluginData/early.log`.

Settings are in `GameData/FastBoot/FastBoot.cfg`:

| Setting | Default | Meaning |
| --- | --- | --- |
| `ConfigParser` | `true` | Use the native config parser. |
| `ConfigParserThreads` | `0` | Pre-parse threads; 0 = CPU threads − 2, at most 8. |
| `Warmup` | `true` | Cold-start warm-up. |
| `FastLookups` | `true` | Index for sound lookups by name. |
| `DeferTerrainCache` | `true` | Build the terrain quad pool after the loading screen. |
| `DragCubeCache` | `off` | `restore` reuses verified drag cubes after a mod change; `check` only reports. |
| `MonoCounters` | `false` | Mono's JIT counters and a per-assembly JIT breakdown in `early.log`. |
| `Profile` | (empty) | `parts`: sample part compilation every 2 ms and write a report to `PluginData/Diagnostics` (Windows). |
| `Diagnostics` | `false` | Write a boot timing report per launch. |
| `StallSampler` | `false` | With Diagnostics: sample the main thread during freezes (Windows). |

## Not done yet

Open, and waiting on measurements or analysis:

- **Precompiled managed code (AOT)**, on the `claude/aot` branch, not in the mod yet. Mono compiles about 28,000
  methods per boot, 2.7 s on the reference install (an 8.4 s reading was taken under heavy paging). KSP's own Mono
  contains an AOT compiler, `native/aotlink` turns its output into a loadable DLL without an external toolchain, and
  KSP code runs from such an image offline. Open: the net saving after verifying and loading the cache, and the
  gameplay cost of compiling without inlining (required so Harmony patches are not bypassed).
- **ModuleManager's re-patch** after a mod change (14.5 s of waiting here).
- **Part compilation** beyond the fixes above: mostly Unity object creation and module setup, which
  KSPCommunityFixes already speeds up.
- **Repeated, clean runs** of every number above.

Measured and ruled out: a larger initial GC heap (`GC_INITIAL_HEAP_SIZE=4G`) made no difference (61 s against
62 s). The multi-minute texture and planet-setup freezes seen earlier came from a missing dependency (KSPBurst,
needed by Parallax), not from KSP itself. The terrain quad pool is about 0.4 s, not the 5 s planet-setup stall it
first appeared to be.

Needs validation before a release:

- Other machines and installs, including a spinning disk and a machine with less memory.
- The GameData-only fallback (no `version.dll`), which has not run in game; and diagnostics inside FastBoot
  (the in-game runs used the separate BootProfiler).
- Linux and macOS: the C# side and kspcfg are written for them; no build has been run there.

## Building

Needs Rust (stable) and a KSP 1.12 install with HarmonyKSP, whose assemblies FastBoot compiles against. The
C# part targets C# 5 and builds with the .NET Framework's `csc`.

```powershell
powershell -ExecutionPolicy Bypass -File build.ps1 -Ksp "D:\Games\KSP"
```

Output goes to `dist/` (`GameData/FastBoot`, `EarlyStart/version.dll`, `tools/ksplaunch.exe`) and
`tests/out/FastBoot.Tests.exe`.

## Testing

```powershell
powershell -ExecutionPolicy Bypass -File test.ps1 -Ksp "D:\Games\KSP"
```

KSP is never started. The script runs:

- Rust unit tests;
- every `FastBoot.Tests` command on desktop .NET;
- the same commands inside KSP's own Mono through `tools/monohost`, which embeds `mono-2.0-bdwgc.dll` the
  way Unity does;
- the early-start chain end to end: `monohost --preload version.dll`, with the loader aimed at monohost
  instead of `UnityPlayer.dll`.

| Command | Checks |
| --- | --- |
| `diff` | kspcfg against stock on every cfg, plus fuzzing |
| `allowlist` | every fast-path code point against stock, in three cultures |
| `config-patch-*` | the Harmony patch with KSPCF first, FastBoot first, and without KSPCF |
| `diagnostics` | the boot profiler and sampler outside Unity, and that every method JIT-compiles |
| `early-chain`, `early-chain-off` | the native loader, the bootstrap and the deferred KSPCF guard; and that a disabled loader does nothing |

## Layout

```
native/kspcfg      Rust: the config text parser (cdylib, C ABI)
native/early       Rust: version.dll proxy, Mono start hook, warm-up start and read recording
native/warm        Rust: cold-start warm-up (plan, read list, parallel reader)
src/FastBoot       C#: the mod
tests/             C#: offline test runner
tools/ksplaunch    Rust: measuring launcher
tools/monohost     Rust: runs .NET programs inside KSP's Mono
GameData/FastBoot  shipped settings and KSP-AVC version file
```

## License

MIT. See [LICENSE](LICENSE).
