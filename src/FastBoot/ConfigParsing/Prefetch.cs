using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading;

namespace FastBoot.ConfigParsing
{
    /// Pre-parses every GameData .cfg on worker threads so the main thread only builds trees. An entry is served
    /// for as long as the file's size and timestamp still match what was parsed, so KSP's first config scan and
    /// KSPCommunityFixes' second one both use it; anything else falls through to a direct parse. Each serve is a
    /// fresh copy, so callers never share arrays.
    internal sealed class Prefetch
    {
        private readonly ManualResetEvent done = new ManualResetEvent(false);
        private readonly object gate = new object();
        private KspCfg.Batch batch;
        private Dictionary<string, int> index;
        private long[] lengths;
        private long[] writeTicks;

        public readonly Stopwatch Elapsed = new Stopwatch();

        public static Prefetch Start(KspCfg kspcfg, string gameDataDir, int threads)
        {
            var p = new Prefetch();
            var t = new Thread(() => p.Run(kspcfg, gameDataDir, threads)) { IsBackground = true, Name = "FastBoot prefetch" };
            t.Start();
            return p;
        }

        private void Run(KspCfg kspcfg, string gameDataDir, int threads)
        {
            try
            {
                Elapsed.Start();
                // GameDatabase never loads PluginData folders, so neither does the prefetch.
                string[] files = Directory.GetFiles(gameDataDir, "*.cfg", SearchOption.AllDirectories).Where(f => !InPluginData(f, gameDataDir)).ToArray();
                var idx = new Dictionary<string, int>(files.Length);
                var len = new long[files.Length];
                var ticks = new long[files.Length];
                for (int i = 0; i < files.Length; i++)
                {
                    idx[PathKey.Of(files[i])] = i;
                    var fi = new FileInfo(files[i]);
                    len[i] = fi.Length;
                    ticks[i] = fi.LastWriteTimeUtc.Ticks;
                }
                KspCfg.Batch b = kspcfg.ParseMany(files, threads);
                if (b == null)
                    throw new InvalidOperationException("kspcfg returned no batch");
                lock (gate)
                {
                    batch = b;
                    index = idx;
                    lengths = len;
                    writeTicks = ticks;
                }
            }
            catch (Exception ex)
            {
                Log.Warning("config prefetch failed, files will be parsed one at a time: " + ex.Message);
            }
            finally
            {
                Elapsed.Stop();
                done.Set();
            }
        }

        private static bool InPluginData(string file, string gameDataDir)
        {
            string rel = file.Substring(Math.Min(file.Length, gameDataDir.Length));
            return rel.Split('/', '\\').Any(seg => string.Equals(seg, "PluginData", StringComparison.OrdinalIgnoreCase));
        }

        /// The pre-parsed lines for `path` if it was prefetched and is unchanged since; otherwise false.
        /// `lines` may be null on success when kspcfg declined the file (the caller then uses the stock parser).
        public bool TryTake(string path, string key, out List<string[]> lines)
        {
            lines = null;
            done.WaitOne();
            lock (gate)
            {
                int i;
                if (batch == null || !index.TryGetValue(key, out i))
                    return false;
                var fi = new FileInfo(path);
                if (!fi.Exists || fi.Length != lengths[i] || fi.LastWriteTimeUtc.Ticks != writeTicks[i])
                    return false;
                lines = batch.Lines(i);
                return true;
            }
        }

        public void Release()
        {
            done.WaitOne();
            lock (gate)
            {
                if (batch != null)
                    batch.Dispose();
                batch = null;
                index = null;
            }
        }
    }

    internal static class PathKey
    {
        /// Case-insensitive, separator-normalised full path, matching how KSP builds UrlFile paths on Windows.
        public static string Of(string path)
        {
            return Path.GetFullPath(path).Replace('/', '\\').ToUpperInvariant();
        }
    }
}
