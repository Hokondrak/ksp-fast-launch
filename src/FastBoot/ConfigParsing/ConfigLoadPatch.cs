using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Reflection;
using System.Threading;
using HarmonyLib;
using KSP.Localization;

namespace FastBoot.ConfigParsing
{
    /// Serves ConfigNode.Load for GameData .cfg files from kspcfg. Tree building stays stock RecurseFormat, so
    /// node names, ids and value sanitising are KSP's own, including KSPCommunityFixes' patched constructors.
    ///
    /// Works with or without KSPCommunityFixes and in any addon order:
    /// - FastBoot's prefix runs first by Harmony priority. When it serves a file it skips the original and marks
    ///   the call as handled.
    /// - Harmony still runs later prefixes, so KSPCF's ConfigNodePerf Load prefix is itself patched to return
    ///   early for handled calls. Calls FastBoot declines (other file types, content outside kspcfg's fast path)
    ///   go to KSPCF or stock exactly as before.
    /// Nothing is unpatched, so a failure part-way leaves KSP's behaviour unchanged.
    internal static class ConfigLoadPatch
    {
        public const string HarmonyId = "FastBoot.ConfigParser";
        private const string KspcfType = "KSPCommunityFixes.Performance.ConfigNodePerf";
        private const string KspcfLoadPrefix = "ConfigNode_Load_Prefix";

        private static KspCfg kspcfg;
        private static Prefetch prefetch;
        private static Func<List<string[]>, ConfigNode> recurseFormat;
        private static string gameDataKey;

        [ThreadStatic]
        private static bool handled;

        private static int servedPrefetched, servedDirect, declined, passedThrough;
        private static long serveTicks;

        public static bool Installed { get; private set; }
        public static bool KspcfGuarded { get; private set; }

        public static bool Install(string gameDataDir, string nativePath, int threads)
        {
            string error;
            kspcfg = KspCfg.Load(nativePath, out error);
            if (kspcfg == null)
            {
                Log.Warning("config parser disabled: " + error);
                return false;
            }
            MethodInfo rf = typeof(ConfigNode).GetMethod("RecurseFormat", BindingFlags.Static | BindingFlags.NonPublic,
                null, new[] { typeof(List<string[]>) }, null);
            MethodInfo load = LoadMethod();
            if (rf == null || load == null)
            {
                Log.Warning("config parser disabled: ConfigNode.RecurseFormat or ConfigNode.Load(string, bool) not found");
                return false;
            }
            recurseFormat = (Func<List<string[]>, ConfigNode>)Delegate.CreateDelegate(typeof(Func<List<string[]>, ConfigNode>), rf);
            gameDataKey = PathKey.Of(gameDataDir).TrimEnd('\\') + "\\";

            var harmony = new Harmony(HarmonyId);
            try
            {
                MethodInfo kspcf = FindKspcfLoadPrefix();
                if (kspcf != null)
                {
                    harmony.Patch(kspcf, new HarmonyMethod(Own("SkipWhenHandled")));
                    KspcfGuarded = true;
                }
                var prefix = new HarmonyMethod(Own("LoadPrefix")) { priority = Priority.First };
                harmony.Patch(load, prefix);
            }
            catch (Exception ex)
            {
                harmony.UnpatchAll(HarmonyId);
                KspcfGuarded = false;
                Log.Warning("config parser disabled, ConfigNode.Load unchanged: " + ex);
                return false;
            }
            prefetch = Prefetch.Start(kspcfg, gameDataDir, threads);
            Installed = true;
            Log.Info("config parser installed (" + (KspcfGuarded ? "alongside KSPCommunityFixes" : "standalone") + ", "
                + threads + " prefetch threads)");
            return true;
        }

        public static MethodInfo LoadMethod()
        {
            return typeof(ConfigNode).GetMethod("Load", BindingFlags.Static | BindingFlags.Public, null,
                new[] { typeof(string), typeof(bool) }, null);
        }

