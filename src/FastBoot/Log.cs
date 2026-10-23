using System;

namespace FastBoot
{
    /// All FastBoot log output goes through here, prefixed "[FastBoot]". The sink is Unity's Debug.Log in game
    /// and the console in offline tests, so no code path that tests exercise touches Unity directly.
    internal static class Log
    {
        private static Action<string> sink = s => { };

        internal static void SetSink(Action<string> s)
        {
            sink = s ?? (x => { });
        }

        internal static void Info(string message)
        {
            sink("[FastBoot] " + message);
        }

        internal static void Warning(string message)
        {
            sink("[FastBoot] WARNING " + message);
        }
    }
}
