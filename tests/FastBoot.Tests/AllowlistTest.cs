using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Text;
using System.Threading;
using FastBoot.ConfigParsing;

namespace FastBoot.Tests
{
    /// Every BMP code point kspcfg accepts on its fast path, placed in every position where culture-sensitive
    /// IndexOf of "//", "{" or "}" could differ from an ordinal search, compared with stock PreFormatConfig under
    /// ru-RU, en-US and the invariant culture. Desktop .NET's collation tables are a proxy for Mono's.
    internal static class AllowlistTest
    {
        private static readonly string[] Contexts =
        {
            "x /C/ y", "x C// y", "x //C y", "x /C y", "x C/ y", "a}Cb", "aC}b", "a{Cb", "aC{b", "C}", "}C", "C{", "{C",
            "k=C", "C=v", "kC=v", " C ", "C", "a=b/C/c", "N C { k = v }", "/C/", "C//", "//C", "{C}", "}C{",
        };

        public static int Run(Options o)
        {
            KspCfg k = DiffTest.LoadKspCfg(o);
            if (k == null)
                return 1;
            int failures = 0;
            var allBad = new SortedSet<int>();
            foreach (string culture in new[] { "ru-RU", "en-US", "" })
            {
                Thread.CurrentThread.CurrentCulture = culture == "" ? CultureInfo.InvariantCulture : new CultureInfo(culture);
                int accepted = 0;
                var bad = new List<string>();
                for (int cp = 1; cp <= 0xFFFF; cp++)
                {
                    if (cp == '\n' || cp == '\r' || (cp >= 0xD800 && cp <= 0xDFFF))
                        continue;
                    string c = ((char)cp).ToString();
                    if (k.FromBytes(Encoding.UTF8.GetBytes("x" + c + "y")) == null)
                        continue; // not on the fast path: stock handles it
                    accepted++;
                    string doc = string.Join("\n", Contexts.Select(t => t.Replace("C", c)).ToArray());
                    List<string[]> rust = k.FromBytes(Encoding.UTF8.GetBytes(doc));
                    if (rust != null && StockParser.DiffTokens(StockParser.PreFormat(doc.Split('\n')), rust) != null)
                    {
                        bad.Add(string.Format("U+{0:X4}", cp));
                        allBad.Add(cp);
                    }
                }
                failures += bad.Count;
                Console.WriteLine("culture {0}: {1} code points on the fast path, {2} disagree with stock{3}",
                    culture == "" ? "invariant" : culture, accepted, bad.Count,
                    bad.Count > 0 ? ": " + string.Join(" ", bad.Take(40).ToArray()) : "");
            }
            if (o.Out != null)
            {
                // One hex code point per line; tools/gen-fastpath-exclusions.py turns this into a Rust table.
                File.WriteAllLines(o.Out, allBad.Select(cp => cp.ToString("X4")).ToArray());
                Console.WriteLine("wrote {0} disagreeing code points to {1}", allBad.Count, o.Out);
            }
            return failures == 0 ? 0 : 1;
        }
    }
}
