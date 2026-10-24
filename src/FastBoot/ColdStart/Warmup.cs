using System;
using System.IO;
using System.Runtime.InteropServices;
using FastBoot.Native;

namespace FastBoot.ColdStart
{
    /// The cold-start warm-up (native/warm), seen from the managed side.
    ///
    /// With the early-start loader (version.dll in the KSP folder) the warm-up already runs from process start and
    /// records what the game opens; here FastBoot only tells it when the boot is over, so it saves the read list
    /// for the next launch. Without the loader, Start begins the same warm-up through kspcfg when FastBoot's addon
    /// loads. That is later (after Unity's data, the plugin assemblies and the first config pass), but still ahead
    /// of everything GameDatabase loads.
    internal static class Warmup
    {
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate int BootFinishedFn();
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate UIntPtr LoaderSummaryFn(IntPtr output, UIntPtr cap);
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate IntPtr StartFn(IntPtr root, UIntPtr rootLen, IntPtr list, UIntPtr listLen);
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate UIntPtr SummaryFn(IntPtr warmup, IntPtr output, UIntPtr cap);

        private static NativeLibrary loader;
        private static SummaryFn fallbackSummary;
        private static IntPtr fallback;

        /// FastBoot's early-start loader, if this process loaded it: version.dll from the KSP folder itself (the
        /// system's version.dll lives elsewhere) that has FastBoot's exports.
        private static NativeLibrary Loader(string kspRoot)
        {
            NativeLibrary lib = NativeLibrary.TryAttach(Path.Combine(kspRoot, "version.dll"));
            return lib != null && lib.Function<BootFinishedFn>("fastboot_early_boot_finished") != null ? lib : null;
        }

        /// Called from the addon's Awake: notes the loader's warm-up, or starts the fallback one.
        public static void Start(string kspRoot, string kspcfgPath, string readlistPath)
        {
            try
            {
                loader = Loader(kspRoot);
                if (loader != null)
                {
                    Log.Info("warm-up running since process start (early-start loader): " + LoaderSummary());
                    return;
                }
                string error;
                NativeLibrary lib = NativeLibrary.TryLoad(kspcfgPath, out error);
                var start = lib == null ? null : lib.Function<StartFn>("kspcfg_warm_start");
                fallbackSummary = lib == null ? null : lib.Function<SummaryFn>("kspcfg_warm_summary");
                if (start == null || fallbackSummary == null)
                {
                    Log.Warning("warm-up not started: " + (error ?? "kspcfg has no warm-up exports"));
                    return;
                }
                fallback = WithUtf16(Path.GetFullPath(kspRoot), (r, rl) => WithUtf16(readlistPath, (l, ll) => start(r, rl, l, ll)));
                Log.Info(fallback == IntPtr.Zero ? "warm-up could not start" : "warm-up started (no early-start loader, so from this point on)");
            }
            catch (Exception ex)
            {
                Log.Warning("warm-up not started: " + ex.Message);
            }
        }

        /// Called at the main menu: has the loader save the read list, and logs what the warm-up did.
        public static void BootFinished()
        {
            try
            {
                if (loader != null)
                {
                    int recorded = loader.Function<BootFinishedFn>("fastboot_early_boot_finished")();
                    Log.Info("boot finished; " + LoaderSummary() + (recorded >= 0 ? "; read list of " + recorded + " files saved for the next launch" : ""));
                }
                else if (fallback != IntPtr.Zero)
                {
                    Log.Info("boot finished; " + ReadUtf16((p, cap) => fallbackSummary(fallback, p, cap)));
                }
            }
            catch (Exception ex)
            {
                Log.Warning("warm-up report failed: " + ex.Message);
            }
        }

        private static string LoaderSummary()
        {
            var summary = loader.Function<LoaderSummaryFn>("fastboot_early_warmup_summary");
            return summary == null ? "(no summary)" : ReadUtf16((p, cap) => summary(p, cap));
        }

        private static unsafe T WithUtf16<T>(string s, Func<IntPtr, UIntPtr, T> f)
        {
            fixed (char* p = s)
                return f((IntPtr)p, new UIntPtr((uint)s.Length));
        }

        /// Calls a native function that writes UTF-16 into (buffer, capacity) and returns the full length.
        private static unsafe string ReadUtf16(Func<IntPtr, UIntPtr, UIntPtr> f)
        {
            int length = (int)f(IntPtr.Zero, UIntPtr.Zero).ToUInt32();
            var buffer = new char[length];
            fixed (char* p = buffer)
                length = Math.Min(length, (int)f((IntPtr)p, new UIntPtr((uint)buffer.Length)).ToUInt32());
            return new string(buffer, 0, length);
        }
    }
}
