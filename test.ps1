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
}

Write-Output ""
$failed = 0
foreach ($k in $results.Keys) {
    $state = if ($results[$k]) { "PASS" } else { $failed++; "FAIL" }
    Write-Output ("{0}  {1}" -f $state, $k)
}
exit $failed
