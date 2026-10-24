using UnityEngine;

namespace FastBoot
{
    /// Runs once when the main menu first loads, which is where a boot ends.
    [KSPAddon(KSPAddon.Startup.MainMenu, true)]
    public sealed class BootFinishedAddon : MonoBehaviour
    {
        private void OnApplicationQuit()
        {
            Diagnostics.PartDatabaseWatch.Note(KSPUtil.ApplicationRootPath, "when the game quit");
            if (Diagnostics.PartDatabaseWatch.Enabled)
                Diagnostics.PqsCacheWatch.Note("when the game quit");
        }

        private void Start()
        {
            // Kept alive only for OnApplicationQuit; it does nothing per frame.
            DontDestroyOnLoad(gameObject);
            ColdStart.Warmup.BootFinished();
            Terrain.PqsCacheDeferral.StartPrewarm(this);
            if (Lookups.AudioLookupPatch.Installed)
                Log.Info("fast audio lookup: " + Lookups.AudioLookupPatch.Calls + " lookups, index built "
                    + Lookups.AudioLookupPatch.Rebuilds + " times");
            Diagnostics.PartDatabaseWatch.Note(KSPUtil.ApplicationRootPath, "at the main menu");
            if (Diagnostics.PartDatabaseWatch.Enabled)
                Diagnostics.PqsCacheWatch.Note("at the main menu");
            DragCubes.DragCubeReuse.AtMainMenu();
        }
    }
}
