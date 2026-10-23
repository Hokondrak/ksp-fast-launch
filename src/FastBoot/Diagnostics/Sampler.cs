// Main-thread stack sampler for stalls that log nothing.
//
// When the main thread has not completed a frame for a while, the watchdog thread suspends it for a few
// microseconds, copies its register context and the top of its stack into buffers allocated in advance,
// and resumes it. Nothing inside the suspended window allocates or takes a lock: the main thread may hold
// the GC or heap lock at that moment. After resuming, return addresses found on the copied stack are
// resolved with the Mono runtime's own mono_pmip, and the instruction pointer is attributed to its module
// (mono-2.0-bdwgc.dll is GC/JIT, the NVIDIA/d3d11 drivers are GPU work, ntdll/KernelBase are waits).
// Scanning a stack copy conservatively finds stale return addresses too, so read the results in aggregate.

using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

namespace FastBoot.Diagnostics
{
    internal struct StackSample
    {
        public double T;
        public double StalledFor;
        public int GcSinceLastSample;
        public string Module;     // module holding the instruction pointer
        public string[] Managed;  // managed frames, innermost first
    }

    internal sealed class Sampler
    {
        private const int StackBytes = 128 * 1024;
        private const int ContextSize = 0x4D0;
        private const int ContextFlagsOffset = 0x30;
        private const int RspOffset = 0x98;
        private const int RipOffset = 0xF8;
        private const uint ContextControlInteger = 0x00100003; // CONTEXT_AMD64 | CONTROL | INTEGER
        private const uint ThreadAccess = 0x0002 | 0x0008 | 0x0040; // SUSPEND_RESUME | GET_CONTEXT | QUERY_INFORMATION
        private const int MaxManagedFrames = 14;

        private readonly IntPtr thread;
        private readonly IntPtr contextRaw;
        private readonly IntPtr context;
        private readonly IntPtr stackCopy;
        private readonly ulong stackLow;
        private readonly ulong stackHigh;
        private readonly Dictionary<ulong, string> nameCache = new Dictionary<ulong, string>();
        private bool pmipAvailable = true;

        public bool Ready { get; private set; }

        // Must be constructed on the thread to be sampled.
        public Sampler()
        {
            try
            {
                thread = OpenThread(ThreadAccess, false, GetCurrentThreadId());
                UIntPtr low, high;
                GetCurrentThreadStackLimits(out low, out high);
                stackLow = low.ToUInt64();
                stackHigh = high.ToUInt64();
                contextRaw = Marshal.AllocHGlobal(ContextSize + 16);
                context = new IntPtr((contextRaw.ToInt64() + 15) & ~15L);
                stackCopy = Marshal.AllocHGlobal(StackBytes);
                WarmUp();
                Ready = thread != IntPtr.Zero;
            }
            catch (Exception)
            {
                Ready = false;
            }
        }

        // Mono compiles a P/Invoke stub on first call, under the JIT lock. If that first call happened while the
        // main thread was suspended holding that lock, the watchdog would wait forever and the game would hang.
        // So every call used inside the suspended window is made once here, harmlessly, on the main thread.
        private void WarmUp()
        {
            SuspendThread(IntPtr.Zero);   // invalid handle: fails, compiles the stub
            ResumeThread(IntPtr.Zero);
            Marshal.WriteInt32(context, ContextFlagsOffset, unchecked((int)ContextControlInteger));
            GetThreadContext(IntPtr.Zero, context);
            Marshal.ReadInt64(context, RipOffset);
            CopyMemory(stackCopy, context, new UIntPtr(8));
            Marshal.ReadInt64(stackCopy, 0);
        }

