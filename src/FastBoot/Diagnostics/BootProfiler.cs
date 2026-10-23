using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using UnityEngine;
using Debug = UnityEngine.Debug;

namespace FastBoot.Diagnostics
{
    internal struct LogEvent
    {
        public double T;      // seconds since the profiler clock started
        public bool Main;     // emitted on the main thread
        public string Text;   // first line of the message, truncated
    }

    internal struct Mark
    {
        public double T;
        public string Text;
    }

    /// Measures where desktop-to-main-menu time goes, without patching anything: it timestamps every log line as
    /// Unity emits it, watches frames and LoadingScreen phases, and runs a watchdog thread that rewrites the report
    /// every few seconds, so a run killed mid-load still leaves data. Optionally stack-samples the main thread
    /// during freezes (Sampler). Started by FastBootAddon when Diagnostics is enabled.
    ///
    /// Output: PluginData/Diagnostics/boot-<stamp>.md (summary), .tsv (every log line), -stacks.tsv (samples).
    internal sealed class BootProfiler : MonoBehaviour
    {
        /// Set by FastBootAddon before the component is added.
        internal static string OutputDirectory;
        internal static bool EnableSampler;

        private const double HitchSeconds = 0.25;
        private const double StallSeconds = 2.0;
        private const int FlushIntervalMs = 3000;

        private static readonly Stopwatch Clock = Stopwatch.StartNew();
        private static readonly object Gate = new object();
        private static readonly List<LogEvent> Logs = new List<LogEvent>(80000);
        private static readonly List<Mark> Marks = new List<Mark>(256);
        private static readonly List<Mark> Hitches = new List<Mark>(256);
        private static readonly List<StackSample> Samples = new List<StackSample>(1024);
        private const double SampleAfterStallSeconds = 1.0;
        private const double SampleIntervalSeconds = 0.25;
        private static Sampler sampler;

        private static int mainThreadId;
        private static volatile int heartbeat;
        private static volatile bool finished;
        private static string lastMainLog = "";
        private static double processAgeAtStart;
        private static int gcAtStart;
        private static string outDir;
        private static string stamp;

        // Unity APIs are main-thread only; calling them from the watchdog crashes UnityPlayer natively.
        // These are captured on the main thread and only read by the report builder.
        private static string machineInfo = "";
        private static volatile string memoryInfo = "";

        private double lastFrame;
        private double lastMemorySample;
        private string lastLoader = "";
        private string lastTitle = "";
        private Thread watchdog;

        private void Awake()
        {
            DontDestroyOnLoad(gameObject);
            mainThreadId = Thread.CurrentThread.ManagedThreadId;
            stamp = DateTime.Now.ToString("yyyyMMdd-HHmmss");
            outDir = OutputDirectory;
            Directory.CreateDirectory(outDir);
            try
            {
                // Clock started at static initialisation; subtract its reading taken at the same moment.
                processAgeAtStart = (DateTime.Now - Process.GetCurrentProcess().StartTime).TotalSeconds - Clock.Elapsed.TotalSeconds;
            }
            catch (Exception)
            {
                processAgeAtStart = -1;
            }
            gcAtStart = GC.CollectionCount(0);
            try
            {
                machineInfo = string.Format(CultureInfo.InvariantCulture, "{0} ({1} threads), {2} MB RAM, {3} ({4} MB), {5}; incremental GC active: {6}",
                    SystemInfo.processorType, SystemInfo.processorCount, SystemInfo.systemMemorySize,
                    SystemInfo.graphicsDeviceName, SystemInfo.graphicsMemorySize, SystemInfo.graphicsDeviceType,
                    UnityEngine.Scripting.GarbageCollector.isIncremental);
            }
            catch (Exception)
            {
            }
            SampleMemory();
            if (EnableSampler)
            {
                sampler = new Sampler();
                AddMark(sampler.Ready ? "stall sampler ready" : "stall sampler unavailable");
            }
            Application.logMessageReceivedThreaded += OnLog;
            AddMark("diagnostics started (process age " + processAgeAtStart.ToString("F1", CultureInfo.InvariantCulture) + "s)");
            lastFrame = Clock.Elapsed.TotalSeconds;
            watchdog = new Thread(WatchdogLoop) { IsBackground = true, Name = "FastBoot diagnostics watchdog" };
            watchdog.Start();
        }

