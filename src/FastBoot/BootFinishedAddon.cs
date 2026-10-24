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
        }

        private void Start()
        {
            // Kept alive only for OnApplicationQuit; it does nothing per frame.
            DontDestroyOnLoad(gameObject);
            ColdStart.Warmup.BootFinished();
            Diagnostics.PartDatabaseWatch.Note(KSPUtil.ApplicationRootPath, "at the main menu");
        }
    }
}
