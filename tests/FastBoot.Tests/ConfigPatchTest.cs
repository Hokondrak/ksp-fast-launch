using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Reflection;
using FastBoot.ConfigParsing;
using HarmonyLib;

namespace FastBoot.Tests
{
    /// ConfigLoadPatch under the real Harmony, with and without KSPCommunityFixes, in both addon orders.
    /// Every GameData cfg must load to the tree the unpatched pipeline would give under the same patch graph
    /// (compared field by field), KSPCF's own parser must run only for files FastBoot declines (no double
    /// parsing), and ModuleManager.ConfigCache must load exactly as before.
    internal static class ConfigPatchTest
    {
        public enum Scenario
        {
            KspcfFirst,
            FastBootFirst,
            Standalone,
        }

        private const BindingFlags Private = BindingFlags.Static | BindingFlags.NonPublic;
        private static int kspcfReads;

        public static int Run(Options o, Scenario scenario)
        {
            Type perf = scenario == Scenario.Standalone ? null : LoadKspcf(o);
            MethodInfo load = ConfigLoadPatch.LoadMethod();
            string[] files = Directory.GetFiles(o.GameData, "*.cfg", SearchOption.AllDirectories)
                .Where(f => new FileInfo(f).Length > 0 && File.ReadAllLines(f).Length > 0).ToArray();
            string cache = Path.Combine(o.GameData, "ModuleManager.ConfigCache");

            if (scenario == Scenario.KspcfFirst)
                ApplyKspcf(perf);

            // Reference results, computed before FastBoot is installed. Under KSPCF the stock text pipeline runs
            // through KSPCF's patched constructors, exactly as FastBoot's trees will.
            if (scenario == Scenario.FastBootFirst)
                ApplyKspcf(perf);
            var stockRef = files.ToDictionary(f => f, f => StockParser.Tree(StockParser.PreFormat(File.ReadAllLines(f))));
            var currentRef = files.ToDictionary(f => f, f => (ConfigNode)load.Invoke(null, new object[] { f, true }));
            ConfigNode cacheRef = File.Exists(cache) ? (ConfigNode)load.Invoke(null, new object[] { cache, true }) : null;
            if (scenario == Scenario.FastBootFirst)
                new Harmony("ConfigNodePerf").UnpatchAll("ConfigNodePerf");

            bool installed = ConfigLoadPatch.Install(o.GameData, o.KspCfgPath, 4);
            if (!installed)
                return 1;
            if (scenario == Scenario.FastBootFirst)
                ApplyKspcf(perf);
            if (perf != null)
                CountKspcfReads(perf);
            Console.WriteLine("scenario {0}: ConfigNode.Load prefixes = {1}", scenario, ConfigLoadPatch.PatchGraph());

            var sw = Stopwatch.StartNew();
            int asStock = 0, asBefore = 0, wrong = 0;
            foreach (string f in files)
            {
                var got = (ConfigNode)load.Invoke(null, new object[] { f, true });
                if (StockParser.DiffTrees(stockRef[f], got) == null) asStock++;
                else if (StockParser.DiffTrees(currentRef[f], got) == null) asBefore++;
                else if (wrong++ < 5) Console.WriteLine("WRONG " + f.Substring(o.GameData.Length) + ": " + StockParser.DiffTrees(stockRef[f], got));
            }
            double seconds = sw.Elapsed.TotalSeconds;
            int readsDuringCfgLoads = kspcfReads;
            string cacheDiff = cacheRef == null ? null : StockParser.DiffTrees(cacheRef, (ConfigNode)load.Invoke(null, new object[] { cache, true }));
            string again = StockParser.DiffTrees(stockRef[files[0]], (ConfigNode)load.Invoke(null, new object[] { files[0], true }));
            load.Invoke(null, new object[] { files[0], false }); // localization branch must not throw

            Console.WriteLine("{0} files: {1} equal to stock, {2} equal to the pre-FastBoot result only, {3} wrong; {4:F2}s",
                files.Length, asStock, asBefore, wrong, seconds);
            Console.WriteLine(ConfigLoadPatch.Stats());
            int declined = Counter("declined");
            bool noDoubleParse = perf == null || readsDuringCfgLoads == declined;
            if (perf != null)
                Console.WriteLine("KSPCF's parser ran {0} times for cfg files; FastBoot declined {1}: {2}",
                    readsDuringCfgLoads, declined, noDoubleParse ? "no double parsing" : "DOUBLE PARSING");
            Console.WriteLine("ConfigCache: {0}; repeated load: {1}", cacheDiff == null ? "unchanged" : "DIFFERENT: " + cacheDiff,
                again == null ? "identical" : "DIFFERENT");
            ConfigLoadPatch.ReleasePrefetch();
            return wrong == 0 && cacheDiff == null && again == null && noDoubleParse ? 0 : 1;
        }