        private static void OnLog(string condition, string stackTrace, LogType type)
        {
            if (finished || condition == null)
                return;
            int nl = condition.IndexOf('\n');
            string first = nl >= 0 ? condition.Substring(0, nl) : condition;
            if (first.Length > 200)
                first = first.Substring(0, 200);
            bool main = Thread.CurrentThread.ManagedThreadId == mainThreadId;
            var e = new LogEvent { T = Clock.Elapsed.TotalSeconds, Main = main, Text = first };
            lock (Gate)
            {
                Logs.Add(e);
                if (main)
                    lastMainLog = first;
            }
        }

        private static void AddMark(string text)
        {
            lock (Gate)
                Marks.Add(new Mark { T = Clock.Elapsed.TotalSeconds, Text = text });
        }

        private void Update()
        {
            heartbeat++;
            double now = Clock.Elapsed.TotalSeconds;
            double dt = now - lastFrame;
            lastFrame = now;
            if (dt > HitchSeconds)
            {
                lock (Gate)
                    Hitches.Add(new Mark { T = now - dt, Text = dt.ToString("F2", CultureInfo.InvariantCulture) + "s frame after: " + lastMainLog });
            }
            if (finished)
                return;

            if (now - lastMemorySample > 1.0)
            {
                lastMemorySample = now;
                SampleMemory();
            }
            TrackLoadingScreen();

            if (HighLogic.LoadedScene == GameScenes.MAINMENU)
                Finish("main menu reached");
        }

        // Main thread only.
        private static void SampleMemory()
        {
            try
            {
                memoryInfo = string.Format(CultureInfo.InvariantCulture,
                    "Mono heap {0} MB used of {1} MB; Unity total allocated {2} MB; process private bytes {3} MB",
                    UnityEngine.Profiling.Profiler.GetMonoUsedSizeLong() >> 20,
                    UnityEngine.Profiling.Profiler.GetMonoHeapSizeLong() >> 20,
                    UnityEngine.Profiling.Profiler.GetTotalAllocatedMemoryLong() >> 20,
                    Process.GetCurrentProcess().PrivateMemorySize64 >> 20);
            }
            catch (Exception)
            {
            }
        }

        private void TrackLoadingScreen()
        {
            LoadingScreen screen = LoadingScreen.Instance;
            if (screen == null || screen.loaders == null)
                return;
            string loader = "done";
            string title = "";
            for (int i = 0; i < screen.loaders.Count; i++)
            {
                LoadingSystem l = screen.loaders[i];
                if (l == null || l.IsReady())
                    continue;
                loader = i + ": " + l.GetType().Name;
                try
                {
                    title = StripCounters(l.ProgressTitle());
                }
                catch (Exception)
                {
                    title = "";
                }
                break;
            }
            if (loader != lastLoader)
            {
                AddMark("loader -> " + loader);
                lastLoader = loader;
            }
            if (title != lastTitle && !string.IsNullOrEmpty(title))
            {
                AddMark("  title -> " + title);
                lastTitle = title;
            }
        }

        // "Loading texture asset 123/3653 (buffer=12MB)" -> "Loading texture asset"
        private static readonly Regex Counters = new Regex(@"[\d/().=%]+(MB)?\)?", RegexOptions.Compiled);

        private static string StripCounters(string s)
        {
            if (string.IsNullOrEmpty(s))
                return "";
            return Counters.Replace(s, "").Trim();
        }

        private void OnApplicationQuit()
        {
            Finish("quit before main menu");
        }

