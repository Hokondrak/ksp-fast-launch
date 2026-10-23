# Runs every offline test: Rust unit tests, then each FastBoot.Tests command in its own process.
# KSP is never started. Run build.ps1 first.
# Usage: powershell -ExecutionPolicy Bypass -File test.ps1 -Ksp "D:\Games\KSP" [-FuzzCases 200000]
param([string]$Ksp = $env:KSP_DIR, [int]$FuzzCases = 200000)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $Ksp) { throw "Pass -Ksp <KSP folder> or set KSP_DIR" }
$runner = Join-Path $root "tests\out\FastBoot.Tests.exe"
if (-not (Test-Path $runner)) { throw "Build first: build.ps1 -Ksp $Ksp" }
$dist = Join-Path $root "dist"

$results = [ordered]@{}
Push-Location $root
try {
    cargo test --release --workspace
    $results["rust unit tests"] = $LASTEXITCODE -eq 0
} finally { Pop-Location }

$commands = "diff", "allowlist", "config-patch-kspcf-first", "config-patch-fastboot-first", "config-patch-standalone", "diagnostics"
foreach ($c in $commands) {
    Write-Output "== $c"
    & $runner $c --ksp $Ksp --dist $dist --cases $FuzzCases
    $results[$c] = $LASTEXITCODE -eq 0
}

Write-Output ""
$failed = 0
foreach ($k in $results.Keys) {
    $state = if ($results[$k]) { "PASS" } else { $failed++; "FAIL" }
    Write-Output ("{0}  {1}" -f $state, $k)
}
exit $failed
