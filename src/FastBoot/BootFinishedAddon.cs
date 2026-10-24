using UnityEngine;

namespace FastBoot
{
    /// Runs once when the main menu first loads, which is where a boot ends.
    [KSPAddon(KSPAddon.Startup.MainMenu, true)]
    public sealed class BootFinishedAddon : MonoBehaviour
    {
        private void Start()
        {
            ColdStart.Warmup.BootFinished();
        }
    }
}
