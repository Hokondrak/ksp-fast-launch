using System;
using System.IO;
using System.Linq;
using System.Reflection;
using FastBoot.ConfigParsing;
using FastBoot.Diagnostics;
using FastBoot.Native;
using UnityEngine;

namespace FastBoot
{
    /// Entry point: reads settings and starts each enabled feature. Runs with the first batch of addons, after all
    /// plugin assemblies are loaded and before KSPCommunityFixes' second config scan. When the optional early-start
    /// loader is installed, the config parser was already started by StartEarly, before KSP's first scan.
    [KSPAddon(KSPAddon.Startup.Instantly, true)]
    public sealed class FastBootAddon : MonoBehaviour
    {
        private void Awake()
        {
            DontDestroyOnLoad(gameObject);
            Log.SetSink(s => UnityEngine.Debug.Log(s));
            string home = ModFolder();
            Settings settings = Settings.Load(Path.Combine(home, "FastBoot.cfg"));
            Log.Info(Version() + " on " + NativeLibrary.Current + "; " + settings
                + (Early.Bootstrap.Started ? "; early start active" : ""));

            // First: the warm-up is only useful while there is still loading ahead of it.
            if (settings.Warmup)
                ColdStart.Warmup.Start(KSPUtil.ApplicationRootPath, KspCfgPath(home),
                    Path.Combine(Path.Combine(home, "PluginData"), "readlist.txt"));
            if (settings.Diagnostics)
                StartDiagnostics(home, settings);
            if (settings.Profile.Length > 0)
                StartProfiler(home, settings.Profile);
            PartDatabaseWatch.Note(KSPUtil.ApplicationRootPath, "when FastBoot's addon started");
            if (settings.FastLookups)
                Lookups.AudioLookupPatch.Install();
            if (settings.ConfigParser)
            {
                if (ConfigLoadPatch.Installed)
                    Log.Info("config parser already running since early start");
                else if (IsLoaded("KspFastCfg"))
                    Log.Info("config parser skipped: the separate KspFastCfg mod is installed and already does this");
                else
                    StartConfigParser(home, Path.Combine(KSPUtil.ApplicationRootPath, "GameData"), settings);
                if (ConfigLoadPatch.Installed)
                    GameEvents.OnGameDatabaseLoaded.Add(OnDatabaseLoaded);
            }
        }

        /// Called by Early.Bootstrap as soon as Assembly-CSharp is loaded, before the engine runs any game code.
        /// Must not call into Unity: only file access and Harmony.
        internal static void StartEarly(string kspRoot)
        {
            string gameData = Path.Combine(kspRoot, "GameData");
            string home = ModFolder();
            Settings settings = Settings.Load(Path.Combine(home, "FastBoot.cfg"));
            PartDatabaseWatch.Enabled = settings.Profile.Length > 0;
            PartDatabaseWatch.Note(kspRoot, "at early start");
            if (!settings.ConfigParser)
                return;
            // KspFastCfg's assembly is not loaded yet, so look for it on disk; Awake reports the skip.
            if (File.Exists(Path.Combine(Path.Combine(Path.Combine(gameData, "KspFastCfg"), "Plugins"), "KspFastCfg.dll")))
                return;
            StartConfigParser(home, gameData, settings);
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

        /// ModuleManager calls this on every loaded mod once it has applied all patches, just before KSP's
        /// PartLoader reads PartDatabase.cfg.
        public static void ModuleManagerPostLoad()
        {
            PartDatabaseWatch.Note(KSPUtil.ApplicationRootPath, "after ModuleManager's patching");
        }

        private void StartProfiler(string home, string name)
        {
            PartDatabaseWatch.Enabled = true;
            PhaseProfiler.Window window = PhaseProfiler.Find(name);
            if (window == null || NativeLibrary.Current != NativeLibrary.Platform.Windows)
            {
                Log.Warning("profiler not started: " + (window == null
                    ? "unknown Profile '" + name + "'; known: " + string.Join(", ", PhaseProfiler.Windows.Select(w => w.Name).ToArray())
                    : "it needs Windows"));
                return;
            }
            PhaseProfiler.OutputDirectory = Path.Combine(Path.Combine(home, "PluginData"), "Diagnostics");
            PhaseProfiler.Selected = window;
            gameObject.AddComponent<PhaseProfiler>();
        }

        private static void StartConfigParser(string home, string gameData, Settings settings)
        {
            ConfigLoadPatch.Install(gameData, KspCfgPath(home), settings.EffectiveThreads());
        }

        /// This platform's build of FastBoot's native library, under PluginData/native.
        private static string KspCfgPath(string home)
        {
            return Path.Combine(Path.Combine(Path.Combine(Path.Combine(home, "PluginData"), "native"),
                NativeLibrary.PlatformFolder()), NativeLibrary.FileName("kspcfg"));
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
