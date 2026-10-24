// Continuous main-thread sampling through one stretch of the boot.
//
// The stall sampler only looks at the main thread when it has stopped completing frames, so it cannot see work
// made of many short steps, such as part compilation (about 1,500 parts at 5-10 ms each). This profiler samples
// the main thread every couple of milliseconds from a start log line to a stop log line, then writes which
// managed frames the time went to. It uses the same Sampler, so the same rules apply: nothing allocates while the
// main thread is suspended, and stack scanning is conservative, so read the counts, not single samples.

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading;
using UnityEngine;

namespace FastBoot.Diagnostics
{
    internal sealed class PhaseProfiler : MonoBehaviour
    {
        /// A stretch of the boot, delimited by the first log line starting with any of Start and the first after
        /// it starting with Stop.
        internal sealed class Window
        {
            public string Name;
            public string[] Start;
            public string Stop;
        }

        internal static readonly Window[] Windows =
        {
            // KSP loads the part database when it has one and creates it when ModuleManager deleted it.
            new Window
            {
                Name = "parts",
                Start = new[] { "PartLoader: Loading part database", "PartLoader: Creating part database" },
                Stop = "PartLoader: Compiling Internal Prop",
            },
            // The whole load: from the first log line after FastBoot's addon starts to the main menu.
            new Window
            {
                Name = "boot",
                Start = new[] { "" },
                Stop = "[HighLogic]: =========================== Scene Change : From LOADING to MAINMENU",
            },
        };

        internal static string OutputDirectory;
        internal static Window Selected;

        private const int IntervalMs = 2;
        // Never sample longer than this, whatever the log says.
        private const double MaxSeconds = 180;

        private Sampler sampler;
        private Thread thread;
        private volatile bool stopping;
        private readonly List<StackSample> samples = new List<StackSample>();
        private readonly Stopwatch clock = new Stopwatch();
        private bool running, done;

        /// The window called `name`, or null.
        internal static Window Find(string name)
        {
            return Windows.FirstOrDefault(w => string.Equals(w.Name, name, StringComparison.OrdinalIgnoreCase));
        }

        private void Awake()
        {
            sampler = new Sampler();
            if (!sampler.Ready || Selected == null)
            {
                Log.Warning("profiler unavailable" + (Selected == null ? " (no window selected)" : ""));
                Destroy(this);
                return;
            }
            Application.logMessageReceived += OnLog;
            Log.Info("profiler armed for '" + Selected.Name + "': from \"" + string.Join("\" or \"", Selected.Start) + "\" to \"" + Selected.Stop + "\"");
        }

        private void OnDestroy()
        {
            Application.logMessageReceived -= OnLog;
            End("profiler stopped early: the game left the loading screen first");
        }

        private void OnLog(string message, string stackTrace, LogType type)
        {
            if (done || message == null)
                return;
            if (!running && Selected.Start.Any(s => message.StartsWith(s, StringComparison.Ordinal)))
                Begin();
            else if (running && message.StartsWith(Selected.Stop, StringComparison.Ordinal))
                End(null);
        }

        private DateTime startedAt;

        private void Begin()
        {
            running = true;
            startedAt = DateTime.Now;
            clock.Start();
            thread = new Thread(SampleLoop) { IsBackground = true, Name = "FastBoot profiler", Priority = System.Threading.ThreadPriority.AboveNormal };
            thread.Start();
        }

        // Profiler thread: only Sampler calls, never Unity.
        private void SampleLoop()
        {
            while (!stopping && clock.Elapsed.TotalSeconds < MaxSeconds)
            {
                StackSample s;
                if (sampler.TrySample(clock.Elapsed.TotalSeconds, 0, 0, out s))
                    samples.Add(s);
                Thread.Sleep(IntervalMs);
            }
        }

