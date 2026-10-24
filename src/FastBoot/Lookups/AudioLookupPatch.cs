using System;
using System.Collections.Generic;
using System.Reflection;
using HarmonyLib;
using UnityEngine;

namespace FastBoot.Lookups
{
    /// GameDatabase.GetAudioClip and ExistsAudioClip find a clip by walking every loaded clip and asking the
    /// engine for each one's name, a native call that allocates a new string. Mods that build sounds per part
    /// call them for every part: with RocketSoundEnhancement on the reference install that was 30% of all part
    /// compilation time. This serves both from a NameIndex with the same result.
    internal static class AudioLookupPatch
    {
        public const string HarmonyId = "FastBoot.Lookups";

        private static NameIndex<AudioClip> index;

        public static bool Installed { get; private set; }
        public static int Calls { get; private set; }

        public static int Rebuilds
        {
            get { return index == null ? 0 : index.Rebuilds; }
        }

        public static bool Install()
        {
            if (Installed)
                return true;
            var harmony = new Harmony(HarmonyId);
            try
            {
                NameIndex<AudioClip>.CheckRuntime();
                index = new NameIndex<AudioClip>(c => c.name, c => (UnityEngine.Object)c != null);
                harmony.Patch(Target("GetAudioClip"), new HarmonyMethod(Own("GetAudioClipPrefix")));
                harmony.Patch(Target("ExistsAudioClip"), new HarmonyMethod(Own("ExistsAudioClipPrefix")));
            }
            catch (Exception ex)
            {
                harmony.UnpatchAll(HarmonyId);
                index = null;
                Log.Warning("fast audio lookup disabled, GameDatabase unchanged: " + ex.Message);
                return false;
            }
            Installed = true;
            Log.Info("fast audio lookup installed (GameDatabase.GetAudioClip, ExistsAudioClip)");
            return true;
        }

        internal static MethodInfo Target(string name)
        {
            MethodInfo m = typeof(GameDatabase).GetMethod(name, BindingFlags.Instance | BindingFlags.Public, null, new[] { typeof(string) }, null);
            if (m == null)
                throw new MissingMethodException("GameDatabase", name);
            return m;
        }

        private static MethodInfo Own(string name)
        {
            return typeof(AudioLookupPatch).GetMethod(name, BindingFlags.Static | BindingFlags.NonPublic);
        }

        // A missing list is left to stock, which then fails exactly as it always has.
        private static bool GetAudioClipPrefix(GameDatabase __instance, string url, ref AudioClip __result)
        {
            List<AudioClip> clips = __instance.databaseAudio;
            if (clips == null)
                return true;
            Calls++;
            __result = index.Find(clips, url);
            return false;
        }

        private static bool ExistsAudioClipPrefix(GameDatabase __instance, string url, ref bool __result)
        {
            List<AudioClip> clips = __instance.databaseAudio;
            if (clips == null)
                return true;
            Calls++;
            __result = index.Find(clips, url) != null;
            return false;
        }
    }
}
