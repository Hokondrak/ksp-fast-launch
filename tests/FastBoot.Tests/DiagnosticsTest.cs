using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Text.RegularExpressions;
using System.Threading;
using FastBoot.Diagnostics;

namespace FastBoot.Tests
{
    /// 1. JIT-compiles every FastBoot method. Desktop .NET refuses to JIT methods that call Unity's native (ECall)
    ///    bindings, which is exactly the set that may only run on Unity's main thread, so any failure outside the
    ///    list below is a bug: the diagnostics watchdog thread could reach it and crash UnityPlayer.
    /// 2. Replays a real KSP.log through the report builder.
    /// 3. Exercises the stall sampler's suspend/copy/resume cycle against a busy thread.
    internal static class DiagnosticsTest
    {
        private static readonly HashSet<string> MainThreadOnly = new HashSet<string>
        {
            "FastBootAddon.Awake", "FastBootAddon.StartDiagnostics", "FastBootAddon.StartProfiler",
            "PhaseProfiler.Awake", "PhaseProfiler.OnDestroy", "BootFinishedAddon.Start",
            "BootProfiler.Awake", "BootProfiler.SampleMemory", "BootProfiler.Finish",
            "Sampler.mono_pmip", "Sampler.mono_free", // Unity's Mono exports; they exist only inside KSP
        };

        public static int Run(Options o)
        {
            int failures = JitCheck();
            failures += Replay(Path.Combine(o.Ksp, "KSP.log"));
            failures += SamplerMechanics();
            failures += ProfileSummary();
            return failures == 0 ? 0 : 1;
        }

        /// The profile report from synthetic samples: shares, frame-name cleaning, inclusive counting once per sample.
        private static int ProfileSummary()
        {
            Func<string, string[], StackSample> s = (module, frames) => new StackSample { Module = module, Managed = frames };
            var samples = new List<StackSample>
            {
                s("UnityPlayer.dll", new[] { "PartLoader:CompileModel () {0000AAAA}", "PartLoader:ParsePart () {0000BBBB}", "PartLoader:ParsePart () {0000CCCC}" }),
                s("UnityPlayer.dll", new[] { "PartLoader:CompileModel () {0000AAAA}", "PartLoader:ParsePart () {0000BBBB}" }),
                s("mono-2.0-bdwgc.dll", new[] { "PartModule:Load (ConfigNode) {0000DDDD}", "PartLoader:ParsePart () {0000BBBB}" }),
                s("ntdll.dll", new string[0]),
            };
            string report = PhaseProfiler.Summarize(samples, 0.008, PhaseProfiler.Find("parts"), null);
            var expect = new[]
            {
                "-  50.0 %       2  PartLoader:CompileModel ()",   // self
                "-  75.0 %       3  PartLoader:ParsePart ()",      // inclusive, counted once although it appears twice
                "-  25.0 %       1  (no managed frame)",
                "-  50.0 %       2  UnityPlayer.dll",
            };
            int failed = expect.Count(e => !report.Contains(e));
            if (failed > 0 || report.Contains("{0000"))
            {
                Console.WriteLine("PROFILE REPORT WRONG:");
                Console.WriteLine(report);
                return 1;
            }
            Console.WriteLine("profile report: shares, cleaning and inclusive counting correct");
            return 0;
        }

        private static int JitCheck()
        {
            Assembly fastBoot = typeof(Log).Assembly;
            int ok = 0, failed = 0;
            foreach (Type t in fastBoot.GetTypes())
            {
                const BindingFlags all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
                foreach (MethodBase m in t.GetMethods(all).Cast<MethodBase>().Concat(t.GetConstructors(all)))
                {
                    if (m.IsAbstract || m.ContainsGenericParameters || (m.Attributes & MethodAttributes.PinvokeImpl) != 0)
                        continue;
                    if (t.IsSubclassOf(typeof(Delegate)))
                        continue;
                    try
                    {
                        RuntimeHelpers.PrepareMethod(m.MethodHandle);
                        ok++;
                    }
                    catch (Exception ex)
                    {
                        string name = t.Name + "." + m.Name;
                        if (MainThreadOnly.Contains(name))
                            continue;
                        failed++;
                        Console.WriteLine("JIT FAIL " + name + ": " + ex.Message);
                    }
                }
            }
            Console.WriteLine("JIT check: {0} methods compiled, {1} unexpected failures", ok, failed);
            return failed;
        }

        private static int Replay(string kspLog)
        {
            if (!File.Exists(kspLog))
            {
                Console.WriteLine("replay skipped: no KSP.log at " + kspLog);
                return 0;
            }
            var rx = new Regex(@"^\[(?:LOG|WRN|ERR|EXC) (\d\d:\d\d:\d\d\.\d+)\] (.*)$");
            var logs = new List<LogEvent>();
            TimeSpan? t0 = null;
            foreach (string line in File.ReadLines(kspLog))
            {
                Match m = rx.Match(line);
                if (!m.Success) continue;
                TimeSpan ts = TimeSpan.ParseExact(m.Groups[1].Value, @"hh\:mm\:ss\.fff", CultureInfo.InvariantCulture);
                if (t0 == null) t0 = ts;
                string text = m.Groups[2].Value;
                logs.Add(new LogEvent { T = (ts - t0.Value).TotalSeconds, Main = true, Text = text.Length > 200 ? text.Substring(0, 200) : text });
            }
            if (logs.Count == 0)
            {
                Console.WriteLine("replay skipped: KSP.log has no timestamped lines");
                return 0;
            }
            string md = Report.Build("offline replay", logs, new List<Mark>(), new List<Mark>(), 0, 0, logs[logs.Count - 1].T,
                "offline", "offline", new List<StackSample>());
            bool ok = md.Contains("## Longest silences on the main thread") && md.Contains("Profiler clock:");
            Console.WriteLine("replay: {0} log lines -> {1} chars of report{2}", logs.Count, md.Length, ok ? "" : " MISSING SECTIONS");
            return ok ? 0 : 1;
        }

        private static int SamplerMechanics()
        {
            Sampler sampler = null;
            long spins = 0;
            bool stop = false;
            var ready = new ManualResetEvent(false);
            var busy = new Thread(() =>
            {
                sampler = new Sampler();
                ready.Set();
                while (!stop) Interlocked.Increment(ref spins);
            });
            busy.Start();
            ready.WaitOne();
            int failures = 0, taken = 0;
            for (int i = 0; i < 40; i++)
            {
                long before = Interlocked.Read(ref spins);
                StackSample s;
                bool ok = sampler.TrySample(i, 1.0, 0, out s);
                Thread.Sleep(5);
                if (!ok || Interlocked.Read(ref spins) <= before)
                {
                    Console.WriteLine("SAMPLER FAIL at sample " + i);
                    failures++;
                    break;
                }
                taken++;
            }
            stop = true;
            busy.Join();
            Console.WriteLine("sampler: ready {0}, {1} samples, busy thread kept running", sampler.Ready, taken);
            return failures;
        }
    }
}