        // Called from the watchdog thread only.
        public bool TrySample(double t, double stalledFor, int gcSinceLast, out StackSample sample)
        {
            sample = default(StackSample);
            if (!Ready)
                return false;

            ulong rip, rsp;
            int copied;
            // ---- suspended window: no allocation, no managed locks ----
            if (SuspendThread(thread) == uint.MaxValue)
                return false;
            try
            {
                Marshal.WriteInt32(context, ContextFlagsOffset, unchecked((int)ContextControlInteger));
                if (!GetThreadContext(thread, context))
                    return false;
                rip = unchecked((ulong)Marshal.ReadInt64(context, RipOffset));
                rsp = unchecked((ulong)Marshal.ReadInt64(context, RspOffset));
                if (rsp < stackLow || rsp >= stackHigh)
                    return false;
                copied = (int)Math.Min((ulong)StackBytes, stackHigh - rsp);
                CopyMemory(stackCopy, new IntPtr(unchecked((long)rsp)), new UIntPtr((uint)copied));
            }
            finally
            {
                ResumeThread(thread);
            }
            // ---- main thread running again ----

            var frames = new List<string>(MaxManagedFrames);
            string top = Resolve(rip);
            if (top != null)
                frames.Add(top);
            for (int off = 0; off + 8 <= copied && frames.Count < MaxManagedFrames; off += 8)
            {
                ulong v = unchecked((ulong)Marshal.ReadInt64(stackCopy, off));
                if (v < 0x10000)
                    continue;
                string name = Resolve(v);
                if (name != null && (frames.Count == 0 || frames[frames.Count - 1] != name))
                    frames.Add(name);
            }
            sample = new StackSample
            {
                T = t,
                StalledFor = stalledFor,
                GcSinceLastSample = gcSinceLast,
                Module = ModuleOf(rip),
                Managed = frames.ToArray()
            };
            return true;
        }

        private string Resolve(ulong ip)
        {
            if (!pmipAvailable)
                return null;
            string cached;
            if (nameCache.TryGetValue(ip, out cached))
                return cached;
            string name = null;
            try
            {
                IntPtr p = mono_pmip(new IntPtr(unchecked((long)ip)));
                if (p != IntPtr.Zero)
                {
                    name = Marshal.PtrToStringAnsi(p);
                    mono_free(p);
                    int plus = name.IndexOf(" + ", StringComparison.Ordinal);
                    if (plus > 0)
                        name = name.Substring(0, plus);
                }
            }
            catch (Exception)
            {
                // Not running under Unity's Mono (e.g. the offline test): module attribution only.
                pmipAvailable = false;
            }
            if (nameCache.Count < 200000)
                nameCache[ip] = name;
            return name;
        }

        private static string ModuleOf(ulong ip)
        {
            IntPtr module;
            if (!GetModuleHandleEx(0x4 | 0x2, new IntPtr(unchecked((long)ip)), out module) || module == IntPtr.Zero)
                return "jit/unknown";
            var sb = new StringBuilder(260);
            if (GetModuleFileName(module, sb, sb.Capacity) == 0)
                return "unknown";
            string path = sb.ToString();
            int slash = path.LastIndexOf('\\');
            return slash >= 0 ? path.Substring(slash + 1) : path;
        }

        [DllImport("kernel32.dll")]
        private static extern uint GetCurrentThreadId();

        [DllImport("kernel32.dll")]
        private static extern IntPtr OpenThread(uint access, bool inherit, uint threadId);

        [DllImport("kernel32.dll")]
        private static extern void GetCurrentThreadStackLimits(out UIntPtr low, out UIntPtr high);

        [DllImport("kernel32.dll")]
        private static extern uint SuspendThread(IntPtr thread);

        [DllImport("kernel32.dll")]
        private static extern uint ResumeThread(IntPtr thread);

        [DllImport("kernel32.dll")]
        private static extern bool GetThreadContext(IntPtr thread, IntPtr context);

        [DllImport("kernel32.dll", EntryPoint = "RtlMoveMemory")]
        private static extern void CopyMemory(IntPtr dest, IntPtr src, UIntPtr count);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
        private static extern bool GetModuleHandleEx(uint flags, IntPtr address, out IntPtr module);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
        private static extern uint GetModuleFileName(IntPtr module, StringBuilder name, int size);

        [DllImport("mono-2.0-bdwgc.dll")]
        private static extern IntPtr mono_pmip(IntPtr ip);

        [DllImport("mono-2.0-bdwgc.dll")]
        private static extern void mono_free(IntPtr p);
    }
}
