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

namespace FastBoot.Diagnostics
{
    /// Builds the Markdown report. Runs on the watchdog thread, so it must not touch any Unity API.
    internal static class Report
    {
        private static readonly CultureInfo Inv = CultureInfo.InvariantCulture;
        private static readonly Regex PartLine = new Regex(@"^PartLoader: Compiling Part '([^']+)'", RegexOptions.Compiled);
        private static readonly Regex AssetLine = new Regex(@"^Load (\w+(?: \w+)?(?: [Tt]exture| model)?): (.+)$", RegexOptions.Compiled);
        private static readonly Regex AddonLine = new Regex(@"^\[AddonLoader\]: Instantiating addon '([^']+)' from assembly '([^']+)'", RegexOptions.Compiled);
        private static readonly Regex InternalLine = new Regex(@"^PartLoader: Compiling (Internal Prop|Internal Space) '([^']+)'", RegexOptions.Compiled);

        // Runs on the watchdog thread: must not call any Unity API.
        public static string Build(string status, List<LogEvent> logs, List<Mark> marks, List<Mark> hitches,
            double processAgeAtStart, int gcAtStart, double now, string machineInfo, string memoryInfo, List<StackSample> samples)
        {
            var sb = new StringBuilder();
            sb.Append("# KSP boot profile (FastBoot diagnostics)\n\n");
            sb.AppendFormat(Inv, "Status: **{0}**  \n", status);
            sb.AppendFormat(Inv, "Written: {0:yyyy-MM-dd HH:mm:ss}  \n", DateTime.Now);
            sb.AppendFormat(Inv, "Profiler clock: {0:F1}s since BootProfiler loaded; process was {1:F1}s old at that point, so desktop-to-now is about {2:F1}s.  \n",
                now, processAgeAtStart, now + Math.Max(0, processAgeAtStart));
            sb.AppendFormat(Inv, "GC collections (gen0) during profile: {0}  \n", GC.CollectionCount(0) - gcAtStart);
            sb.AppendFormat(Inv, "Machine: {0}  \n", machineInfo);
            sb.AppendFormat(Inv, "Memory (last main-thread sample): {0}  \n", memoryInfo);
            int mainCount = logs.Count(l => l.Main);
            sb.AppendFormat(Inv, "Log lines: {0} ({1} on the main thread)\n\n", logs.Count, mainCount);

            AppendPhases(sb, marks, now);
            AppendKspcf(sb);
            AppendGaps(sb, logs);
            AppendSamples(sb, samples, marks);
            AppendGrouped(sb, logs, PartLine, "Slowest part compilations", 25, m => m.Groups[1].Value, false);
            AppendGrouped(sb, logs, InternalLine, "Slowest internal spaces / props", 15, m => m.Groups[1].Value + " " + m.Groups[2].Value, false);
            AppendGrouped(sb, logs, AddonLine, "Slowest addon instantiations (constructor + Awake, until the next log line)", 20, m => m.Groups[2].Value + " / " + m.Groups[1].Value, true);
            AppendAssetRates(sb, logs);
            AppendMarks(sb, "Frame hitches over 0.25 s (worst 25)", hitches.OrderByDescending(h => ParseLead(h.Text)).Take(25).ToList());
            AppendMarks(sb, "Phase and watchdog marks", marks);
            return sb.ToString();
        }

