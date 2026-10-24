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
            failures += States();
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
            // What restore mode writes must be what KSP itself writes, byte for byte.
            string written = DragCubeFile.Write(f.Version, f.Order.Select(u => new KeyValuePair<string, List<string>>(u, f.Parts[u])));
            string original = File.ReadAllText(path);
            if (written != original)
            {
                int at = Enumerable.Range(0, Math.Min(written.Length, original.Length)).FirstOrDefault(i => written[i] != original[i]);
                failures += Fail("rewritten PartDatabase.cfg differs from KSP's at character " + at + " of " + original.Length);
            }
            else
                Console.WriteLine("PartDatabase.cfg rewritten from its parsed parts: byte for byte identical ({0} characters)", original.Length);
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
            cache.Parts["Mod/Parts/a/a"] = new DragCubeCache.Entry { Fingerprint = "ab12", State = DragCubeCache.State.Stable, Cubes = { "cube = Default, 1,2,3", "procedural = True" } };
            cache.Parts["Mod/Parts/b/b"] = new DragCubeCache.Entry { Fingerprint = "cd34", State = DragCubeCache.State.Unstable };
            cache.Save(path);
            DragCubeCache back = DragCubeCache.Load(path);
            bool ok = back.Version == cache.Version && back.Parts.Count == 2 && back.Parts["Mod/Parts/a/a"].Fingerprint == "ab12"
                && back.Parts["Mod/Parts/a/a"].State == DragCubeCache.State.Stable && back.Parts["Mod/Parts/b/b"].State == DragCubeCache.State.Unstable
                && back.Parts["Mod/Parts/a/a"].Cubes.SequenceEqual(cache.Parts["Mod/Parts/a/a"].Cubes) && back.Parts["Mod/Parts/b/b"].Cubes.Count == 0;
            // A version 1 cache (no states) loads with every part unverified.
            File.WriteAllLines(path, new[] { "# FastBoot drag cube cache v1", "V\tv", "P\tMod/Parts/a/a\tab12", "C\tcube = Default, 1,2,3" });
            DragCubeCache v1 = DragCubeCache.Load(path);
            File.Delete(path);
            ok = ok && v1.Parts.Count == 1 && v1.Parts["Mod/Parts/a/a"].State == DragCubeCache.State.Unverified && v1.Parts["Mod/Parts/a/a"].Cubes.Count == 1;
            Console.WriteLine("drag cube cache file: " + (ok ? "round trip exact, version 1 files load as unverified" : "ROUND TRIP BROKEN"));
            return ok ? 0 : 1;
        }

        /// The rules that decide what restore mode may reuse.
        private static int States()
        {
            const string cube = "cube = Default, 1.25,0.5,0.75, 1,1,1, 0,0,0, 1,1,1";
            Func<DragCubeCache.State, DragCubeCache.Entry> prev = s => new DragCubeCache.Entry { Fingerprint = "f", State = s, Cubes = { cube } };
            var same = new List<string> { cube };
            var rounding = new List<string> { cube.Replace("1.25", "1.251") };
            var other = new List<string> { cube.Replace("0.5,", "0.9,") };
            var S = DragCubeCache.State.Stable;
            var U = DragCubeCache.State.Unstable;
            var V = DragCubeCache.State.Unverified;
            var cases = new[]
            {
                Tuple.Create("new part", (DragCubeCache.Entry)null, "f", same, true, V),
                Tuple.Create("inputs changed", prev(S), "g", same, true, V),
                Tuple.Create("rendered the same again", prev(V), "f", same, true, S),
                Tuple.Create("rendered within rounding", prev(V), "f", rounding, true, S),
                Tuple.Create("rendered differently", prev(V), "f", other, true, U),
                Tuple.Create("stable, rendered differently", prev(S), "f", other, true, U),
                Tuple.Create("unstable stays unstable", prev(U), "f", same, true, U),
                Tuple.Create("unstable, loaded", prev(U), "f", other, false, U),
                Tuple.Create("stable, restored or loaded", prev(S), "f", same, false, S),
                Tuple.Create("stable, file edited without a render", prev(S), "f", other, false, V),
                Tuple.Create("unverified, loaded", prev(V), "f", same, false, V),
            };
            int failures = 0;
            foreach (var c in cases)
            {
                DragCubeCache.State got = DragCubeCache.Next(c.Item2, c.Item3, c.Item4, c.Item5);
                if (got != c.Item6)
                    failures += Fail("state '" + c.Item1 + "': " + got + ", expected " + c.Item6);
            }
            int transitionFailures = failures;

            var cache = new DragCubeCache();
            cache.Parts["stable"] = prev(S);
            cache.Parts["unstable"] = prev(U);
            cache.Parts["unverified"] = prev(V);
            cache.Parts["changed"] = prev(S);
            cache.Parts["empty"] = new DragCubeCache.Entry { Fingerprint = "f", State = S };
            var fps = new Dictionary<string, string> { { "stable", "f" }, { "unstable", "f" }, { "unverified", "f" }, { "changed", "g" }, { "empty", "f" }, { "uncached", "f" } };
            Dictionary<string, List<string>> reusable = cache.Reusable(fps);
            if (reusable.Count != 1 || !reusable.ContainsKey("stable"))
                failures += Fail("reusable parts: " + string.Join(", ", reusable.Keys.ToArray()) + ", expected only 'stable'");
            Console.WriteLine("restore rules: {0} of {1} state transitions right; reusable: {2}", cases.Length - transitionFailures, cases.Length,
                string.Join(", ", reusable.Keys.ToArray()));
            return failures;
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
