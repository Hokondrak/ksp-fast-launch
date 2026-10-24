using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.Reflection;
using HarmonyLib;
using UnityEngine;

namespace FastBoot.Terrain
{
    /// Takes the pre-building of KSP's terrain quad pool off the loading screen.
    ///
    /// During planet setup, PQSCache.CreateCache builds `cachePQInitialCount` (9,216) terrain quads, each a GameObject
    /// with three components and a mesh, synchronously: about 5 s of the boot on the reference install. They are a
    /// pool for later scenes; nothing takes a quad while loading (KSP logs "PQSCache: Increased PQ Cache" when the
    /// pool runs dry, and no boot did). And when the pool is empty, GetQuad grows it on demand, 256 at a time.
    ///
    /// So during the loading screen this does exactly what CreateCache does except the pre-building: create the
    /// shared quad arrays, an empty pool, mark the cache ready and fire onCacheReady. From the main menu on, the same
    /// number of quads is built with KSP's own IncreasePQCache, a few milliseconds per frame, until the pool is as big
    /// as KSP would have made it. Only Assembly-CSharp touches PQSCache (checked across every installed plugin).
    internal static class PqsCacheDeferral
    {
        public const string HarmonyId = "FastBoot.Terrain";
        private const BindingFlags Instance = BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic;

        private static readonly FieldInfo InitialCount = typeof(PQSCache).GetField("cachePQInitialCount", Instance);
        private static readonly FieldInfo TotalCount = typeof(PQSCache).GetField("cachePQTotalCount", Instance);
        private static readonly FieldInfo Unassigned = typeof(PQSCache).GetField("cachePQUnassigned", Instance);
        private static readonly FieldInfo AssignedCount = typeof(PQSCache).GetField("cachePQAssignedCount", Instance);
        private static readonly FieldInfo Ready = typeof(PQSCache).GetField("cacheReady", Instance);
        private static readonly FieldInfo OnReady = typeof(PQSCache).GetField("onCacheReady", Instance);
        private static readonly FieldInfo CacheMesh = typeof(PQS).GetField("cacheMesh", BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic);
        private static readonly MethodInfo CreateSharedArrays = typeof(PQS).GetMethod("CreateCache", BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic, null, Type.EmptyTypes, null);
        private static readonly MethodInfo Increase = typeof(PQSCache).GetMethod("IncreasePQCache", Instance, null, new[] { typeof(int) }, null);

        private static PQSCache deferredCache;
        private static int target;

        public static bool Installed { get; private set; }

        public static bool Install()
        {
            if (Installed)
                return true;
            var harmony = new Harmony(HarmonyId);
            try
            {
                if (InitialCount == null || TotalCount == null || Unassigned == null || AssignedCount == null || Ready == null || OnReady == null
                    || CacheMesh == null || CreateSharedArrays == null || Increase == null)
                    throw new MissingMemberException("PQSCache or PQS no longer has the members this relies on");
                harmony.Patch(typeof(PQSCache).GetMethod("CreateCache", Instance, null, Type.EmptyTypes, null),
                    new HarmonyMethod(typeof(PqsCacheDeferral).GetMethod("CreateCachePrefix", BindingFlags.Static | BindingFlags.NonPublic)));
            }
            catch (Exception ex)
            {
                harmony.UnpatchAll(HarmonyId);
                Log.Warning("terrain cache deferral disabled, KSP unchanged: " + ex.Message);
                return false;
            }
            Installed = true;
            return true;
        }

        // Prefix on PQSCache.CreateCache. Outside the loading screen, or once done, stock runs unchanged.
        private static bool CreateCachePrefix(PQSCache __instance)
        {
            if (deferredCache != null || HighLogic.LoadedScene != GameScenes.LOADING)
                return true;
            try
            {
                if (CacheMesh.GetValue(null) as UnityEngine.Object == null)
                    CreateSharedArrays.Invoke(null, null);
                int count = (int)InitialCount.GetValue(__instance);
                Unassigned.SetValue(__instance, Activator.CreateInstance(Unassigned.FieldType, count));
                AssignedCount.SetValue(__instance, 0);
                Ready.SetValue(__instance, true);
                var onReady = OnReady.GetValue(__instance) as Delegate;
                if (onReady != null)
                    onReady.DynamicInvoke();
                OnReady.SetValue(__instance, null);
                deferredCache = __instance;
                target = count;
                Log.Info("terrain cache: pre-building " + count + " quads after the loading screen instead of now");
                return false;
            }
            catch (Exception ex)
            {
                Log.Warning("terrain cache deferral failed, KSP builds the cache now as usual: " + ex.Message);
                return true;
            }
        }

        /// From the main menu: builds the deferred quads in time slices on `host`.
        public static void StartPrewarm(MonoBehaviour host)
        {
            if (deferredCache != null)
                host.StartCoroutine(Prewarm(deferredCache));
        }

        private static IEnumerator Prewarm(PQSCache cache)
        {
            var increase = (Action<int>)Delegate.CreateDelegate(typeof(Action<int>), cache, Increase);
            var total = Stopwatch.StartNew();
            var slice = new Stopwatch();
            double busy = 0;
            int frames = 0;
            const int Batch = 8;
            while (cache != null && (int)TotalCount.GetValue(cache) < target)
            {
                // Generous at the main menu, gentle anywhere the player is actually playing.
                double budgetMs = HighLogic.LoadedScene == GameScenes.MAINMENU || HighLogic.LoadedScene == GameScenes.LOADING ? 6 : 2;
                slice.Reset();
                slice.Start();
                do
                {
                    increase(Math.Min(Batch, target - (int)TotalCount.GetValue(cache)));
                }
                while (slice.Elapsed.TotalMilliseconds < budgetMs && (int)TotalCount.GetValue(cache) < target);
                busy += slice.Elapsed.TotalMilliseconds;
                frames++;
                yield return null;
            }
            if (cache != null)
                Log.Info(string.Format(CultureInfo.InvariantCulture, "terrain cache: {0} quads pre-built in {1:F0} ms of work over {2} frames ({3:F1} s)",
                    TotalCount.GetValue(cache), busy, frames, total.Elapsed.TotalSeconds));
        }
    }
}
