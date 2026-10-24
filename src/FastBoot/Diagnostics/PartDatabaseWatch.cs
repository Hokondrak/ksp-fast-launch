using System;
using System.Globalization;
using System.IO;

namespace FastBoot.Diagnostics
{
    /// Logs the state of KSP's PartDatabase.cfg (its drag cube cache) at points through the boot, to pin down
    /// when ModuleManager deletes it after a mod change and when KSP writes it again. A drag cube cache that
    /// survives mod changes has to act between those two moments. Only active while profiling.
    internal static class PartDatabaseWatch
    {
        internal static bool Enabled;

        internal static void Note(string kspRoot, string when)
        {
            if (!Enabled)
                return;
            try
            {
                var f = new FileInfo(Path.Combine(kspRoot, "PartDatabase.cfg"));
                Log.Info("PartDatabase.cfg " + when + ": " + (f.Exists
                    ? string.Format(CultureInfo.InvariantCulture, "{0} bytes, written {1:HH:mm:ss}", f.Length, f.LastWriteTime)
                    : "missing"));
            }
            catch (Exception ex)
            {
                Log.Warning("PartDatabase.cfg " + when + ": " + ex.Message);
            }
        }
    }
}
