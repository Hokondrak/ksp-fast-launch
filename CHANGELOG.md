# Changelog

## Unreleased (0.1.0)

Not yet run in game; see "Not done yet" in the README.

- Config parser: GameData `.cfg` loads served from the native kspcfg parser with stock-identical results,
  pre-parsed on worker threads and served for as long as each file is unchanged, so both config passes use it.
- Works alongside KSPCommunityFixes in any load order, with no double parsing. When KSPCF loads after FastBoot,
  it is guarded as soon as its assembly loads.
- Optional early start (`version.dll`, Windows): installs the config parser before KSP's first config scan.
  It can be switched off by file, environment variable or deletion, and writes its own `early.log`.
- Diagnostics: per-launch boot timing report and an optional main-thread stall sampler (off by default).
- `ksplaunch`: measuring launcher with per-phase CPU, memory, disk and GPU telemetry.
- Rust binaries link the C runtime statically, so they need no Visual C++ redistributable.
