// Offline test runner for FastBoot, on desktop .NET Framework against the installed KSP's own assemblies.
// KSP is never started. Each command runs in its own process because Harmony patches are process-wide.
//
//   FastBoot.Tests.exe <command> --ksp <KSP folder> --dist <build output folder> [--cases N]
//
// Commands: diff, allowlist, config-patch-kspcf-first, config-patch-fastboot-first, config-patch-standalone,
//           diagnostics, early-chain, early-chain-off, early-chain-foreign (these three only under monohost
//           --preload version.dll)
using System;
using System.Collections.Generic;
using System.IO;
using System.Reflection;

namespace FastBoot.Tests
{
    internal static class Program
    {
        private static readonly List<string> SearchDirs = new List<string>();

        private static int Main(string[] args)
        {
            if (args.Length == 0)
                return Usage();
            var opts = Options.Parse(args);
            if (opts == null)
                return Usage();
            // Tests may run while the machine is in use (games included); never compete for CPU.
            try
            {
                System.Diagnostics.Process.GetCurrentProcess().PriorityClass = System.Diagnostics.ProcessPriorityClass.BelowNormal;
            }
            catch (Exception)
            {
                // Not supported under every runtime (e.g. some Mono builds); the tests still run.
            }
            Console.WriteLine("runtime: " + RuntimeName() + ", culture " + System.Globalization.CultureInfo.CurrentCulture.Name);
            SearchDirs.Add(opts.Managed);
            SearchDirs.Add(Path.Combine(opts.GameData, "000_Harmony"));
            SearchDirs.Add(Path.Combine(opts.GameData, Path.Combine("KSPCommunityFixes", "Plugins")));
            SearchDirs.Add(opts.FastBootPlugins);
            AppDomain.CurrentDomain.AssemblyResolve += Resolve;
            try
            {
                return Dispatch(args[0], opts);
            }
            catch (Exception ex)
            {
                Console.WriteLine("ERROR " + ex);
                return 3;
            }
        }

        // Separate from Main so KSP types resolve only after the resolver is registered.
        private static int Dispatch(string command, Options opts)
        {
            Log.SetSink(Console.WriteLine);
            switch (command)
            {
                case "diff": return DiffTest.Run(opts);
                case "allowlist": return AllowlistTest.Run(opts);
                case "config-patch-kspcf-first": return ConfigPatchTest.Run(opts, ConfigPatchTest.Scenario.KspcfFirst);
                case "config-patch-fastboot-first": return ConfigPatchTest.Run(opts, ConfigPatchTest.Scenario.FastBootFirst);
                case "config-patch-standalone": return ConfigPatchTest.Run(opts, ConfigPatchTest.Scenario.Standalone);
                case "diagnostics": return DiagnosticsTest.Run(opts);
                case "early-chain": return EarlyChainTest.Run(opts, EarlyChainTest.Mode.Enabled);
                case "early-chain-off": return EarlyChainTest.Run(opts, EarlyChainTest.Mode.Disabled);
                case "early-chain-foreign": return EarlyChainTest.Run(opts, EarlyChainTest.Mode.Foreign);
                default: return Usage();
            }
        }

        private static Assembly Resolve(object sender, ResolveEventArgs e)
        {
            string name = new AssemblyName(e.Name).Name + ".dll";
            foreach (string dir in SearchDirs)
            {
                string p = Path.Combine(dir, name);
                if (File.Exists(p))
                    return Assembly.LoadFrom(p);
            }
            return null;
        }

        /// "Mono <version>" when running under Mono (monohost), else the .NET Framework version.
        private static string RuntimeName()
        {
            Type mono = Type.GetType("Mono.Runtime");
            if (mono == null)
                return ".NET Framework " + Environment.Version;
            MethodInfo name = mono.GetMethod("GetDisplayName", BindingFlags.NonPublic | BindingFlags.Static);
            return "Mono " + (name == null ? "?" : name.Invoke(null, null));
        }

        private static int Usage()
        {
            Console.WriteLine("usage: FastBoot.Tests.exe <diff|allowlist|config-patch-kspcf-first|config-patch-fastboot-first|"
                + "config-patch-standalone|diagnostics|early-chain|early-chain-off|early-chain-foreign> --ksp <KSP folder> --dist <build output> [--cases N] [--out file] [--gamedata dir]");
            return 2;
        }
    }

    internal sealed class Options
    {
        public string Ksp;
        public string Dist;
        public int Cases = 200000;
        public string Out;

        public string Managed { get { return Path.Combine(Ksp, Path.Combine("KSP_x64_Data", "Managed")); } }
        /// --gamedata, else the install's own GameData.
        public string GameData { get { return GameDataOverride ?? Path.Combine(Ksp, "GameData"); } }
        public string GameDataOverride;
        public string FastBootHome { get { return Path.Combine(Dist, Path.Combine("GameData", "FastBoot")); } }
        public string FastBootPlugins { get { return Path.Combine(FastBootHome, "Plugins"); } }

        public string KspCfgPath
        {
            get
            {
                return Path.Combine(Path.Combine(Path.Combine(Path.Combine(FastBootHome, "PluginData"), "native"),
                    Native.NativeLibrary.PlatformFolder()), Native.NativeLibrary.FileName("kspcfg"));
            }
        }

        public static Options Parse(string[] args)
        {
            var o = new Options();
            for (int i = 1; i + 1 < args.Length; i += 2)
            {
                switch (args[i])
                {
                    case "--ksp": o.Ksp = args[i + 1]; break;
                    case "--dist": o.Dist = args[i + 1]; break;
                    case "--cases": o.Cases = int.Parse(args[i + 1]); break;
                    case "--out": o.Out = args[i + 1]; break;
                    case "--gamedata": o.GameDataOverride = args[i + 1]; break;
                    default: return null;
                }
            }
            return o.Ksp != null && o.Dist != null ? o : null;
        }
    }
}
