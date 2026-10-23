using System;
using System.IO;
using System.Reflection;
using FastBoot.ConfigParsing;
using FastBoot.Diagnostics;
using FastBoot.Native;
using UnityEngine;

namespace FastBoot
{
    /// Entry point: reads settings and starts each enabled feature. Runs with the first batch of addons, after all
    /// plugin assemblies are loaded and before KSPCommunityFixes' second config scan.
    [KSPAddon(KSPAddon.Startup.Instantly, true)]
    public sealed class FastBootAddon : MonoBehaviour
    {
        private void Awake()
        {
            DontDestroyOnLoad(gameObject);
            Log.SetSink(s => UnityEngine.Debug.Log(s));
            string home = ModFolder();
            Settings settings = Settings.Load(Path.Combine(home, "FastBoot.cfg"));
            Log.Info(Version() + " on " + NativeLibrary.Current + "; " + settings);

            if (settings.Diagnostics)
                StartDiagnostics(home, settings);
            if (settings.ConfigParser)
                StartConfigParser(home, settings);
        }

        private void StartDiagnostics(string home, Settings settings)
        {
            if (IsLoaded("BootProfiler"))
            {
                Log.Info("diagnostics skipped: the separate BootProfiler mod is installed and already does this");
                return;
            }
            BootProfiler.OutputDirectory = Path.Combine(Path.Combine(home, "PluginData"), "Diagnostics");
            BootProfiler.EnableSampler = settings.StallSampler && NativeLibrary.Current == NativeLibrary.Platform.Windows;
            gameObject.AddComponent<BootProfiler>();
        }

        private void StartConfigParser(string home, Settings settings)
        {
            if (IsLoaded("KspFastCfg"))
            {
                Log.Info("config parser skipped: the separate KspFastCfg mod is installed and already does this");
                return;
            }
            string native = Path.Combine(Path.Combine(Path.Combine(Path.Combine(home, "PluginData"), "native"),
                NativeLibrary.PlatformFolder()), NativeLibrary.FileName("kspcfg"));
            string gameData = Path.Combine(KSPUtil.ApplicationRootPath, "GameData");
            if (ConfigLoadPatch.Install(gameData, native, settings.EffectiveThreads()))
                GameEvents.OnGameDatabaseLoaded.Add(OnDatabaseLoaded);
        }

        private void OnDatabaseLoaded()
        {
            GameEvents.OnGameDatabaseLoaded.Remove(OnDatabaseLoaded);
            Log.Info("ConfigNode.Load prefixes: " + ConfigLoadPatch.PatchGraph());
            Log.Info(ConfigLoadPatch.Stats());
            ConfigLoadPatch.ReleasePrefetch();
        }

        /// GameData/FastBoot, wherever KSP is installed.
        private static string ModFolder()
        {
            return Path.GetDirectoryName(Path.GetDirectoryName(typeof(FastBootAddon).Assembly.Location));
        }

        private static bool IsLoaded(string assemblyName)
        {
            foreach (Assembly a in AppDomain.CurrentDomain.GetAssemblies())
                if (a.GetName().Name == assemblyName)
                    return true;
            return false;
        }

        private static string Version()
        {
            var v = (AssemblyInformationalVersionAttribute)Attribute.GetCustomAttribute(
                typeof(FastBootAddon).Assembly, typeof(AssemblyInformationalVersionAttribute));
            return "FastBoot " + (v == null ? "?" : v.InformationalVersion);
        }
    }
}
