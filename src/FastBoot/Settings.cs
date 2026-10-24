using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;

namespace FastBoot
{
    /// User settings from GameData/FastBoot/FastBoot.cfg. Read directly from disk at startup, before
    /// ModuleManager or the game database exist, so the file uses a deliberately tiny "key = value" format and
    /// is parsed here rather than through ConfigNode (which FastBoot itself patches).
    internal sealed class Settings
    {
        /// Serve GameData .cfg loads through the Rust parser.
        public bool ConfigParser = true;

        /// Worker threads for pre-parsing cfg files; 0 picks a value from the CPU count.
        public int ConfigParserThreads;

        /// Write a boot timing report to PluginData/Diagnostics.
        public bool Diagnostics;

        /// Stack-sample the main thread during freezes (needs Diagnostics). Suspends the main thread briefly.
        public bool StallSampler;

        /// Read the files KSP is about to load into the OS file cache ahead of it (cold-start warm-up). The
        /// early-start loader reads this key itself, before any managed code runs.
        public bool Warmup = true;

        public static Settings Load(string path)
        {
            var s = new Settings();
            if (!File.Exists(path))
                return s;
            foreach (KeyValuePair<string, string> kv in ReadPairs(path))
            {
                switch (kv.Key)
                {
                    case "ConfigParser": s.ConfigParser = Bool(kv, s.ConfigParser); break;
                    case "ConfigParserThreads": s.ConfigParserThreads = Int(kv, s.ConfigParserThreads); break;
                    case "Diagnostics": s.Diagnostics = Bool(kv, s.Diagnostics); break;
                    case "StallSampler": s.StallSampler = Bool(kv, s.StallSampler); break;
                    case "Warmup": s.Warmup = Bool(kv, s.Warmup); break;
                    default: Log.Warning("unknown setting '" + kv.Key + "' in " + path); break;
                }
            }
            return s;
        }

        public int EffectiveThreads()
        {
            if (ConfigParserThreads > 0)
                return ConfigParserThreads;
            return Math.Max(1, Math.Min(8, Environment.ProcessorCount - 2));
        }

        public override string ToString()
        {
            return string.Format(CultureInfo.InvariantCulture, "ConfigParser={0} (threads {1}), Warmup={2}, Diagnostics={3}, StallSampler={4}",
                ConfigParser, EffectiveThreads(), Warmup, Diagnostics, StallSampler);
        }

        private static IEnumerable<KeyValuePair<string, string>> ReadPairs(string path)
        {
            foreach (string raw in File.ReadAllLines(path))
            {
                string line = raw;
                int comment = line.IndexOf("//", StringComparison.Ordinal);
                if (comment >= 0)
                    line = line.Substring(0, comment);
                int eq = line.IndexOf('=');
                if (eq <= 0)
                    continue; // braces, node name, blank lines
                yield return new KeyValuePair<string, string>(line.Substring(0, eq).Trim(), line.Substring(eq + 1).Trim());
            }
        }

        private static bool Bool(KeyValuePair<string, string> kv, bool fallback)
        {
            bool v;
            if (bool.TryParse(kv.Value, out v))
                return v;
            Log.Warning("setting " + kv.Key + " expects true or false, got '" + kv.Value + "'");
            return fallback;
        }

        private static int Int(KeyValuePair<string, string> kv, int fallback)
        {
            int v;
            if (int.TryParse(kv.Value, NumberStyles.Integer, CultureInfo.InvariantCulture, out v) && v >= 0)
                return v;
            Log.Warning("setting " + kv.Key + " expects a non-negative number, got '" + kv.Value + "'");
            return fallback;
        }
    }
}
