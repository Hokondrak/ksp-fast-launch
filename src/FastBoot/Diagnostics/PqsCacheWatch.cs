using System;
using System.Diagnostics;
using System.Globalization;
using System.Reflection;
using HarmonyLib;

namespace FastBoot.Diagnostics
{
    /// Measures KSP's terrain quad cache (PQSCache): how long its up-front creation blocks the main thread during
    /// planet setup, how many quads it pre-builds and how many are actually in use later. The pre-built quads are a
    /// pool for flight; if few are used by the main menu, building them later would take the time off the boot.
    /// Only active while profiling.
    internal static class PqsCacheWatch
    {
        private const string HarmonyId = "FastBoot.PqsCacheWatch";
        private static readonly Stopwatch Clock = new Stopwatch();
        private static int calls;

        public static void Install()
        {
            try
            {
                MethodInfo create = typeof(PQSCache).GetMethod("CreateCache", BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic);
                var own = BindingFlags.Static | BindingFlags.NonPublic;
                new Harmony(HarmonyId).Patch(create, new HarmonyMethod(typeof(PqsCacheWatch).GetMethod("Before", own)),
                    new HarmonyMethod(typeof(PqsCacheWatch).GetMethod("After", own)));
            }
            catch (Exception ex)
            {
                Log.Warning("PQS cache watch not installed: " + ex.Message);
            }
        }

        private static void Before()
        {
            calls++;
            Clock.Start();
        }

        private static void After()
        {
            Clock.Stop();
            Log.Info(string.Format(CultureInfo.InvariantCulture, "PQS cache: CreateCache call {0} took {1:F0} ms", calls, Clock.Elapsed.TotalMilliseconds));
        }

        /// The cache's counters, by reflection so field visibility does not matter.
        public static void Note(string when)
        {
            PQSCache cache = PQSCache.Instance;
            if (cache == null)
            {
                Log.Info("PQS cache " + when + ": none");
                return;
            }
            var sb = new System.Text.StringBuilder("PQS cache " + when + ":");
            foreach (string f in new[] { "cachePQInitialCount", "cachePQIncreasePerFrame", "cachePQTotalCount", "cachePQAssignedCount", "cachePQUnassignedCount", "cachePQCoroutine", "cacheReady" })
            {
                FieldInfo fi = typeof(PQSCache).GetField(f, BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic);
                sb.Append(' ').Append(f).Append('=').Append(fi == null ? "?" : Convert.ToString(fi.GetValue(cache), CultureInfo.InvariantCulture));
            }
            sb.Append(string.Format(CultureInfo.InvariantCulture, "; CreateCache total {0:F0} ms in {1} call(s)", Clock.Elapsed.TotalMilliseconds, calls));
            Log.Info(sb.ToString());
        }
    }
}
