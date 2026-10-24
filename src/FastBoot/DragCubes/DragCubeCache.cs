using System;
using System.Collections.Generic;
using System.IO;
using System.Text;

namespace FastBoot.DragCubes
{
    /// FastBoot's own copy of every part's drag cubes, with the fingerprint of the inputs they were rendered from
    /// and whether KSP has been seen to render them the same way twice. Kept in PluginData (which GameDatabase
    /// never loads) as plain text:
    ///
    ///     V &lt;tab&gt; PartDatabase.cfg version line
    ///     P &lt;tab&gt; url &lt;tab&gt; fingerprint &lt;tab&gt; state
    ///     C &lt;tab&gt; cube line as in PartDatabase.cfg
    internal sealed class DragCubeCache
    {
        // v3: stable means an identical re-render (v2 also accepted rounding) and fingerprints cover more inputs, so
        // older caches load with every part unverified.
        private const string Header = "# FastBoot drag cube cache v3";
        private static readonly string[] OlderHeaders = { "# FastBoot drag cube cache v1", "# FastBoot drag cube cache v2" };

        public enum State
        {
            /// Never compared with a second render of the same inputs: KSP renders it.
            Unverified,
            /// A fresh render of the same inputs matched: safe to reuse.
            Stable,
            /// A fresh render of the same inputs came out different (animated parts): KSP always renders it.
            Unstable,
        }

        public sealed class Entry
        {
            public string Fingerprint;
            public State State;
            public List<string> Cubes = new List<string>();
        }

        public string Version;
        public readonly Dictionary<string, Entry> Parts = new Dictionary<string, Entry>(StringComparer.Ordinal);

        public static DragCubeCache Load(string path)
        {
            var cache = new DragCubeCache();
            if (!File.Exists(path))
                return cache;
            string[] lines = File.ReadAllLines(path);
            if (lines.Length == 0 || (lines[0] != Header && Array.IndexOf(OlderHeaders, lines[0]) < 0))
                return cache; // unknown format: start over
            bool older = lines[0] != Header;
            Entry current = null;
            foreach (string line in lines)
            {
                string[] f = line.Split('\t');
                if (f[0] == "V" && f.Length == 2)
                    cache.Version = f[1];
                else if (f[0] == "P" && (f.Length == 3 || f.Length == 4))
                    cache.Parts[f[1]] = current = new Entry { Fingerprint = f[2], State = f.Length == 4 && !older ? ParseState(f[3]) : State.Unverified };
                else if (f[0] == "C" && f.Length == 2 && current != null)
                    current.Cubes.Add(f[1]);
            }
            return cache;
        }

        private static State ParseState(string s)
        {
            switch (s)
            {
                case "stable": return State.Stable;
                case "unstable": return State.Unstable;
                default: return State.Unverified;
            }
        }

        public void Save(string path)
        {
            var sb = new StringBuilder(Header).Append('\n');
            if (Version != null)
                sb.Append("V\t").Append(Version).Append('\n');
            foreach (KeyValuePair<string, Entry> kv in Parts)
            {
                sb.Append("P\t").Append(kv.Key).Append('\t').Append(kv.Value.Fingerprint).Append('\t')
                    .Append(kv.Value.State.ToString().ToLowerInvariant()).Append('\n');
                foreach (string c in kv.Value.Cubes)
                    sb.Append("C\t").Append(c).Append('\n');
            }
            Directory.CreateDirectory(Path.GetDirectoryName(path));
            string tmp = path + ".tmp";
            File.WriteAllText(tmp, sb.ToString());
            if (File.Exists(path))
                File.Delete(path);
            File.Move(tmp, path);
        }

        /// The parts whose cached cubes may be reused for these fingerprints: same inputs, and seen to render
        /// the same way twice.
        public Dictionary<string, List<string>> Reusable(IDictionary<string, string> fingerprints)
        {
            var result = new Dictionary<string, List<string>>(StringComparer.Ordinal);
            foreach (KeyValuePair<string, string> kv in fingerprints)
            {
                Entry e;
                if (!kv.Value.StartsWith(Fingerprint.Ineligible, StringComparison.Ordinal)
                    && Parts.TryGetValue(kv.Key, out e) && e.Fingerprint == kv.Value && e.State == State.Stable && e.Cubes.Count > 0)
                    result[kv.Key] = e.Cubes;
            }
            return result;
        }

        /// A part's state after this boot. `previous` is its cache entry from before this boot (or null), `fresh`
        /// whether KSP rendered its cubes this boot (rather than restoring or loading them).
        public static State Next(Entry previous, string fingerprint, List<string> cubes, bool fresh)
        {
            if (previous == null || previous.Fingerprint != fingerprint)
                return State.Unverified;
            if (previous.State == State.Unstable)
                return State.Unstable; // once seen to vary, always rendered
            double worst;
            // Only an identical render counts: "within rounding" is a different render, so such parts stay rendered.
            bool same = DragCubeFile.Compare(previous.Cubes, cubes, out worst) == DragCubeFile.Match.Identical;
            if (!fresh)
                // Loaded or restored, not rendered: proves nothing new, but cubes that changed without a render
                // (another tool edited the file) are no longer the verified ones.
                return same ? previous.State : State.Unverified;
            return same ? State.Stable : State.Unstable;
        }
    }
}
