using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using FastBoot.DragCubes;

namespace FastBoot.Tests
{
    /// The drag cube check's pieces that do not need the game: parsing the install's real PartDatabase.cfg,
    /// comparing cubes (identical, within rounding, different), the cache file round trip, and finding the model
    /// folders of every real part config.
    internal static class DragCubesTest
    {
        public static int Run(Options o)
        {
            int failures = 0;
            string database = Path.Combine(o.Ksp, "PartDatabase.cfg");
            if (File.Exists(database))
                failures += RealFile(database);
            else
                Console.WriteLine("no PartDatabase.cfg in the install; real-file checks skipped");
            failures += Comparisons();
            failures += CacheRoundTrip();
            failures += ModelFolders(o);
            return failures == 0 ? 0 : 1;
        }

        private static int Fail(string what)
        {
            Console.WriteLine("FAIL " + what);
            return 1;
        }

        private static int RealFile(string path)
        {
            DragCubeFile f = DragCubeFile.Parse(File.ReadAllText(path));
            int cubes = f.Parts.Values.Sum(c => c.Count);
            Console.WriteLine("PartDatabase.cfg: version '{0}', {1} parts, {2} cube lines", f.Version, f.Parts.Count, cubes);
            int failures = 0;
            if (f.Version == null || f.Parts.Count == 0)
                failures += Fail("parsed no header or no parts");
            int textCubes = File.ReadAllLines(path).Count(l => l.Trim().StartsWith("cube =") || l.Trim().StartsWith("procedural ="));
            if (textCubes != cubes)
                failures += Fail("parsed " + cubes + " cube lines, the file has " + textCubes);
            double d;
            if (f.Parts.Values.Any(c => DragCubeFile.Compare(c, c.ToList(), out d) != DragCubeFile.Match.Identical))
                failures += Fail("a part's cubes are not identical to themselves");
            return failures;
        }

        private static int Comparisons()
        {
            const string a = "cube = Default, 0.1891,0.6503,0.7224, 1.213,0.8567,0.2578, 1.192E-07,-0.009901,1.192E-07, 1.25,0.1572,1.25";
            const string b = "cube = Clean, 1.564,0.7805,0.711, 0.7819,0.7805,0.711, 0,0,0, 1.25,0.625,1.25";
            var cases = new[]
            {
                Tuple.Create("same", new[] { a, b }, new[] { a, b }, DragCubeFile.Match.Identical),
                Tuple.Create("same cubes in another order", new[] { a, b }, new[] { b, a }, DragCubeFile.Match.Identical),
                Tuple.Create("last digit off by one", new[] { a }, new[] { a.Replace("0.6503", "0.6504") }, DragCubeFile.Match.WithinTolerance),
                Tuple.Create("float noise around zero", new[] { a }, new[] { a.Replace("1.192E-07,-0.009901", "0,-0.009901") }, DragCubeFile.Match.WithinTolerance),
                Tuple.Create("a real difference", new[] { a }, new[] { a.Replace("0.6503", "0.6603") }, DragCubeFile.Match.Different),
                Tuple.Create("a cube missing", new[] { a, b }, new[] { a }, DragCubeFile.Match.Different),
                Tuple.Create("a cube renamed", new[] { a }, new[] { a.Replace("Default", "A") }, DragCubeFile.Match.Different),
                Tuple.Create("procedural against a cube", new[] { "procedural = True" }, new[] { a }, DragCubeFile.Match.Different),
                Tuple.Create("procedural both", new[] { "procedural = True" }, new[] { "procedural = True" }, DragCubeFile.Match.Identical),
            };
            int failures = 0;
            foreach (var c in cases)
            {
                double d;
                DragCubeFile.Match got = DragCubeFile.Compare(c.Item2.ToList(), c.Item3.ToList(), out d);
                if (got != c.Item4)
                    failures += Fail("cube comparison '" + c.Item1 + "': " + got + ", expected " + c.Item4);
            }
            Console.WriteLine("cube comparison: {0} of {1} cases right", cases.Length - failures, cases.Length);
            return failures;
        }

        private static int CacheRoundTrip()
        {
            string path = Path.Combine(Path.GetTempPath(), "fastboot-dragcubes-" + System.Diagnostics.Process.GetCurrentProcess().Id + ".txt");
            var cache = new DragCubeCache { Version = "1.12.5.3190 (WindowsPlayer x64)" };
            cache.Parts["Mod/Parts/a/a"] = new DragCubeCache.Entry { Fingerprint = "ab12", Cubes = { "cube = Default, 1,2,3", "procedural = True" } };
            cache.Parts["Mod/Parts/b/b"] = new DragCubeCache.Entry { Fingerprint = "cd34" };
            cache.Save(path);
            DragCubeCache back = DragCubeCache.Load(path);
            File.Delete(path);
            bool ok = back.Version == cache.Version && back.Parts.Count == 2 && back.Parts["Mod/Parts/a/a"].Fingerprint == "ab12"
                && back.Parts["Mod/Parts/a/a"].Cubes.SequenceEqual(cache.Parts["Mod/Parts/a/a"].Cubes) && back.Parts["Mod/Parts/b/b"].Cubes.Count == 0;
            Console.WriteLine("drag cube cache file: " + (ok ? "round trip exact" : "ROUND TRIP BROKEN"));
            return ok ? 0 : 1;
        }

        /// Every PART in the install's cfg files must yield at least one existing model folder, and fingerprints
        /// must be stable and sensitive to each input.
        private static int ModelFolders(Options o)
        {
            int parts = 0, none = 0, missing = 0;
            // Empty files make stock log through Unity, which does not exist offline; they hold no parts anyway.
            foreach (string file in Directory.GetFiles(o.GameData, "*.cfg", SearchOption.AllDirectories)
                .Where(f => new FileInfo(f).Length > 0 && File.ReadAllLines(f).Any(l => l.Trim().Length > 0)))
            {
                ConfigNode root = ConfigNode.Load(file);
                if (root == null)
                    continue;
                foreach (ConfigNode part in root.GetNodes("PART"))
                {
                    parts++;
                    List<string> folders = Fingerprint.ModelFolders(part, file, o.GameData);
                    if (folders.Count == 0)
                        none++;
                    else if (!folders.Any(Directory.Exists))
                        missing++;
                }
            }
            Console.WriteLine("model folders: {0} part configs, {1} with no folder, {2} whose folders all do not exist", parts, none, missing);
            string f1 = Fingerprint.Compute("g", "PART { name = x }", new[] { "F1" }, new[] { "M1" });
            bool stable = f1 == Fingerprint.Compute("g", "PART { name = x }", new[] { "F1" }, new[] { "M1" });
            bool sensitive = new[]
            {
                Fingerprint.Compute("g2", "PART { name = x }", new[] { "F1" }, new[] { "M1" }),
                Fingerprint.Compute("g", "PART { name = y }", new[] { "F1" }, new[] { "M1" }),
                Fingerprint.Compute("g", "PART { name = x }", new[] { "F2" }, new[] { "M1" }),
                Fingerprint.Compute("g", "PART { name = x }", new[] { "F1" }, new[] { "M2" }),
            }.All(x => x != f1);
            Console.WriteLine("fingerprint: " + (stable ? "stable" : "UNSTABLE") + ", " + (sensitive ? "changes with every input" : "MISSES AN INPUT"));
            return parts > 0 && none == 0 && stable && sensitive ? 0 : 1;
        }
    }
}