        private void End(string note)
        {
            if (!running || done)
                return;
            done = true;
            stopping = true;
            thread.Join();
            clock.Stop();
            try
            {
                string stamp = DateTime.Now.ToString("yyyyMMdd-HHmmss", CultureInfo.InvariantCulture);
                Directory.CreateDirectory(OutputDirectory);
                string md = Path.Combine(OutputDirectory, Selected.Name + "-profile-" + stamp + ".md");
                string started = "Started " + startedAt.ToString("HH:mm:ss.fff", CultureInfo.InvariantCulture) + " (local time).";
                File.WriteAllText(md, started + Environment.NewLine + Environment.NewLine + Summarize(samples, clock.Elapsed.TotalSeconds, Selected, note));
                File.WriteAllLines(Path.ChangeExtension(md, ".tsv"),
                    new[] { "t_s\tmodule\tframes (innermost first)" }.Concat(samples.Select(s =>
                        s.T.ToString("F4", CultureInfo.InvariantCulture) + "\t" + s.Module + "\t" + string.Join(" < ", s.Managed.Select(Clean).ToArray()))).ToArray());
                Log.Info("profile of '" + Selected.Name + "' written to " + md);
            }
            catch (Exception ex)
            {
                Log.Warning("profile not written: " + ex.Message);
            }
        }

        /// A frame name without the code address Mono appends ("Type:Method (args) {0000...}").
        internal static string Clean(string frame)
        {
            int brace = frame.LastIndexOf(" {", StringComparison.Ordinal);
            return (brace > 0 ? frame.Substring(0, brace) : frame).Trim();
        }

        /// The report for `samples` taken over `seconds`. Pure, so the offline tests can check it.
        internal static string Summarize(IList<StackSample> samples, double seconds, Window window, string note)
        {
            var sb = new StringBuilder();
            var inv = CultureInfo.InvariantCulture;
            int n = samples.Count;
            sb.AppendLine("# FastBoot profile: " + window.Name);
            sb.AppendLine();
            sb.AppendLine(string.Format(inv, "From \"{0}\" to \"{1}\": {2:F2} s, {3} samples (one every {4:F1} ms).",
                string.Join("\" or \"", window.Start), window.Stop, seconds, n, n == 0 ? 0 : seconds * 1000 / n));
            if (note != null)
                sb.AppendLine().AppendLine("Note: " + note + ".");
            sb.AppendLine();
            sb.AppendLine("Shares are of samples. Inclusive counts a frame once per sample wherever it appears on the stack;");
            sb.AppendLine("stack scanning is conservative, so stale frames inflate inclusive shares slightly. Self is the innermost");
            sb.AppendLine("managed frame, which for native work (rendering, physics, file reads) is the managed caller.");
            if (n == 0)
                return sb.ToString();

            Section(sb, "Where the instruction pointer was (module)", samples.GroupBy(s => s.Module ?? "?").Select(g => Pair(g.Key, g.Count())), n, 12);
            Section(sb, "Self (innermost managed frame)",
                samples.GroupBy(s => s.Managed.Length > 0 ? Clean(s.Managed[0]) : "(no managed frame)").Select(g => Pair(g.Key, g.Count())), n, 40);
            var inclusive = new Dictionary<string, int>();
            foreach (StackSample s in samples)
                foreach (string f in s.Managed.Select(Clean).Distinct())
                    inclusive[f] = (inclusive.ContainsKey(f) ? inclusive[f] : 0) + 1;
            Section(sb, "Inclusive (anywhere on the stack)", inclusive.Select(kv => Pair(kv.Key, kv.Value)), n, 60);
            return sb.ToString();
        }

        private static KeyValuePair<string, int> Pair(string k, int v)
        {
            return new KeyValuePair<string, int>(k, v);
        }

        private static void Section(StringBuilder sb, string title, IEnumerable<KeyValuePair<string, int>> counts, int total, int top)
        {
            sb.AppendLine().AppendLine("## " + title).AppendLine();
            foreach (var kv in counts.OrderByDescending(kv => kv.Value).ThenBy(kv => kv.Key, StringComparer.Ordinal).Take(top))
                sb.AppendLine(string.Format(CultureInfo.InvariantCulture, "- {0,5:F1} %  {1,6}  {2}", 100.0 * kv.Value / total, kv.Value, kv.Key));
        }
    }
}
