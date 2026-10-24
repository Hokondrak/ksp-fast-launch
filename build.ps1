# Builds FastBoot into dist\ (cleared first):
#   dist\GameData\FastBoot\      the mod (FastBoot.dll, native kspcfg, settings, version file)
#   dist\EarlyStart\version.dll  optional early-start loader; goes next to KSP_x64.exe
#   dist\tools\ksplaunch.exe     the measuring launcher
#   tests\out\FastBoot.Tests.exe
#
# Usage: powershell -ExecutionPolicy Bypass -File build.ps1 -Ksp "D:\Games\KSP"   (or set KSP_DIR)
# Needs: Rust (cargo), and a KSP 1.12 install with Harmony (000_Harmony) to compile against.
param([string]$Ksp = $env:KSP_DIR)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $Ksp -or -not (Test-Path (Join-Path $Ksp "KSP_x64_Data\Managed\Assembly-CSharp.dll"))) {
    throw "Pass -Ksp <KSP folder> or set KSP_DIR (need KSP_x64_Data\Managed\Assembly-CSharp.dll)"
}
$managed = Join-Path $Ksp "KSP_x64_Data\Managed"
$harmony = Join-Path $Ksp "GameData\000_Harmony\0Harmony.dll"
if (-not (Test-Path $harmony)) { throw "Harmony not found at $harmony (install HarmonyKSP)" }
$csc = Join-Path $env:WINDIR "Microsoft.NET\Framework64\v4.0.30319\csc.exe"

function Invoke-Checked([string]$what, [scriptblock]$block) {
    & $block
    if ($LASTEXITCODE -ne 0) { throw "$what failed with exit code $LASTEXITCODE" }
}

# 1. Rust: kspcfg (native parser), the early-start loader, ksplaunch and monohost.
Push-Location $root
try { Invoke-Checked "cargo build" { cargo build --release --workspace } } finally { Pop-Location }

# 2. Mod layout. A fresh dist\ keeps test leftovers (early.log, disable-early) out of the package.
$dist = Join-Path $root "dist"
if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
$mod = Join-Path $dist "GameData\FastBoot"
$native = Join-Path $mod "PluginData\native\win-x64"
$early = Join-Path $dist "EarlyStart"
New-Item -ItemType Directory -Force (Join-Path $mod "Plugins"), $native, $early, (Join-Path $dist "tools") | Out-Null
Copy-Item (Join-Path $root "GameData\FastBoot\*") $mod -Recurse -Force
Copy-Item (Join-Path $root "LICENSE") (Join-Path $mod "LICENSE.txt") -Force
Copy-Item (Join-Path $root "target\release\kspcfg.dll") $native -Force
Copy-Item (Join-Path $root "target\release\version.dll") $early -Force
Copy-Item (Join-Path $root "target\release\ksplaunch.exe") (Join-Path $dist "tools") -Force

# 3. FastBoot.dll, against KSP's own Unity/Mono profile assemblies.
$refs = "mscorlib", "System", "System.Core", "UnityEngine", "UnityEngine.CoreModule", "UnityEngine.AudioModule", "Assembly-CSharp" |
    ForEach-Object { "-r:" + (Join-Path $managed "$_.dll") }
$sources = Get-ChildItem (Join-Path $root "src\FastBoot") -Recurse -Filter *.cs | ForEach-Object { $_.FullName }
Invoke-Checked "csc FastBoot.dll" {
    & $csc -nologo -nostdlib -noconfig -target:library -optimize+ -debug- -unsafe -langversion:5 -warnaserror+ `
        ("-out:" + (Join-Path $mod "Plugins\FastBoot.dll")) @refs ("-r:" + $harmony) @sources
}

# 4. Offline test runner (desktop .NET Framework).
$testOut = Join-Path $root "tests\out"
New-Item -ItemType Directory -Force $testOut | Out-Null
$testSources = Get-ChildItem (Join-Path $root "tests\FastBoot.Tests") -Filter *.cs | ForEach-Object { $_.FullName }
Invoke-Checked "csc FastBoot.Tests.exe" {
    & $csc -nologo -optimize+ -platform:x64 -langversion:5 ("-out:" + (Join-Path $testOut "FastBoot.Tests.exe")) `
        ("-r:" + (Join-Path $managed "Assembly-CSharp.dll")) ("-r:" + (Join-Path $managed "UnityEngine.CoreModule.dll")) ("-r:" + (Join-Path $managed "UnityEngine.AudioModule.dll")) `
        ("-r:" + (Join-Path $managed "UnityEngine.dll")) ("-r:" + $harmony) ("-r:" + (Join-Path $mod "Plugins\FastBoot.dll")) @testSources
}
Write-Output "built $mod"