        internal static Type LoadKspcf(Options o)
        {
            Assembly a = Assembly.LoadFrom(Path.Combine(Path.Combine(Path.Combine(o.GameData, "KSPCommunityFixes"), "Plugins"), "KSPCommunityFixes.dll"));
            return a.GetType("KSPCommunityFixes.Performance.ConfigNodePerf", true);
        }

        /// KSPCF 1.41.1's ConfigNodePerf.Awake, minus DontDestroyOnLoad and the two save-scene clean toggles.
        internal static void ApplyKspcf(Type perf)
        {
            var h = new Harmony("ConfigNodePerf");
            Func<string, HarmonyMethod> pre = name => new HarmonyMethod(perf.GetMethod(name, Private));
            h.Patch(AccessTools.Method(typeof(ConfigNode), "Load", new[] { typeof(string), typeof(bool) }), pre("ConfigNode_Load_Prefix"));
            h.Patch(AccessTools.Method(typeof(ConfigNode), "Parse"), pre("ConfigNode_Parse_Prefix"));
            h.Patch(AccessTools.Method(typeof(ConfigNode), "Save", new[] { typeof(string), typeof(string) }), pre("ConfigNode_Save_Prefix"));
            h.Patch(AccessTools.Method(typeof(ConfigNode), "CleanupInput"), pre("ConfigNode_CleanupInput_Prefix"));
            h.Patch(AccessTools.Method(typeof(ConfigNode), "CopyToRecursive"), pre("ConfigNode_CopyToRecursive_Prefix"));
            h.Patch(AccessTools.Constructor(typeof(ConfigNode), new[] { typeof(string) }), pre("ConfigNode_Ctor1_Prefix"));
            h.Patch(AccessTools.Constructor(typeof(ConfigNode), new[] { typeof(string), typeof(string) }), pre("ConfigNode_Ctor2_Prefix"));
            h.Patch(AccessTools.Method(typeof(ConfigNode), "AddValue", new[] { typeof(string), typeof(string), typeof(string) }), pre("ConfigNode_AddValue_Prefix"));
            h.Patch(AccessTools.Method(typeof(ConfigNode), "AddValue", new[] { typeof(string), typeof(string) }), pre("ConfigNode_AddValue2_Prefix"));
            h.Patch(AccessTools.Method(typeof(ConfigNode.Value), "Sanitize"), pre("ConfigNodeValue_Sanitize_Prefix"));
            h.Patch(AccessTools.Method(typeof(ConfigNode.ValueList), "Add"), pre("ConfigNodeValueList_Add_Prefix"));
        }

        internal static void CountKspcfReads(Type perf)
        {
            new Harmony("FastBoot.Tests").Patch(perf.GetMethod("ReadFile", Private),
                new HarmonyMethod(typeof(ConfigPatchTest).GetMethod("OnKspcfRead", Private)));
        }

        private static void OnKspcfRead()
        {
            kspcfReads++;
        }

        /// How many times KSPCF's own parser ran since CountKspcfReads.
        internal static int KspcfReads { get { return kspcfReads; } }

        /// One of ConfigLoadPatch's private counters.
        internal static int Counter(string field)
        {
            return (int)typeof(ConfigLoadPatch).GetField(field, Private).GetValue(null);
        }
    }
}
