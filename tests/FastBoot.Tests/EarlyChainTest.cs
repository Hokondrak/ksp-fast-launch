using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using FastBoot.ColdStart;
using FastBoot.ConfigParsing;
using FastBoot.Early;

namespace FastBoot.Tests
{
    /// The early-start chain, driven by the real native loader. test.ps1 runs this under
    /// `monohost --preload version.dll` with FASTBOOT_EARLY_TARGET=monohost.exe, so Mono's initialisation goes
    /// through the loader's GetProcAddress hook exactly as it does under Unity's player. Then:
    /// - Bootstrap.Start ran before this program did (the loader opened this same FastBoot.dll),
    /// - the config parser installed itself when Assembly-CSharp loaded, before any addon code,
    /// - KSPCommunityFixes loaded afterwards is guarded the moment it loads,
    /// - every cfg loads to the stock tree in two passes (KSP's scan and KSPCF's), both from the prefetch,
    /// - KSPCF's own parser runs only for files FastBoot declines,
    /// - the warm-up ran, and the main-menu call saved a read list of the files the run opened.
    /// With the loader switched off (FASTBOOT_EARLY_DISABLE=1) nothing may happen at all.
    internal static class EarlyChainTest
    {
        public static int Run(Options o, bool loaderEnabled)
        {
            // Read before this test touches anything: JIT-compiling Run already loaded Assembly-CSharp.
            bool started = Bootstrap.Started;
            bool installedOnGameLoad = ConfigLoadPatch.Installed;
            bool guardedEarly = ConfigLoadPatch.KspcfGuarded;
            string log = EarlyLog(o);
            Console.WriteLine("early.log:");
            Console.WriteLine(log.TrimEnd());

            var failures = new List<string>();
            if (!loaderEnabled)
            {
                if (started) failures.Add("Bootstrap.Start ran although the loader was disabled");
                if (installedOnGameLoad) failures.Add("config parser installed although the loader was disabled");
                if (!log.Contains("off: ")) failures.Add("early.log does not say why the loader is off");
                return Report(failures, "disabled loader left the process alone");
            }

            if (!started) failures.Add("Bootstrap.Start did not run");
            if (!installedOnGameLoad) failures.Add("config parser was not installed when Assembly-CSharp loaded");
            if (guardedEarly) failures.Add("KSPCF reported guarded before it was loaded");
            if (!log.Contains("import table restored")) failures.Add("the loader did not restore the import table");
            if (failures.Count > 0)
                return Report(failures, null);

            Type perf = ConfigPatchTest.LoadKspcf(o);
            if (!ConfigLoadPatch.KspcfGuarded)
                failures.Add("KSPCF was not guarded when it loaded");
            ConfigPatchTest.ApplyKspcf(perf);
            ConfigPatchTest.CountKspcfReads(perf);
            Console.WriteLine("ConfigNode.Load prefixes = " + ConfigLoadPatch.PatchGraph());

            string[] files = Directory.GetFiles(o.GameData, "*.cfg", SearchOption.AllDirectories)
                .Where(f => new FileInfo(f).Length > 0 && File.ReadAllLines(f).Length > 0).ToArray();
            var stockRef = files.ToDictionary(f => f, f => StockParser.Tree(StockParser.PreFormat(File.ReadAllLines(f))));
            MethodInfo load = ConfigLoadPatch.LoadMethod();
            int wrong = 0;
            for (int pass = 1; pass <= 2; pass++)
            {
                foreach (string f in files)
                {
                    string diff = StockParser.DiffTrees(stockRef[f], (ConfigNode)load.Invoke(null, new object[] { f, true }));
                    if (diff != null && wrong++ < 5)
                        Console.WriteLine("WRONG (pass " + pass + ") " + f.Substring(o.GameData.Length) + ": " + diff);
                }
            }
            Console.WriteLine(ConfigLoadPatch.Stats());
            int prefetched = ConfigPatchTest.Counter("servedPrefetched");
            int direct = ConfigPatchTest.Counter("servedDirect");
            int declined = ConfigPatchTest.Counter("declined");
            ConfigLoadPatch.ReleasePrefetch();

            if (wrong > 0)
                failures.Add(wrong + " loads differ from stock");
            if (direct != 0 || prefetched + declined != 2 * files.Length)
                failures.Add(string.Format("expected both passes from the prefetch: {0} prefetched + {1} declined of {2} loads, {3} direct",
                    prefetched, declined, 2 * files.Length, direct));
            if (ConfigPatchTest.KspcfReads != declined)
                failures.Add(string.Format("DOUBLE PARSING: KSPCF's parser ran {0} times, FastBoot declined {1}", ConfigPatchTest.KspcfReads, declined));

            // What FastBoot's addons do in Awake and at the main menu. The loader sits in dist/EarlyStart here,
            // where in game it sits in the KSP folder.
            string readlist = Path.Combine(Path.Combine(o.FastBootHome, "PluginData"), "readlist.txt");
            if (File.Exists(readlist))
                File.Delete(readlist);
            Warmup.Start(Path.Combine(o.Dist, "EarlyStart"), o.KspCfgPath, readlist);
            Warmup.BootFinished();
            string[] recorded = File.Exists(readlist) ? File.ReadAllLines(readlist).Where(l => !l.StartsWith("#")).ToArray() : new string[0];
            int recordedCfgs = recorded.Count(l => l.StartsWith("GameData/") && l.EndsWith(".cfg", StringComparison.OrdinalIgnoreCase));
            Console.WriteLine("read list: {0} files, {1} of them GameData cfg files", recorded.Length, recordedCfgs);
            if (recordedCfgs < files.Length / 2)
                failures.Add("the read list misses the cfg files this run opened (" + recordedCfgs + " of " + files.Length + ")");
            if (recorded.Any(l => l.Contains("\\") || Path.IsPathRooted(l)))
                failures.Add("the read list has absolute or backslash paths");
            if (!EarlyLog(o).Contains("warm-up "))
                failures.Add("early.log does not report the warm-up");
            return Report(failures, string.Format("{0} files x 2 passes equal to stock; KSPCF parsed only the {1} declined loads; read list saved",
                files.Length, declined));
        }

        private static string EarlyLog(Options o)
        {
            string path = Path.Combine(Path.Combine(o.FastBootHome, "PluginData"), "early.log");
            if (!File.Exists(path))
                return "";
            // The loader keeps the file open for writing.
            using (var s = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete))
            using (var r = new StreamReader(s))
                return r.ReadToEnd();
        }

        private static int Report(List<string> failures, string success)
        {
            foreach (string f in failures)
                Console.WriteLine("FAIL " + f);
            if (failures.Count == 0)
                Console.WriteLine("ok: " + success);
            return failures.Count == 0 ? 0 : 1;
        }
    }
}
