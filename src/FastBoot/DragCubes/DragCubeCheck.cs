using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Text;
using HarmonyLib;

namespace FastBoot.DragCubes
{
    /// Check-only mode of the drag cube cache (DragCubeCache = check). It never writes PartDatabase.cfg; it only
    /// measures whether reusing cubes would have been correct:
    ///
    /// 1. At start, before ModuleManager can delete it, keep a copy of PartDatabase.cfg (Snapshot).
    /// 2. After ModuleManager's patching, note whether the file is gone, which means KSP is about to render every
    ///    part's cubes again, and fingerprint every part from its final config (AfterPatching).
    /// 3. At the main menu, KSP has written the file again. If it rendered cubes this boot, compare them with the
    ///    copy from step 1 and with FastBoot's cache, for the parts whose fingerprint did not change: those are
    ///    exactly the cubes a restore mode would have reused. Write a report, then refresh the cache (AtMainMenu).
    internal static class DragCubeCheck
    {
        public const string Off = "off";
        public const string Check = "check";

        private static bool enabled;
        private static string root, home;
        private static string snapshot;
        private static bool regenerating;
        private static Dictionary<string, string> fingerprints;
        private static double fingerprintMs;
        private static int unresolvedModules;

        private static string Database
        {
            get { return Path.Combine(root, "PartDatabase.cfg"); }
        }

        private static string CachePath
        {
            get { return Path.Combine(Path.Combine(home, "PluginData"), "dragcubes.txt"); }
        }

        /// From the DragCubeCache setting. Safe to call more than once; the first call decides.
        public static void Configure(string mode, string kspRoot, string fastBootHome)
        {
            if (root != null)
                return;
            root = Path.GetFullPath(kspRoot);
            home = fastBootHome;
            string m = (mode ?? "").Trim().ToLowerInvariant();
            enabled = m == Check;
            if (m.Length > 0 && m != Off && m != Check)
                Log.Warning("unknown DragCubeCache '" + mode + "' (off or check); the drag cube cache is off");
        }

        public static void Snapshot()
        {
            if (!enabled || snapshot != null)
                return;
            Guard("snapshot", () =>
            {
                snapshot = File.Exists(Database) ? File.ReadAllText(Database) : "";
            });
        }

        /// From FastBootAddon.ModuleManagerPostLoad: the game database now holds every part's final config.
        public static void AfterPatching()
        {
            if (!enabled || fingerprints != null)
                return;
            Guard("fingerprinting", () =>
            {
                regenerating = !File.Exists(Database);
                var clock = Stopwatch.StartNew();
                fingerprints = FingerprintAll();
                fingerprintMs = clock.Elapsed.TotalMilliseconds;
                Log.Info(string.Format(CultureInfo.InvariantCulture, "drag cube check: {0} parts fingerprinted in {1:F0} ms; {2}",
                    fingerprints.Count, fingerprintMs, regenerating ? "PartDatabase.cfg is gone, so KSP will render every part's cubes" : "PartDatabase.cfg is intact"));
            });
        }

        public static void AtMainMenu()
        {
            if (!enabled || fingerprints == null)
                return;
            Guard("comparison", () =>
            {
                if (!File.Exists(Database))
                {
                    Log.Warning("drag cube check: no PartDatabase.cfg at the main menu");
                    return;
                }
                DragCubeFile current = DragCubeFile.Parse(File.ReadAllText(Database));
                DragCubeCache cache = DragCubeCache.Load(CachePath);
                if (regenerating)
                    Report(current, cache);
                var fresh = new DragCubeCache { Version = current.Version };
                foreach (KeyValuePair<string, List<string>> kv in current.Parts)
                {
                    string fp;
                    if (fingerprints.TryGetValue(kv.Key, out fp))
                        fresh.Parts[kv.Key] = new DragCubeCache.Entry { Fingerprint = fp, Cubes = kv.Value };
                }
                fresh.Save(CachePath);
                Log.Info("drag cube check: cache holds " + fresh.Parts.Count + " parts");
            });
        }