        // Samples are taken only while the main thread is stalled (no frame for 1 s+), every 0.25 s.
        private static void AppendSamples(StringBuilder sb, List<StackSample> samples, List<Mark> marks)
        {
            if (samples.Count == 0)
                return;
            sb.AppendFormat(Inv, "## What the main thread was doing during stalls ({0} samples, 0.25 s apart)\n\n", samples.Count);
            sb.Append("Each sample is the main thread's instruction pointer module and managed frames found on its stack. ");
            sb.Append("Stack scanning is conservative, so deeper frames can be stale; trust the counts, not single samples.\n\n");
            AppendCounts(sb, "Instruction pointer module", samples.GroupBy(s => s.Module), samples.Count, 10);
            AppendCounts(sb, "Innermost managed frame", samples.GroupBy(s => s.Managed.Length > 0 ? s.Managed[0] : "(none)"), samples.Count, 15);
            AppendCounts(sb, "Innermost three managed frames", samples.GroupBy(s => string.Join(" <- ", s.Managed.Take(3).ToArray())), samples.Count, 15);
            sb.AppendFormat(Inv, "Samples with a GC since the previous sample: {0} of {1}.\n\n", samples.Count(s => s.GcSinceLastSample > 0), samples.Count);
            // Per phase: attribute each sample to the LoadingScreen phase it fell in.
            var phases = marks.Where(m => m.Text.StartsWith("loader -> ")).ToList();
            if (phases.Count > 0)
            {
                sb.Append("By phase (top innermost frame):\n\n");
                foreach (var g in samples.GroupBy(s => PhaseAt(phases, s.T)))
                {
                    var top = g.GroupBy(s => s.Managed.Length > 0 ? s.Managed[0] : s.Module).OrderByDescending(x => x.Count()).First();
                    sb.AppendFormat(Inv, "- {0}: {1} samples (~{2:F0} s stalled); most often {3} ({4})\n", g.Key, g.Count(), g.Count() * 0.25, Trim(top.Key, 120), top.Count());
                }
                sb.Append('\n');
            }
        }

        private static string PhaseAt(List<Mark> phases, double t)
        {
            string p = "before loading screen";
            foreach (Mark m in phases)
            {
                if (m.T <= t)
                    p = m.Text.Substring(10);
                else
                    break;
            }
            return p;
        }

        private static void AppendCounts(StringBuilder sb, string title, IEnumerable<IGrouping<string, StackSample>> groups, int total, int top)
        {
            sb.AppendFormat(Inv, "**{0}**\n\n", title);
            foreach (var g in groups.OrderByDescending(g => g.Count()).Take(top))
                sb.AppendFormat(Inv, "- {0,4} ({1:P0})  {2}\n", g.Count(), (double)g.Count() / total, Trim(g.Key, 200));
            sb.Append('\n');
        }

        private static double ParseLead(string s)
        {
            int i = s.IndexOf('s');
            double v;
            return i > 0 && double.TryParse(s.Substring(0, i), NumberStyles.Float, CultureInfo.InvariantCulture, out v) ? v : 0;
        }

        private static void AppendPhases(StringBuilder sb, List<Mark> marks, double now)
        {
            var loaders = marks.Where(m => m.Text.StartsWith("loader -> ")).ToList();
            if (loaders.Count == 0)
                return;
            sb.Append("## LoadingScreen phases\n\n| Phase | Start (s) | Duration (s) |\n|---|---:|---:|\n");
            for (int i = 0; i < loaders.Count; i++)
            {
                double end = i + 1 < loaders.Count ? loaders[i + 1].T : now;
                sb.AppendFormat(Inv, "| {0} | {1:F1} | {2:F1} |\n", loaders[i].Text.Substring(10), loaders[i].T, end - loaders[i].T);
            }
            sb.Append('\n');
        }

        private static void AppendKspcf(StringBuilder sb)
        {
            Type report = null;
            foreach (Assembly a in AppDomain.CurrentDomain.GetAssemblies())
            {
                report = a.GetType("KSPCommunityFixes.Performance.KSPCFFastLoaderReport", false);
                if (report != null)
                    break;
            }
            if (report == null)
                return;
            sb.Append("## KSPCF FastLoader stopwatches (valid even if the run was quit)\n\n| Stopwatch | Seconds |\n|---|---:|\n");
            foreach (FieldInfo f in report.GetFields(BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic))
            {
                var sw = f.GetValue(null) as Stopwatch;
                if (sw != null)
                    sb.AppendFormat(Inv, "| {0} | {1:F2} |\n", f.Name, sw.Elapsed.TotalSeconds);
            }
            sb.Append('\n');
        }

