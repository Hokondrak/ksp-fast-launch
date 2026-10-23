# FastBoot

A Kerbal Space Program 1.12 mod that shortens the time from desktop to main menu on heavily modded installs,
without changing what the game loads.

> **Status: pre-release, not yet validated in game.** Everything below passes offline tests against a large real
> install (the Jool Odyssey modpack), including inside KSP's own Mono runtime, but FastBoot has not yet been run inside the game.
> Do not install it on a save you care about yet.

## What it does

| Part | What | Where |
| --- | --- | --- |
| Config parser | Serves `ConfigNode.Load` for GameData `.cfg` files from a native Rust parser (`kspcfg`), pre-parsing every file on worker threads. Tree building stays KSP's own, so the result is identical to stock; files outside the parser's verified fast path go to stock (or KSPCommunityFixes) unchanged. | `GameData/FastBoot` |
| Early start (optional) | A `version.dll` placed next to `KSP_x64.exe` that starts the config parser *before* KSP's first config scan, which runs before any GameData plugin exists. Without it, only the second scan benefits. | `EarlyStart/version.dll` |
| Diagnostics (off by default) | A boot timing report per launch, and optionally a stall sampler that records what the main thread is doing during freezes. | `GameData/FastBoot/PluginData/Diagnostics` |
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
  the corresponding feature to them.
- **ModuleManager**: unaffected. `ModuleManager.ConfigCache` loads to the same tree as before.

## Install (once released)

1. Copy `GameData/FastBoot` into KSP's `GameData`. Requires [HarmonyKSP](https://github.com/KSPModdingLibs/HarmonyKSP).
2. Optional, Windows only: copy `EarlyStart/version.dll` next to `KSP_x64.exe`.

To switch the early start off, delete `version.dll`, create an empty `GameData/FastBoot/PluginData/disable-early`,
or set `FASTBOOT_EARLY_DISABLE=1`. Each launch's early-start log is `GameData/FastBoot/PluginData/early.log`.

Settings are in `GameData/FastBoot/FastBoot.cfg`:

| Setting | Default | Meaning |
| --- | --- | --- |
| `ConfigParser` | `true` | Use the native config parser. |
| `ConfigParserThreads` | `0` | Pre-parse threads; 0 = CPU threads − 2, at most 8. |
| `Diagnostics` | `false` | Write a boot timing report per launch. |
| `StallSampler` | `false` | With Diagnostics: sample the main thread during freezes (Windows). |

## Not done yet

These depend on measurements from real launches and are deliberately out of scope until those exist:

- **Texture loading.** Asset bytes are read in about 4 s, but the texture phase takes minutes, and most of it
  is waiting. After roughly 2.3 GB have loaded, freezes of 51–180 s appear. Whether they come from garbage
  collection, VRAM or paging decides the fix, so `ksplaunch` and the stall sampler exist to find out first.
- **GC tuning defaults.** Unity's Mono honours `GC_INITIAL_HEAP_SIZE` and related variables, but no default
  ships until measured launches show it helps.
- **Planetary setup** (a 90–110 s freeze on the reference install): not yet analysed.
- **Speed claims** for whole launches.

Needs validation in game before a release:

- Every FastBoot feature inside the game (offline tests cover the logic, not Unity).
- That the early start's hook point behaves under Unity's player as it does under `monohost`. The player names
  its Mono domain "Unity Root Domain" and has no child-domain string, which suggests the game runs in the
  domain the loader sees. That is inferred from the binary, not observed.
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
native/early       Rust: version.dll proxy and Mono start hook
src/FastBoot       C#: the mod
tests/             C#: offline test runner
tools/ksplaunch    Rust: measuring launcher
tools/monohost     Rust: runs .NET programs inside KSP's Mono
GameData/FastBoot  shipped settings and KSP-AVC version file
```

## License

MIT. See [LICENSE](LICENSE).
