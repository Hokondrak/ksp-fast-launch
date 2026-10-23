using System;
using System.IO;
using System.Runtime.InteropServices;

namespace FastBoot.Native
{
    /// Loads a native library by full path and resolves its exports as delegates. DllImport probing differs
    /// between Mono on Windows, Linux and macOS, so FastBoot never relies on it for its own libraries.
    internal sealed class NativeLibrary
    {
        private readonly IntPtr handle;
        private readonly bool windows;

        private NativeLibrary(IntPtr handle, bool windows)
        {
            this.handle = handle;
            this.windows = windows;
        }

        public enum Platform
        {
            Windows,
            Linux,
            MacOS,
        }

        public static Platform Current
        {
            get
            {
                if (Path.DirectorySeparatorChar == '\\')
                    return Platform.Windows;
                return File.Exists("/usr/lib/libSystem.B.dylib") ? Platform.MacOS : Platform.Linux;
            }
        }

        /// File name of a library called `name` on this platform: name.dll, libname.so or libname.dylib.
        public static string FileName(string name)
        {
            switch (Current)
            {
                case Platform.Windows: return name + ".dll";
                case Platform.MacOS: return "lib" + name + ".dylib";
                default: return "lib" + name + ".so";
            }
        }

        /// Folder under PluginData/native for this platform's builds.
        public static string PlatformFolder()
        {
            switch (Current)
            {
                case Platform.Windows: return "win-x64";
                case Platform.MacOS: return "osx";
                default: return "linux-x64";
            }
        }

        /// Returns null (and the reason) instead of throwing, so callers can simply disable their feature.
        public static NativeLibrary TryLoad(string path, out string error)
        {
            error = null;
            if (!File.Exists(path))
            {
                error = "not found: " + path;
                return null;
            }
            try
            {
                if (Current == Platform.Windows)
                {
                    IntPtr h = LoadLibraryW(path);
                    if (h == IntPtr.Zero)
                        error = "LoadLibrary failed with error " + Marshal.GetLastWin32Error() + " for " + path;
                    return h == IntPtr.Zero ? null : new NativeLibrary(h, true);
                }
                IntPtr u = Current == Platform.MacOS ? MacDlopen(path, RtldNow) : LinuxDlopen(path, RtldNow);
                if (u == IntPtr.Zero)
                    error = "dlopen failed for " + path;
                return u == IntPtr.Zero ? null : new NativeLibrary(u, false);
            }
            catch (Exception ex)
            {
                error = ex.GetType().Name + ": " + ex.Message;
                return null;
            }
        }

        /// The export as a delegate of type T, or null if it is missing.
        public T Function<T>(string export) where T : class
        {
            IntPtr p;
            if (windows)
                p = GetProcAddress(handle, export);
            else
                p = Current == Platform.MacOS ? MacDlsym(handle, export) : LinuxDlsym(handle, export);
            if (p == IntPtr.Zero)
                return null;
            return Marshal.GetDelegateForFunctionPointer(p, typeof(T)) as T;
        }

        private const int RtldNow = 2;

        [DllImport("kernel32", SetLastError = true, CharSet = CharSet.Unicode)]
        private static extern IntPtr LoadLibraryW(string path);

        [DllImport("kernel32", CharSet = CharSet.Ansi, BestFitMapping = false)]
        private static extern IntPtr GetProcAddress(IntPtr module, string name);

        [DllImport("libdl.so.2", EntryPoint = "dlopen")]
        private static extern IntPtr LinuxDlopen(string path, int flags);

        [DllImport("libdl.so.2", EntryPoint = "dlsym")]
        private static extern IntPtr LinuxDlsym(IntPtr handle, string name);

        [DllImport("/usr/lib/libSystem.B.dylib", EntryPoint = "dlopen")]
        private static extern IntPtr MacDlopen(string path, int flags);

        [DllImport("/usr/lib/libSystem.B.dylib", EntryPoint = "dlsym")]
        private static extern IntPtr MacDlsym(IntPtr handle, string name);
    }
}
