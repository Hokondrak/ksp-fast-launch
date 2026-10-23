using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Text;
using FastBoot.ConfigParsing;

namespace FastBoot.Tests
{
    /// kspcfg against stock on every cfg file in the install (tokens and trees) and on random documents.
    /// Fails on any disagreement. Timings are informative only.
    internal static class DiffTest
    {
        public static int Run(Options o)
        {
            KspCfg k = LoadKspCfg(o);
            if (k == null)
                return 1;
            string[] all = Directory.GetFiles(o.GameData, "*.cfg", SearchOption.AllDirectories);
            int identical = 0, fallback = 0, empty = 0, tokenMismatch = 0, treeMismatch = 0, stockSelfMismatch = 0;
            var nonEmpty = new List<string>();
            foreach (string f in all)
            {
                string[] raw = File.ReadAllLines(f);
                if (raw.Length == 0)
                {
                    empty++; // stock logs through Unity here; kspcfg reports an empty file and defers to stock
                    if (k.FromFile(f) != null)
                        tokenMismatch++;
                    continue;
                }
                nonEmpty.Add(f);
                List<string[]> stock = StockParser.PreFormat(raw);
                List<string[]> rust = k.FromFile(f);
                if (rust == null)
                {
                    fallback++;
                    continue;
                }
                string d = StockParser.DiffTokens(stock, rust);
                if (d != null)
                {
                    if (tokenMismatch++ < 5) Console.WriteLine("TOKEN MISMATCH " + Rel(o, f) + ": " + d);
                    continue;
                }
                string t = StockParser.DiffTrees(StockParser.Tree(stock), StockParser.Tree(rust));
                if (t != null)
                {
                    if (treeMismatch++ < 5) Console.WriteLine("TREE MISMATCH " + Rel(o, f) + ": " + t);
                    continue;
                }
                identical++;
                ConfigNode full = ConfigNode.Load(f);
                if (StockParser.DiffTrees(full, StockParser.Tree(stock)) != null)
                    stockSelfMismatch++;
            }
            Console.WriteLine("files {0}: identical {1}, stock fallback {2}, empty {3}, token mismatches {4}, tree mismatches {5}, stock self-check {6}",
                all.Length, identical, fallback, empty, tokenMismatch, treeMismatch, stockSelfMismatch);

            int fuzzMismatch = Fuzz(k, o.Cases);
            Time(k, nonEmpty.ToArray());
            return tokenMismatch + treeMismatch + stockSelfMismatch + fuzzMismatch == 0 ? 0 : 1;
        }

        public static KspCfg LoadKspCfg(Options o)
        {
            string error;
            KspCfg k = KspCfg.Load(o.KspCfgPath, out error);
            if (k == null)
                Console.WriteLine("cannot load kspcfg: " + error);
            return k;
        }

        private static int Fuzz(KspCfg k, int cases)
        {
            int same = 0, fallback = 0, mismatch = 0;
            var rng = new Random(12345);
            const string alphabet = "ab  \t{}{}==//xé ж　 ;\"#";
            string[] newlines = { "\n", "\r\n", "\r" };
            for (int c = 0; c < cases; c++)
            {
                var sb = new StringBuilder();
                int lines = rng.Next(0, 6);
                for (int l = 0; l < lines; l++)
                {
                    int len = rng.Next(0, 14);
                    for (int i = 0; i < len; i++) sb.Append(alphabet[rng.Next(alphabet.Length)]);
                    if (l + 1 < lines || rng.Next(2) == 0) sb.Append(newlines[rng.Next(3)]);
                }
                byte[] body = Encoding.UTF8.GetBytes(sb.ToString());
                byte[] bytes = rng.Next(4) == 0 ? new byte[] { 0xEF, 0xBB, 0xBF }.Concat(body).ToArray() : body;
                var read = new List<string>();
                using (var r = new StreamReader(new MemoryStream(bytes), Encoding.UTF8, true))
                {
                    string line;
                    while ((line = r.ReadLine()) != null) read.Add(line);
                }
                List<string[]> rust = k.FromBytes(bytes);
                if (read.Count == 0)
                {
                    if (rust == null) same++; else mismatch++;
                    continue;
                }
                if (rust == null)
                {
                    fallback++;
                    continue;
                }
                string d = StockParser.DiffTokens(StockParser.PreFormat(read.ToArray()), rust);
                if (d == null) same++;
                else if (mismatch++ < 5) Console.WriteLine("FUZZ MISMATCH " + d);
            }
            Console.WriteLine("fuzz {0}: identical {1}, fallback {2}, mismatches {3}", cases, same, fallback, mismatch);
            return mismatch;
        }

        private static void Time(KspCfg k, string[] files)
        {
            var sw = Stopwatch.StartNew();
            foreach (string f in files) ConfigNode.Load(f);
            double stock = sw.Elapsed.TotalSeconds;
            sw.Restart();
            using (KspCfg.Batch b = k.ParseMany(files, 4))
            {
                for (int i = 0; i < files.Length; i++)
                {
                    List<string[]> l = b.Lines(i);
                    if (l == null) ConfigNode.Load(files[i]);
                    else StockParser.Tree(l);
                }
            }
            Console.WriteLine("timing (informative; fallback files parsed by stock): stock {0:F2}s, kspcfg 4 threads + stock trees {1:F2}s",
                stock, sw.Elapsed.TotalSeconds);
        }

        private static string Rel(Options o, string f)
        {
            return f.Substring(o.GameData.Length);
        }
    }
}