        private static void Guard(string what, Action action)
        {
            try
            {
                action();
            }
            catch (Exception ex)
            {
                Log.Warning("drag cube check: " + what + " failed: " + ex);
            }
        }

        private static Dictionary<string, string> FingerprintAll()
        {
            string gameData = Path.Combine(root, "GameData");
            string global = GlobalKey();
            var folders = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            var modules = new Dictionary<string, string>(StringComparer.Ordinal);
            var result = new Dictionary<string, string>(StringComparer.Ordinal);
            unresolvedModules = 0;
            foreach (UrlDir.UrlConfig uc in GameDatabase.Instance.GetConfigs("PART"))
            {
                ConfigNode part = uc.config;
                IEnumerable<string> folderStamps = Fingerprint.ModelFolders(part, uc.parent.fullPath, gameData).Select(f =>
                {
                    string s;
                    if (!folders.TryGetValue(f, out s))
                        folders[f] = s = Fingerprint.FolderStamp(f);
                    return s;
                });
                IEnumerable<string> moduleStamps = Fingerprint.Modules(part).Select(m =>
                {
                    string s;
                    if (!modules.TryGetValue(m, out s))
                        modules[m] = s = ModuleStamp(m);
                    return s;
                });
                result[uc.url] = Fingerprint.Compute(global, part.ToString(), folderStamps.ToList(), moduleStamps.ToList());
            }
            return result;
        }

        private static string ModuleStamp(string name)
        {
            Type t = AssemblyLoader.GetClassByName(typeof(PartModule), name) ?? typeof(PartModule).Assembly.GetType(name, false);
            if (t == null)
            {
                unresolvedModules++;
                return name + " unresolved";
            }
            Assembly a = t.Assembly;
            return name + " " + a.GetName().Name + " " + a.GetName().Version + " " + Fingerprint.Stamp(a.Location);
        }

        /// KSP's own assembly, and every assembly with a Harmony patch on drag cube generation (KSPCommunityFixes
        /// replaces it): a new version of either can render different cubes from the same part.
        private static string GlobalKey()
        {
            var owners = new SortedSet<string>(StringComparer.Ordinal);
            foreach (MethodBase m in Harmony.GetAllPatchedMethods())
            {
                Patches info = Harmony.GetPatchInfo(m);
                if (info == null)
                    continue;
                bool onDragCubes = m.DeclaringType == typeof(DragCubeSystem);
                foreach (Patch p in info.Prefixes.Concat(info.Postfixes).Concat(info.Transpilers).Concat(info.Finalizers))
                {
                    Type patchType = p.PatchMethod.DeclaringType;
                    if (onDragCubes || (patchType != null && patchType.FullName.IndexOf("DragCube", StringComparison.OrdinalIgnoreCase) >= 0))
                    {
                        Assembly a = p.PatchMethod.DeclaringType.Assembly;
                        owners.Add(a.GetName().Name + " " + a.GetName().Version + " " + Fingerprint.Stamp(a.Location));
                    }
                }
            }
            return "KSP " + Fingerprint.Stamp(typeof(GameDatabase).Assembly.Location) + "; " + string.Join("; ", owners.ToArray());
        }

