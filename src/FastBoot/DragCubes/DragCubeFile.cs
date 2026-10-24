using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;

namespace FastBoot.DragCubes
{
    /// KSP's PartDatabase.cfg: the drag cubes of every part, rendered once and reused until ModuleManager deletes the
    /// file after a mod change. Each PART has a url (config file url plus part name) and DRAG_CUBE entries: either
    /// `cube = Name, 24 numbers` or `procedural = True`.
    internal sealed class DragCubeFile
    {
        /// The `version = ...` line of the PART_DATABASE header, as KSP wrote it.
        public string Version;

        /// Part url -> its cube lines ("cube = ..." / "procedural = True"), in file order.
        public readonly Dictionary<string, List<string>> Parts = new Dictionary<string, List<string>>(StringComparer.Ordinal);

        /// Part urls in file order.
        public readonly List<string> Order = new List<string>();

        public static DragCubeFile Parse(string text)
        {
            var file = new DragCubeFile();
            List<string> current = null;
            foreach (string raw in text.Split('\n'))
            {
                string line = raw.Trim();
                int eq = line.IndexOf('=');
                if (eq <= 0)
                    continue;
                string key = line.Substring(0, eq).Trim();
                string value = line.Substring(eq + 1).Trim();
                switch (key)
                {
                    case "version":
                        if (file.Version == null)
                            file.Version = value;
                        break;
                    case "url":
                        current = new List<string>();
                        if (!file.Parts.ContainsKey(value))
                            file.Order.Add(value);
                        file.Parts[value] = current;
                        break;
                    case "cube":
                    case "procedural":
                        if (current != null)
                            current.Add(key + " = " + value);
                        break;
                }
            }
            return file;
        }

        /// PartDatabase.cfg text exactly as KSP writes it (ConfigNode.Save: tabs, CRLF, no byte order mark).
        public static string Write(string version, IEnumerable<KeyValuePair<string, List<string>>> parts)
        {
            var sb = new System.Text.StringBuilder();
            const string nl = "\r\n";
            sb.Append("PART_DATABASE").Append(nl).Append('{').Append(nl).Append("\tversion = ").Append(version).Append(nl).Append('}').Append(nl);
            foreach (KeyValuePair<string, List<string>> part in parts)
            {
                sb.Append("PART").Append(nl).Append('{').Append(nl).Append("\turl = ").Append(part.Key).Append(nl);
                sb.Append("\tDRAG_CUBE").Append(nl).Append("\t{").Append(nl);
                foreach (string cube in part.Value)
                    sb.Append("\t\t").Append(cube).Append(nl);
                sb.Append("\t}").Append(nl).Append('}').Append(nl);
            }
            return sb.ToString();
        }

        /// How two sets of cubes for one part relate.
        public enum Match
        {
            Identical,
            WithinTolerance,
            Different,
        }

        /// Compares cube lists by cube name (order-independent). Numbers are as KSP prints them, four significant
        /// digits, so "within tolerance" allows a difference in the last printed digit.
        public static Match Compare(List<string> a, List<string> b, out double worst)
        {
            worst = 0;
            if (a.Count != b.Count)
            {
                worst = double.PositiveInfinity;
                return Match.Different;
            }
            if (a.SequenceEqual(b))
                return Match.Identical;
            var remaining = b.ToList();
            foreach (string cube in a)
            {
                string name = CubeName(cube);
                int j = remaining.FindIndex(x => CubeName(x) == name);
                if (j < 0)
                {
                    worst = double.PositiveInfinity;
                    return Match.Different;
                }
                double d = Distance(cube, remaining[j]);
                remaining.RemoveAt(j);
                worst = Math.Max(worst, d);
            }
            if (worst == 0)
                return Match.Identical; // same cubes, listed in another order
            return worst <= 1.0 ? Match.WithinTolerance : Match.Different;
        }

        private static string CubeName(string line)
        {
            int comma = line.IndexOf(',');
            return comma < 0 ? line : line.Substring(0, comma);
        }

        /// Largest difference between corresponding numbers, in units of the last printed digit of the larger one
        /// (so 1.0 is "one step of rounding"). Infinity if they do not even have the same shape.
        internal static double Distance(string a, string b)
        {
            double[] x = Numbers(a), y = Numbers(b);
            if (x == null || y == null || x.Length != y.Length)
                return a == b ? 0 : double.PositiveInfinity;
            double worst = 0;
            for (int i = 0; i < x.Length; i++)
            {
                double diff = Math.Abs(x[i] - y[i]);
                if (diff == 0)
                    continue;
                double scale = Math.Max(Math.Abs(x[i]), Math.Abs(y[i]));
                // Four significant digits: one step is 10^(floor(log10(scale)) - 3). Tiny values (float noise
                // around zero, printed like 1.192E-07) count as zero to within 1e-5.
                double step = scale < 1e-5 ? 1e-5 : Math.Pow(10, Math.Floor(Math.Log10(scale)) - 3);
                worst = Math.Max(worst, diff / step);
            }
            return worst;
        }

        private static double[] Numbers(string line)
        {
            int comma = line.IndexOf(',');
            if (comma < 0)
                return null;
            string[] parts = line.Substring(comma + 1).Split(',');
            var values = new double[parts.Length];
            for (int i = 0; i < parts.Length; i++)
                if (!double.TryParse(parts[i].Trim(), NumberStyles.Float, CultureInfo.InvariantCulture, out values[i]))
                    return null;
            return values;
        }
    }
}
