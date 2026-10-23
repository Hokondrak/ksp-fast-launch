using System;
using System.Collections.Generic;

namespace FastBoot
{
    /// All FastBoot log output goes through here, prefixed "[FastBoot]". The sink is Unity's Debug.Log in game
    /// and the console in offline tests, so no code path that tests exercise touches Unity directly. Messages
    /// logged before a sink exists (the early bootstrap runs before Unity's logger) are kept and flushed to the
    /// first sink.
    internal static class Log
    {
        private static readonly object Gate = new object();
        private static readonly List<string> Pending = new List<string>();
        private static Action<string> sink;

        internal static void SetSink(Action<string> s)
        {
            string[] backlog;
            lock (Gate)
            {
                sink = s;
                backlog = Pending.ToArray();
                Pending.Clear();
            }
            if (s != null)
                foreach (string line in backlog)
                    s(line);
        }

        internal static void Info(string message)
        {
            Write("[FastBoot] " + message);
        }

        internal static void Warning(string message)
        {
            Write("[FastBoot] WARNING " + message);
        }

        private static void Write(string line)
        {
            Action<string> s;
            lock (Gate)
            {
                s = sink;
                if (s == null)
                {
                    Pending.Add(line);
                    return;
                }
            }
            s(line);
        }
    }
}
