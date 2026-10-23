# Stand-in for KSP when testing ksplaunch: writes Boehm-format GC statistics to $env:GC_LOG_FILE at known
# times and a BootProfiler-style report with one freeze, then exits. Script clock s = process time - age:
#   freeze: s 1.0 .. 2.5 (logged "@1.0s" for "1.50s")
#   GC: "World-stopped marking took 1200 msecs" written at s 2.7, so it covered s 1.5 .. 2.7
#   expected overlap with the freeze: s 1.5 .. 2.5 = 1.0 s
param([string]$ProfilerDir)
$sw = [Diagnostics.Stopwatch]::StartNew()
$age = ((Get-Date) - (Get-Process -Id $PID).StartTime).TotalSeconds - $sw.Elapsed.TotalSeconds  # like BootProfiler: age when the clock started
function Wait-Until([double]$t) { while ($sw.Elapsed.TotalSeconds -lt $t) { Start-Sleep -Milliseconds 10 } }
function Gc-Line([string]$s) { [IO.File]::AppendAllText($env:GC_LOG_FILE, $s + "`n") }
$hog = New-Object byte[] (64MB)  # some memory for the telemetry to see; allocated up front so it cannot delay the timeline

Wait-Until 0.6
Gc-Line "GC #1 freed 1000 bytes, heap 65536 KiB (+ 0 KiB unmapped)"
Gc-Line "World-stopped marking took 40 msecs (40 in average)"
Wait-Until 2.7
Gc-Line "Grow heap to 1048576 KiB after 123456 bytes allocated"
Gc-Line "World-stopped marking took 1200 msecs (620 in average)"
Gc-Line "Complete collection took 1250 msecs"
Wait-Until 3.5
$md = @"
# KSP boot profile

Status: **main menu reached**
Profiler clock: 3.0s since BootProfiler loaded; process was $($age.ToString('F3', [Globalization.CultureInfo]::InvariantCulture))s old at that point, so desktop-to-now is about 3.5s.

## Longest silences on the main thread (worst 30)

- **1.50s** @1.0s after: Load DDS texture: Stub/texture-a  $([char]0x2192) next: Load DDS texture: Stub/texture-b
"@
[IO.File]::WriteAllText((Join-Path $ProfilerDir "boot-stub.md"), $md, (New-Object Text.UTF8Encoding $false))
Wait-Until 4.0