        // Time between consecutive main-thread log lines is attributed to the earlier line:
        // it is the work done after that message and before the next one.
        private static void AppendGaps(StringBuilder sb, List<LogEvent> logs)
        {
            var main = logs.Where(l => l.Main).ToList();
            var gaps = new List<KeyValuePair<double, string>>();
            for (int i = 0; i + 1 < main.Count; i++)
                gaps.Add(new KeyValuePair<double, string>(main[i + 1].T - main[i].T,
                    string.Format(Inv, "@{0:F1}s after: {1}  → next: {2}", main[i].T, Trim(main[i].Text, 110), Trim(main[i + 1].Text, 70))));
            sb.Append("## Longest silences on the main thread (worst 30)\n\n");
            foreach (var g in gaps.OrderByDescending(g => g.Key).Take(30))
                sb.AppendFormat(Inv, "- **{0:F2}s** {1}\n", g.Key, g.Value);
            sb.Append('\n');
        }

        private static void AppendGrouped(StringBuilder sb, List<LogEvent> logs, Regex rx, string title, int top, Func<Match, string> key, bool untilAnyLine)
        {
            var main = logs.Where(l => l.Main).ToList();
            var cost = new List<KeyValuePair<double, string>>();
            double total = 0;
            for (int i = 0; i + 1 < main.Count; i++)
            {
                Match m = rx.Match(main[i].Text);
                if (!m.Success)
                    continue;
                // Cost runs until the next line of the same kind, or 5 s at most to avoid swallowing a phase change.
                int j = i + 1;
                while (!untilAnyLine && j < main.Count && !rx.IsMatch(main[j].Text) && main[j].T - main[i].T < 5)
                    j++;
                double end = j < main.Count ? main[j].T : main[main.Count - 1].T;
                double c = Math.Min(end - main[i].T, 5);
                total += c;
                cost.Add(new KeyValuePair<double, string>(c, key(m)));
            }
            if (cost.Count == 0)
                return;
            sb.AppendFormat(Inv, "## {0}\n\n{1} items, {2:F1}s total, {3:F1} ms mean.\n\n", title, cost.Count, total, total * 1000 / cost.Count);
            foreach (var c in cost.OrderByDescending(c => c.Key).Take(top))
                sb.AppendFormat(Inv, "- {0:F0} ms  {1}\n", c.Key * 1000, c.Value);
            sb.Append('\n');
        }

        private static void AppendAssetRates(StringBuilder sb, List<LogEvent> logs)
        {
            var byKind = new Dictionary<string, List<double>>();
            foreach (LogEvent e in logs)
            {
                if (!e.Main)
                    continue;
                Match m = AssetLine.Match(e.Text);
                if (!m.Success)
                    continue;
                List<double> ts;
                if (!byKind.TryGetValue(m.Groups[1].Value, out ts))
                    byKind[m.Groups[1].Value] = ts = new List<double>();
                ts.Add(e.T);
            }
            if (byKind.Count == 0)
                return;
            sb.Append("## Asset log lines by kind (first to last line)\n\n| Kind | Count | First (s) | Last (s) | Span (s) |\n|---|---:|---:|---:|---:|\n");
            foreach (var kv in byKind.OrderBy(k => k.Value[0]))
                sb.AppendFormat(Inv, "| {0} | {1} | {2:F1} | {3:F1} | {4:F1} |\n", kv.Key, kv.Value.Count, kv.Value[0], kv.Value[kv.Value.Count - 1], kv.Value[kv.Value.Count - 1] - kv.Value[0]);
            sb.Append('\n');
        }

        private static void AppendMarks(StringBuilder sb, string title, List<Mark> marks)
        {
            if (marks.Count == 0)
                return;
            sb.AppendFormat(Inv, "## {0}\n\n", title);
            foreach (Mark m in marks)
                sb.AppendFormat(Inv, "- {0,7:F1}s  {1}\n", m.T, m.Text);
            sb.Append('\n');
        }

        private static string Trim(string s, int n)
        {
            return s.Length <= n ? s : s.Substring(0, n) + "…";
        }
    }
}
