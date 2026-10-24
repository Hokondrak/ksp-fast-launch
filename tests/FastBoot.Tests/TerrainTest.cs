using System;
using System.Linq;
using FastBoot.Terrain;
using HarmonyLib;

namespace FastBoot.Tests
{
    /// The terrain cache deferral without the engine: every PQSCache/PQS member it relies on exists in this KSP
    /// build, and its prefix installs on PQSCache.CreateCache. What it does with real quads needs the game.
    internal static class TerrainTest
    {
        public static int Run(Options o)
        {
            bool installed = PqsCacheDeferral.Install();
            var info = Harmony.GetPatchInfo(typeof(PQSCache).GetMethod("CreateCache", Type.EmptyTypes));
            bool patched = info != null && info.Prefixes.Any(p => p.owner == PqsCacheDeferral.HarmonyId);
            Console.WriteLine("terrain cache deferral: {0}, prefix on PQSCache.CreateCache {1}", installed ? "every member found" : "MEMBERS MISSING",
                patched ? "installed" : "MISSING");
            return installed && patched ? 0 : 1;
        }
    }
}