        private static void Report(DragCubeFile current, DragCubeCache cache)
        {
            DragCubeFile before = string.IsNullOrEmpty(snapshot) ? null : DragCubeFile.Parse(snapshot);
            var sb = new StringBuilder();
            var inv = CultureInfo.InvariantCulture;
            sb.AppendLine("# FastBoot drag cube check");
            sb.AppendLine();
            sb.AppendLine("PartDatabase.cfg was gone after ModuleManager's patching, so KSP rendered the drag cubes of all "
                + current.Parts.Count + " parts again this boot. Nothing was reused; this compares what a cache would have given.");
            sb.AppendLine();
            sb.AppendLine("\"Within rounding\" means every number matches to one step of the fourth significant digit KSP prints.");
            if (before != null)
                Compare(sb, "Against PartDatabase.cfg as it was before this boot", "every part in both files", current, url =>
                {
                    List<string> old;
                    return before.Parts.TryGetValue(url, out old) ? old : null;
                });
            else
                sb.AppendLine().AppendLine("No PartDatabase.cfg existed before this boot, so there is nothing to compare with.");

            int changed = 0, notCached = 0;
            foreach (string url in current.Parts.Keys)
            {
                string fp;
                DragCubeCache.Entry e;
                if (!fingerprints.TryGetValue(url, out fp) || !cache.Parts.TryGetValue(url, out e))
                    notCached++;
                else if (e.Fingerprint != fp)
                    changed++;
            }
            Compare(sb, "Against FastBoot's cache, parts whose fingerprint did not change",
                "these are the cubes a restore mode would have reused; " + changed + " parts changed fingerprint and " + notCached + " were not cached",
                current, url =>
                {
                    string fp;
                    DragCubeCache.Entry e;
                    return fingerprints.TryGetValue(url, out fp) && cache.Parts.TryGetValue(url, out e) && e.Fingerprint == fp ? e.Cubes : null;
                });

            int notInDatabase = fingerprints.Keys.Count(u => !current.Parts.ContainsKey(u));
            sb.AppendLine().AppendLine("## Fingerprints").AppendLine();
            sb.AppendLine(string.Format(inv, "{0} parts fingerprinted in {1:F0} ms; {2} module names did not resolve to a class; {3} fingerprinted parts have no database entry (procedural or failed parts).",
                fingerprints.Count, fingerprintMs, unresolvedModules, notInDatabase));

            string stamp = DateTime.Now.ToString("yyyyMMdd-HHmmss", inv);
            string dir = Path.Combine(Path.Combine(home, "PluginData"), "Diagnostics");
            Directory.CreateDirectory(dir);
            string path = Path.Combine(dir, "dragcubes-check-" + stamp + ".md");
            File.WriteAllText(path, sb.ToString());
            Log.Info("drag cube check written to " + path);
        }

        /// One comparison section: `previous(url)` gives the cubes to compare with, or null to skip the part.
        private static void Compare(StringBuilder sb, string title, string scope, DragCubeFile current, Func<string, List<string>> previous)
        {
            int identical = 0, rounding = 0, different = 0;
            var worst = new List<KeyValuePair<double, string>>();
            foreach (KeyValuePair<string, List<string>> kv in current.Parts)
            {
                List<string> old = previous(kv.Key);
                if (old == null)
                    continue;
                double d;
                switch (DragCubeFile.Compare(old, kv.Value, out d))
                {
                    case DragCubeFile.Match.Identical: identical++; break;
                    case DragCubeFile.Match.WithinTolerance: rounding++; break;
                    default: different++; worst.Add(new KeyValuePair<double, string>(d, kv.Key)); break;
                }
            }
            int total = identical + rounding + different;
            sb.AppendLine().AppendLine("## " + title).AppendLine();
            sb.AppendLine("Scope: " + scope + ".").AppendLine();
            sb.AppendLine(string.Format(CultureInfo.InvariantCulture, "{0} parts compared: {1} identical, {2} within rounding, {3} different.",
                total, identical, rounding, different));
            Log.Info(string.Format(CultureInfo.InvariantCulture, "drag cube check, {0}: {1} parts, {2} identical, {3} within rounding, {4} different",
                title.ToLowerInvariant(), total, identical, rounding, different));
            if (worst.Count == 0)
                return;
            sb.AppendLine().AppendLine("Largest differences (in rounding steps; infinity = different cube names or counts):").AppendLine();
            foreach (KeyValuePair<double, string> w in worst.OrderByDescending(w => w.Key).Take(25))
                sb.AppendLine(string.Format(CultureInfo.InvariantCulture, "- {0,8:F1}  {1}", w.Key, w.Value));
        }
    }
}
