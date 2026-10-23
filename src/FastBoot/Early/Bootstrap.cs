using System;
using System.IO;
using System.Reflection;
using System.Runtime.CompilerServices;

namespace FastBoot.Early
{
    /// Entry point for the optional early-start loader (native/early, shipped as version.dll in the KSP folder).
    /// The loader calls Start right after Unity initialises Mono, before any game assembly is loaded, so that the
    /// config parser is in place for KSP's own first config scan, which runs before any GameData plugin exists.
    ///
    /// Nothing here may touch Unity or game types until Assembly-CSharp has loaded: the engine is not running yet.
    /// FastBoot.dll itself is loaded from GameData/FastBoot/Plugins, the same path KSP's AssemblyLoader uses later,
    /// so KSP gets this same assembly (with its state) when it loads plugins.
    public static class Bootstrap
    {
        private static string kspRoot;

        /// True once the native loader has called Start in this process.
        public static bool Started { get; private set; }

        /// Called by the native loader with the KSP folder. Never throws: a failure only means no early start.
        public static void Start(string root)
        {
            try
            {
                if (Started)
                    return;
                Started = true;
                kspRoot = root;
                AppDomain.CurrentDomain.AssemblyResolve += ResolveHarmony;
                AppDomain.CurrentDomain.AssemblyLoad += OnAssemblyLoad;
                Log.Info("early start: waiting for Assembly-CSharp");
                foreach (Assembly a in AppDomain.CurrentDomain.GetAssemblies())
                    if (a.GetName().Name == "Assembly-CSharp")
                        GameAssemblyLoaded();
            }
            catch (Exception ex)
            {
                Log.Warning("early start failed: " + ex);
            }
        }

        private static void OnAssemblyLoad(object sender, AssemblyLoadEventArgs e)
        {
            if (e.LoadedAssembly.GetName().Name != "Assembly-CSharp")
                return;
            AppDomain.CurrentDomain.AssemblyLoad -= OnAssemblyLoad;
            GameAssemblyLoaded();
        }

        // Kept out of OnAssemblyLoad so that method compiles without referencing game types.
        [MethodImpl(MethodImplOptions.NoInlining)]
        private static void GameAssemblyLoaded()
        {
            try
            {
                FastBootAddon.StartEarly(kspRoot);
            }
            catch (Exception ex)
            {
                Log.Warning("early config parser not installed: " + ex);
            }
        }

        // Harmony lives in GameData/000_Harmony and is not on Mono's search path this early. Loading it by that
        // path is what KSP's AssemblyLoader does later, so both end up with the same assembly.
        private static Assembly ResolveHarmony(object sender, ResolveEventArgs e)
        {
            if (new AssemblyName(e.Name).Name != "0Harmony" || kspRoot == null)
                return null;
            string path = Path.Combine(Path.Combine(Path.Combine(kspRoot, "GameData"), "000_Harmony"), "0Harmony.dll");
            return File.Exists(path) ? Assembly.LoadFrom(path) : null;
        }
    }
}