        private void WatchdogLoop()
        {
            int seen = heartbeat;
            double seenAt = Clock.Elapsed.TotalSeconds;
            double lastFlush = seenAt;
            bool stallReported = false;
            double lastSampleAt = 0;
            int gcAtLastSample = GC.CollectionCount(0);
            while (!finished)
            {
                Thread.Sleep(250);
                double now = Clock.Elapsed.TotalSeconds;
                int hb = heartbeat;
                if (hb != seen)
                {
                    seen = hb;
                    seenAt = now;
                    stallReported = false;
                }
                else if (!stallReported && now - seenAt > StallSeconds)
                {
                    string last;
                    lock (Gate)
                        last = lastMainLog;
                    AddMark("main thread has not completed a frame for " + StallSeconds + "s, last log: " + last);
                    stallReported = true;
                }
                if (sampler != null && hb == seen && now - seenAt >= SampleAfterStallSeconds && now - lastSampleAt >= SampleIntervalSeconds)
                {
                    lastSampleAt = now;
                    int gc = GC.CollectionCount(0);
                    StackSample sample;
                    if (sampler.TrySample(now, now - seenAt, gc - gcAtLastSample, out sample))
                    {
                        lock (Gate)
                            Samples.Add(sample);
                    }
                    gcAtLastSample = gc;
                }
                if ((now - lastFlush) * 1000 > FlushIntervalMs)
                {
                    lastFlush = now;
                    TryWrite("live, still loading");
                }
            }
        }

        private void Finish(string why)
        {
            if (finished)
                return;
            AddMark("finish: " + why);
            finished = true;
            Application.logMessageReceivedThreaded -= OnLog;
            TryWrite(why);
        }

        private static void TryWrite(string status)
        {
            try
            {
                List<LogEvent> logs;
                List<Mark> marks, hitches;
                List<StackSample> samples;
                lock (Gate)
                {
                    samples = new List<StackSample>(Samples);
                    logs = new List<LogEvent>(Logs);
                    marks = new List<Mark>(Marks);
                    hitches = new List<Mark>(Hitches);
                }
                string md = Report.Build(status, logs, marks, hitches, processAgeAtStart, gcAtStart, Clock.Elapsed.TotalSeconds,
                    machineInfo, memoryInfo, samples);
                WriteAtomic(Path.Combine(outDir, "boot-" + stamp + ".md"), md);
                var tsv = new StringBuilder(logs.Count * 64);
                tsv.Append("t_s\tthread\ttext\n");
                foreach (LogEvent e in logs)
                    tsv.Append(e.T.ToString("F4", CultureInfo.InvariantCulture)).Append('\t').Append(e.Main ? "main" : "other").Append('\t').Append(e.Text.Replace('\t', ' ')).Append('\n');
                WriteAtomic(Path.Combine(outDir, "boot-" + stamp + ".tsv"), tsv.ToString());
                var st = new StringBuilder(samples.Count * 256);
                st.Append("t_s\tstalled_s\tgc_since_last\tmodule\tmanaged_frames_innermost_first\n");
                foreach (StackSample s in samples)
                    st.Append(s.T.ToString("F2", CultureInfo.InvariantCulture)).Append('\t').Append(s.StalledFor.ToString("F2", CultureInfo.InvariantCulture))
                      .Append('\t').Append(s.GcSinceLastSample).Append('\t').Append(s.Module).Append('\t').Append(string.Join(" <- ", s.Managed)).Append('\n');
                WriteAtomic(Path.Combine(outDir, "boot-" + stamp + "-stacks.tsv"), st.ToString());
            }
            catch (Exception)
            {
                // Never let the profiler disturb the load.
            }
        }

        private static void WriteAtomic(string path, string content)
        {
            string tmp = path + ".tmp";
            File.WriteAllText(tmp, content, new UTF8Encoding(false));
            if (File.Exists(path))
                File.Delete(path);
            File.Move(tmp, path);
        }
    }
}