        private static MethodInfo Own(string name)
        {
            return typeof(ConfigLoadPatch).GetMethod(name, BindingFlags.Static | BindingFlags.NonPublic);
        }

        private static MethodInfo FindKspcfLoadPrefix()
        {
            foreach (Assembly a in AppDomain.CurrentDomain.GetAssemblies())
            {
                if (a.GetName().Name != "KSPCommunityFixes")
                    continue;
                Type t = a.GetType(KspcfType, false);
                return t == null ? null : t.GetMethod(KspcfLoadPrefix, BindingFlags.Static | BindingFlags.NonPublic | BindingFlags.Public);
            }
            return null;
        }

        // Prefix on ConfigNode.Load(string, bool).
        private static bool LoadPrefix(string fileFullName, bool bypassLocalization, ref ConfigNode __result)
        {
            handled = false;
            if (fileFullName == null || !fileFullName.EndsWith(".cfg", StringComparison.OrdinalIgnoreCase))
                return PassThrough();
            string key;
            try
            {
                key = PathKey.Of(fileFullName);
            }
            catch (Exception)
            {
                return PassThrough();
            }
            if (!key.StartsWith(gameDataKey, StringComparison.Ordinal))
                return PassThrough();

            long start = Stopwatch.GetTimestamp();
            List<string[]> lines = null;
            bool prefetched = prefetch != null && prefetch.TryTake(fileFullName, key, out lines);
            if (!prefetched)
                lines = kspcfg.FromFile(fileFullName);
            if (lines == null)
            {
                Interlocked.Increment(ref declined);
                return true; // KSPCF (if present) or stock handles it, exactly as without FastBoot
            }
            // Stock LoadFromStringArray: no tokens -> null; otherwise RecurseFormat, then optional localization.
            __result = lines.Count == 0 ? null : recurseFormat(lines);
            if (__result != null && !bypassLocalization && Localizer.Instance != null)
                Localizer.TranslateBranch(__result);
            if (prefetched)
                Interlocked.Increment(ref servedPrefetched);
            else
                Interlocked.Increment(ref servedDirect);
            Interlocked.Add(ref serveTicks, Stopwatch.GetTimestamp() - start);
            handled = true;
            return false;
        }

        // Prefix on KSPCF's ConfigNodePerf.ConfigNode_Load_Prefix: here __result is that prefix's bool return
        // value ("run the original?"), which stays false because FastBoot already produced the ConfigNode.
        private static bool SkipWhenHandled(ref bool __result)
        {
            if (!handled)
                return true;
            handled = false;
            __result = false;
            return false;
        }

        private static bool PassThrough()
        {
            Interlocked.Increment(ref passedThrough);
            return true;
        }

        /// Every prefix on ConfigNode.Load. Besides FastBoot's (and KSPCF's, which it guards), any other means
        /// another mod parses cfg files too.
        public static string PatchGraph()
        {
            Patches info = Harmony.GetPatchInfo(LoadMethod());
            if (info == null)
                return "(none)";
            var owners = new List<string>();
            foreach (Patch p in info.Prefixes)
                owners.Add(p.owner + "@" + p.priority);
            return string.Join(", ", owners.ToArray());
        }

        public static string Stats()
        {
            double ms = serveTicks * 1000.0 / Stopwatch.Frequency;
            return string.Format(System.Globalization.CultureInfo.InvariantCulture,
                "config parser served {0} pre-parsed + {1} direct in {2:F0} ms; {3} declined to {4}; {5} other loads passed through; prefetch {6:F0} ms",
                servedPrefetched, servedDirect, ms, declined, KspcfGuarded ? "KSPCF" : "stock", passedThrough,
                prefetch == null ? 0 : prefetch.Elapsed.Elapsed.TotalMilliseconds);
        }

        public static void ReleasePrefetch()
        {
            if (prefetch != null)
                prefetch.Release();
        }
    }
}
