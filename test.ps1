# Runs every offline test: Rust unit tests, then each FastBoot.Tests command in its own process, first on
# desktop .NET and then inside KSP's own Mono runtime (tools/monohost), which is the one that matters in game.
# KSP is never started. Run build.ps1 first.
# Usage: powershell -ExecutionPolicy Bypass -File test.ps1 -Ksp "D:\Games\KSP" [-FuzzCases 200000] [-SkipMono]
param([string]$Ksp = $env:KSP_DIR, [int]$FuzzCases = 200000, [switch]$SkipMono)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $Ksp) { throw "Pass -Ksp <KSP folder> or set KSP_DIR" }
$runner = Join-Path $root "tests\out\FastBoot.Tests.exe"
$monohost = Join-Path $root "target\release\monohost.exe"
if (-not (Test-Path $runner)) { throw "Build first: build.ps1 -Ksp $Ksp" }
$dist = Join-Path $root "dist"

# <mirror>\GameData with a junction per GameData folder of $ksp except $exclude, plus copies of its loose .cfg
# files. Rebuilt every run. Junctions are removed with rmdir, which deletes the link and never what it points to.
function New-GameDataMirror([string]$ksp, [string]$mirror, [string[]]$exclude) {
    $gd = Join-Path $mirror "GameData"
    if (Test-Path $gd) {
        Get-ChildItem $gd -Directory | ForEach-Object { cmd /c rmdir "$($_.FullName)" }
        Get-ChildItem $gd -File | Remove-Item
        if (Get-ChildItem $gd) { throw "could not empty $gd" }
    }
    New-Item -ItemType Directory -Force $gd | Out-Null
    foreach ($d in Get-ChildItem (Join-Path $ksp "GameData") -Directory) {
        if ($exclude -notcontains $d.Name) {
            New-Item -ItemType Junction -Path (Join-Path $gd $d.Name) -Target $d.FullName | Out-Null
        }
    }
    Get-ChildItem (Join-Path $ksp "GameData") -File -Filter *.cfg | Copy-Item -Destination $gd
}

$results = [ordered]@{}
Push-Location $root
try {
    cargo test --release --workspace
    $results["rust unit tests"] = $LASTEXITCODE -eq 0
} finally { Pop-Location }

# diagnostics exercises Unity-specific paths and the stall sampler, so it only runs on desktop .NET.
$commands = "diff", "allowlist", "config-patch-kspcf-first", "config-patch-fastboot-first", "config-patch-standalone", "diagnostics"
foreach ($c in $commands) {
    Write-Output "== $c (desktop .NET)"
    & $runner $c --ksp $Ksp --dist $dist --cases $FuzzCases
    $results["$c (desktop .NET)"] = $LASTEXITCODE -eq 0
}

if (-not $SkipMono) {
    # Mono is several times slower, so it fuzzes less; the real-file and code-point checks are exhaustive.
    $monoCases = [Math]::Min($FuzzCases, 20000)
    foreach ($c in ($commands | Where-Object { $_ -ne "diagnostics" })) {
        Write-Output "== $c (KSP's Mono)"
        & $monohost --ksp $Ksp -- $runner $c --ksp $Ksp --dist $dist --cases $monoCases
        $results["$c (KSP's Mono)"] = $LASTEXITCODE -eq 0
    }

    # The early-start loader, end to end: monohost stands in for UnityPlayer.dll (it resolves Mono through
    # GetProcAddress the same way), with the built version.dll preloaded and pointed at it. The KSP folder it
    # sees is a mirror whose GameData links every mod except FastBoot's own predecessors (KspFastCfg,
    # BootProfiler), which FastBoot rightly steps aside for; the install itself is never modified.
    $mirror = Join-Path $env:TEMP "fastboot-test-ksp"
    New-GameDataMirror $Ksp $mirror @("FastBoot", "KspFastCfg", "BootProfiler")
    $loader = Join-Path $dist "EarlyStart\version.dll"
    $env:FASTBOOT_EARLY_TARGET = "monohost.exe"
    $env:FASTBOOT_EARLY_ROOT = $mirror
    $env:FASTBOOT_EARLY_ASSEMBLY = Join-Path $dist "GameData\FastBoot\Plugins\FastBoot.dll"
    try {
        foreach ($c in "early-chain", "early-chain-off") {
            $env:FASTBOOT_EARLY_DISABLE = if ($c -eq "early-chain-off") { "1" } else { "" }
            Write-Output "== $c (KSP's Mono, loader preloaded)"
            & $monohost --ksp $Ksp --preload $loader -- $runner $c --ksp $Ksp --dist $dist --gamedata (Join-Path $mirror "GameData")
            $results["$c (KSP's Mono)"] = $LASTEXITCODE -eq 0
        }
    } finally {
        "FASTBOOT_EARLY_TARGET", "FASTBOOT_EARLY_ROOT", "FASTBOOT_EARLY_ASSEMBLY", "FASTBOOT_EARLY_DISABLE" |
            ForEach-Object { Remove-Item "env:$_" -ErrorAction SilentlyContinue }
    }
}

Write-Output ""
$failed = 0
foreach ($k in $results.Keys) {
    $state = if ($results[$k]) { "PASS" } else { $failed++; "FAIL" }
    Write-Output ("{0}  {1}" -f $state, $k)
}
exit $failed
