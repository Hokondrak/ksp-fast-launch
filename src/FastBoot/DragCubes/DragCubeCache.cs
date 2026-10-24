using System;
using System.Collections.Generic;
using System.IO;
using System.Text;

namespace FastBoot.DragCubes
{
    /// FastBoot's own copy of every part's drag cubes, with the fingerprint of the inputs they were rendered from.
    /// Kept in PluginData (which GameDatabase never loads) as plain text:
    ///
    ///     P &lt;tab&gt; url &lt;tab&gt; fingerprint
    ///     C &lt;tab&gt; cube line as in PartDatabase.cfg
    internal sealed class DragCubeCache
    {
        private const string Header = "# FastBoot drag cube cache v1";

        public sealed class Entry
        {
            public string Fingerprint;
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
            if (lines.Length == 0 || lines[0] != Header)
                return cache; // unknown format: start over
            Entry current = null;
            foreach (string line in lines)
            {
                string[] f = line.Split('\t');
                if (f[0] == "V" && f.Length == 2)
                    cache.Version = f[1];
                else if (f[0] == "P" && f.Length == 3)
                    cache.Parts[f[1]] = current = new Entry { Fingerprint = f[2] };
                else if (f[0] == "C" && f.Length == 2 && current != null)
                    current.Cubes.Add(f[1]);
            }
            return cache;
        }

        public void Save(string path)
        {
            var sb = new StringBuilder(Header).Append('\n');
            if (Version != null)
                sb.Append("V\t").Append(Version).Append('\n');
            foreach (KeyValuePair<string, Entry> kv in Parts)
            {
                sb.Append("P\t").Append(kv.Key).Append('\t').Append(kv.Value.Fingerprint).Append('\n');
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
    }
}
