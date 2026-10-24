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
    /// Reusing drag cubes across mod changes (DragCubeCache = check | restore).
    ///
    /// After a mod change ModuleManager deletes PartDatabase.cfg, KSP's drag cube cache, and KSP renders every
    /// part's cubes again. KSP keys that file by part url only, so it cannot tell what changed; FastBoot can:
    ///
    /// 1. At start, before ModuleManager runs, keep a copy of PartDatabase.cfg (Snapshot).
    /// 2. After ModuleManager's patching, fingerprint every part from its final config (Fingerprint). If the file
    ///    is gone and the mode is restore, write it back with the cubes of every part whose fingerprint is unchanged
    ///    and whose cubes KSP has been seen to render identically twice (DragCubeCache.State.Stable). KSP then
    ///    renders only the rest: changed parts, parts never verified, and parts that render differently each time
    ///    (animated ones) (AfterPatching).
    /// 3. At the main menu KSP has written the complete file. Every part KSP rendered this boot is compared with
    ///    the cache, which is how parts become Stable or Unstable; then the cache is refreshed and, on a boot that
    ///    rendered cubes, a report is written (AtMainMenu).
    ///
    /// Check mode does 1 and 3 and the fingerprinting, but never writes PartDatabase.cfg.
    internal static class DragCubeReuse
    {
        public const string Off = "off";
        public const string Check = "check";
        public const string Restore = "restore";

        private static bool enabled, restore;
        private static string root, home;
        private static string snapshot;
        private static bool regenerating;
        private static Dictionary<string, string> fingerprints;
        private static DragCubeCache cache;
        private static readonly HashSet<string> Restored = new HashSet<string>(StringComparer.Ordinal);
        private static double fingerprintMs;
        private static int unresolvedModules;

        private static string Database
        {
            get { return Path.Combine(root, "PartDatabase.cfg"); }
        }

        private static string PluginData
        {
            get { return Path.Combine(home, "PluginData"); }
        }

        private static string CachePath
        {
            get { return Path.Combine(PluginData, "dragcubes.txt"); }
        }

        /// From the DragCubeCache setting. Safe to call more than once; the first call decides.
        public static void Configure(string mode, string kspRoot, string fastBootHome)
        {
            if (root != null)
                return;
            root = Path.GetFullPath(kspRoot);
            home = fastBootHome;
            string m = (mode ?? "").Trim().ToLowerInvariant();
            enabled = m == Check || m == Restore;
            restore = m == Restore;
            if (m.Length > 0 && m != Off && !enabled)
                Log.Warning("unknown DragCubeCache '" + mode + "' (off, check or restore); the drag cube cache is off");
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

        /// From FastBootAddon.ModuleManagerPostLoad: the game database holds every part's final config, and KSP has
        /// not read PartDatabase.cfg yet.
        public static void AfterPatching()
        {
            if (!enabled || fingerprints != null)
                return;
            regenerating = !File.Exists(Database);
            if (!regenerating)
            {
                // Nothing to restore and nothing KSP will render: the cache stays as the last mod change left it.
                Log.Info("drag cube cache: PartDatabase.cfg is intact, nothing to do");
                return;
            }
            Guard("fingerprinting", () =>
            {
                var clock = Stopwatch.StartNew();
                fingerprints = FingerprintAll();
                fingerprintMs = clock.Elapsed.TotalMilliseconds;
                cache = DragCubeCache.Load(CachePath);
                Log.Info(string.Format(CultureInfo.InvariantCulture, "drag cube cache: {0} parts fingerprinted in {1:F0} ms; {2}",
                    fingerprints.Count, fingerprintMs, regenerating ? "PartDatabase.cfg was deleted (mod change)" : "PartDatabase.cfg is intact"));
            });
            if (restore && regenerating && cache != null)
                Guard("restore", RestoreDatabase);
        }

        private static void RestoreDatabase()
        {
            Dictionary<string, List<string>> reusable = cache.Reusable(fingerprints);
            if (reusable.Count == 0 || cache.Version == null)
            {
                Log.Info("drag cube cache: nothing verified to restore yet; KSP renders every part this time");
                return;
            }
            // In the game database's part order; KSP rewrites the file in its own order anyway.
            var parts = fingerprints.Keys.Where(reusable.ContainsKey).Select(u => new KeyValuePair<string, List<string>>(u, reusable[u])).ToList();
            string tmp = Database + ".fastboot.tmp";
            File.WriteAllText(tmp, DragCubeFile.Write(cache.Version, parts), new UTF8Encoding(false));
            if (File.Exists(Database))
            {
                // Something recreated it after the check above; leave it alone.
                File.Delete(tmp);
                return;
            }
            File.Move(tmp, Database);
            foreach (KeyValuePair<string, List<string>> p in parts)
                Restored.Add(p.Key);
            Log.Info("drag cube cache: restored the cubes of " + parts.Count + " unchanged parts; KSP renders the other "
                + (fingerprints.Count - parts.Count) + " (changed, not yet verified, or animated)");
        }

        public static void AtMainMenu()
        {
            if (!enabled || fingerprints == null || cache == null)
                return;
            Guard("cache update", () =>
            {
                if (!File.Exists(Database))
                {
                    Log.Warning("drag cube cache: no PartDatabase.cfg at the main menu");
                    return;
                }
                DragCubeFile current = DragCubeFile.Parse(File.ReadAllText(Database));
                var fresh = new DragCubeCache { Version = current.Version };
                foreach (string url in current.Order)
                {
                    string fp;
                    if (!fingerprints.TryGetValue(url, out fp))
                        continue;
                    DragCubeCache.Entry previous;
                    cache.Parts.TryGetValue(url, out previous);
                    List<string> cubes = current.Parts[url];
                    bool rendered = regenerating && !Restored.Contains(url);
                    fresh.Parts[url] = new DragCubeCache.Entry
                    {
                        Fingerprint = fp,
                        Cubes = cubes,
                        State = DragCubeCache.Next(previous, fp, cubes, rendered),
                    };
                }
                if (regenerating)
                    Report(current, fresh);
                fresh.Save(CachePath);
                Log.Info(string.Format(CultureInfo.InvariantCulture, "drag cube cache: {0} parts stored ({1} stable, {2} unstable, {3} unverified)",
                    fresh.Parts.Count, fresh.Parts.Values.Count(e => e.State == DragCubeCache.State.Stable),
                    fresh.Parts.Values.Count(e => e.State == DragCubeCache.State.Unstable), fresh.Parts.Values.Count(e => e.State == DragCubeCache.State.Unverified)));
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
                Log.Warning("drag cube cache: " + what + " failed, KSP's own handling is unchanged: " + ex);
            }
        }

        private static Dictionary<string, string> FingerprintAll()
        {
            string gameData = Path.Combine(root, "GameData");
            string global = GlobalKey();
            var folders = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            var modFolders = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            var modules = new Dictionary<string, string>(StringComparer.Ordinal);
            var result = new Dictionary<string, string>(StringComparer.Ordinal);
            unresolvedModules = 0;
            foreach (UrlDir.UrlConfig uc in GameDatabase.Instance.GetConfigs("PART"))
            {
                ConfigNode part = uc.config;
                List<string> folderStamps = Fingerprint.ModelFolders(part, uc.parent.fullPath, gameData).Select(f =>
                {
                    string s;
                    if (!folders.TryGetValue(f, out s))
                        folders[f] = s = Fingerprint.FolderStamp(f);
                    return s;
                }).ToList();
                // Everything in the part's own mod folder: a mod update re-renders all of that mod's parts.
                string modFolder = Fingerprint.ModFolder(uc.parent.fullPath, gameData);
                if (modFolder != null)
                {
                    string ms;
                    if (!modFolders.TryGetValue(modFolder, out ms))
                        modFolders[modFolder] = ms = Fingerprint.ModFolderStamp(modFolder);
                    folderStamps.Add(ms);
                }
                List<string> moduleStamps = Fingerprint.Modules(part).Select(m =>
                {
                    string s;
                    if (!modules.TryGetValue(m, out s))
                        modules[m] = s = ModuleStamp(m);
                    return s;
                }).ToList();
                string fp = Fingerprint.Compute(global, part.ToString(), folderStamps, moduleStamps);
                // A module that does not resolve to a class could be anything: never reuse such a part's cubes.
                result[uc.url] = moduleStamps.Any(m => m.EndsWith(Unresolved, StringComparison.Ordinal)) ? Fingerprint.Ineligible + fp : fp;
            }
            return result;
        }

        private const string Unresolved = " unresolved";

        /// The module's class and every class it derives from, by assembly: code in a base class from another
        /// assembly can change the part as much as the module's own.
        private static string ModuleStamp(string name)
        {
            Type t = AssemblyLoader.GetClassByName(typeof(PartModule), name) ?? typeof(PartModule).Assembly.GetType(name, false);
            if (t == null)
            {
                unresolvedModules++;
                return name + Unresolved;
            }
            var assemblies = new List<Assembly>();
            for (Type c = t; c != null && c != typeof(object); c = c.BaseType)
                if (!assemblies.Contains(c.Assembly))
                    assemblies.Add(c.Assembly);
            return name + " " + string.Join(" ", assemblies.Select(a => a.GetName().Name + " " + a.GetName().Version + " " + Fingerprint.Stamp(a.Location)).ToArray());
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

        private static void Report(DragCubeFile current, DragCubeCache updated)
        {
            var inv = CultureInfo.InvariantCulture;
            string stamp = DateTime.Now.ToString("yyyyMMdd-HHmmss", inv);
            string dir = Path.Combine(PluginData, "Diagnostics");
            Directory.CreateDirectory(dir);
            if (!string.IsNullOrEmpty(snapshot))
                File.WriteAllText(Path.Combine(dir, "PartDatabase-before-last-mod-change.txt"), snapshot); // not .cfg: never a config

            var rendered = current.Order.Where(u => !Restored.Contains(u)).ToList();
            var sb = new StringBuilder();
            sb.AppendLine("# FastBoot drag cube cache report");
            sb.AppendLine();
            sb.AppendLine("ModuleManager deleted PartDatabase.cfg (a mod change). Mode: " + (restore ? "restore" : "check (nothing reused)") + ".");
            sb.AppendLine();
            sb.AppendLine(string.Format(inv, "- restored from FastBoot's cache: {0} parts", Restored.Count));
            sb.AppendLine(string.Format(inv, "- rendered by KSP: {0} parts", rendered.Count));
            sb.AppendLine(string.Format(inv, "- fingerprinting: {0} parts in {1:F0} ms; {2} module names did not resolve to a class",
                fingerprints.Count, fingerprintMs, unresolvedModules));
            sb.AppendLine();
            sb.AppendLine("\"Within rounding\" means every number matches to one step of the fourth significant digit KSP prints.");

            Compare(sb, "Rendered this boot vs FastBoot's cache, same fingerprint",
                "the verification that decides which parts may be restored next time", rendered, current, url =>
                {
                    DragCubeCache.Entry e;
                    return cache.Parts.TryGetValue(url, out e) && e.Fingerprint == fingerprints[url] ? e.Cubes : null;
                });
            DragCubeFile before = string.IsNullOrEmpty(snapshot) ? null : DragCubeFile.Parse(snapshot);
            if (before != null)
                Compare(sb, "Rendered this boot vs PartDatabase.cfg before this boot", "includes parts the mod change really changed", rendered, current, url =>
                {
                    List<string> old;
                    return before.Parts.TryGetValue(url, out old) ? old : null;
                });

            sb.AppendLine().AppendLine("## Cache after this boot").AppendLine();
            foreach (DragCubeCache.State s in new[] { DragCubeCache.State.Stable, DragCubeCache.State.Unstable, DragCubeCache.State.Unverified })
                sb.AppendLine(string.Format(inv, "- {0}: {1}", s.ToString().ToLowerInvariant(), updated.Parts.Values.Count(e => e.State == s)));
            var unstable = updated.Parts.Where(kv => kv.Value.State == DragCubeCache.State.Unstable).Select(kv => kv.Key).OrderBy(u => u, StringComparer.Ordinal).ToList();
            if (unstable.Count > 0)
            {
                sb.AppendLine().AppendLine("Always rendered by KSP (seen to render differently from the same inputs):").AppendLine();
                foreach (string u in unstable)
                    sb.AppendLine("- " + u);
            }

            string path = Path.Combine(dir, "dragcubes-" + stamp + ".md");
            File.WriteAllText(path, sb.ToString());
            Log.Info("drag cube cache report written to " + path);
        }

        /// One comparison section over `urls`: `previous(url)` gives the cubes to compare with, or null to skip.
        private static void Compare(StringBuilder sb, string title, string scope, List<string> urls, DragCubeFile current, Func<string, List<string>> previous)
        {
            int identical = 0, rounding = 0, different = 0;
            var worst = new List<KeyValuePair<double, string>>();
            var old = new Dictionary<string, List<string>>(StringComparer.Ordinal);
            foreach (string url in urls)
            {
                List<string> p = fingerprints.ContainsKey(url) ? previous(url) : null;
                if (p == null)
                    continue;
                double d;
                switch (DragCubeFile.Compare(p, current.Parts[url], out d))
                {
                    case DragCubeFile.Match.Identical: identical++; break;
                    case DragCubeFile.Match.WithinTolerance: rounding++; break;
                    default:
                        different++;
                        worst.Add(new KeyValuePair<double, string>(d, url));
                        old[url] = p;
                        break;
                }
            }
            int total = identical + rounding + different;
            sb.AppendLine().AppendLine("## " + title).AppendLine();
            sb.AppendLine("Scope: " + scope + ".").AppendLine();
            sb.AppendLine(string.Format(CultureInfo.InvariantCulture, "{0} parts compared: {1} identical, {2} within rounding, {3} different.",
                total, identical, rounding, different));
            Log.Info(string.Format(CultureInfo.InvariantCulture, "drag cube cache, {0}: {1} parts, {2} identical, {3} within rounding, {4} different",
                title.ToLowerInvariant(), total, identical, rounding, different));
            var top = worst.OrderByDescending(w => w.Key).Take(12).ToList();
            if (top.Count == 0)
                return;
            sb.AppendLine().AppendLine("Largest differences, in rounding steps (infinity: different cube names or counts):");
            foreach (KeyValuePair<double, string> w in top)
            {
                sb.AppendLine().AppendLine(string.Format(CultureInfo.InvariantCulture, "### {0} ({1:F1})", w.Value, w.Key)).AppendLine();
                sb.AppendLine("```");
                foreach (string c in old[w.Value])
                    sb.AppendLine("before  " + c);
                foreach (string c in current.Parts[w.Value])
                    sb.AppendLine("now     " + c);
                sb.AppendLine("```");
            }
        }
    }
}
