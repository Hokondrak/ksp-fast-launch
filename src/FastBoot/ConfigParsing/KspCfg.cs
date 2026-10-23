using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using FastBoot.Native;

namespace FastBoot.ConfigParsing
{
    /// Bindings to the kspcfg native library (native/kspcfg). kspcfg turns a cfg file into UTF-16 text plus
    /// token offsets; ToLines turns those into the List<string[]> that stock ConfigNode.RecurseFormat consumes.
    internal sealed class KspCfg
    {
        public const uint RequiredVersion = 5;
        private const int StatusOk = 0;

        [StructLayout(LayoutKind.Sequential)]
        private struct Token
        {
            public uint Count;
            public uint AStart;
            public uint ALen;
            public uint BStart;
            public uint BLen;
        }

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate uint VersionFn();
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate IntPtr ParseFileFn(IntPtr path, UIntPtr len);
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate IntPtr ParseBytesFn(byte[] data, UIntPtr len);
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate IntPtr ParseManyFn(IntPtr paths, UIntPtr len, uint threads);
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate UIntPtr BatchLenFn(IntPtr batch);
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate IntPtr BatchGetFn(IntPtr batch, UIntPtr index);
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate void FreeFn(IntPtr handle);
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate int StatusFn(IntPtr parsed);
        [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate UIntPtr BufferFn(IntPtr parsed, out IntPtr data);

        private ParseFileFn parseFile;
        private ParseBytesFn parseBytes;
        private ParseManyFn parseMany;
        private BatchLenFn batchLen;
        private BatchGetFn batchGet;
        private FreeFn batchFree;
        private FreeFn free;
        private StatusFn status;
        private BufferFn text;
        private BufferFn tokens;

        private KspCfg()
        {
        }

        /// Loads kspcfg from `path`; returns null with a reason if it is missing, broken or too old.
        public static KspCfg Load(string path, out string error)
        {
            NativeLibrary lib = NativeLibrary.TryLoad(path, out error);
            if (lib == null)
                return null;
            var version = lib.Function<VersionFn>("kspcfg_version");
            if (version == null || version() < RequiredVersion)
            {
                error = "kspcfg at " + path + " is missing or older than version " + RequiredVersion;
                return null;
            }
            var k = new KspCfg
            {
                parseFile = lib.Function<ParseFileFn>("kspcfg_parse_file"),
                parseBytes = lib.Function<ParseBytesFn>("kspcfg_parse_bytes"),
                parseMany = lib.Function<ParseManyFn>("kspcfg_parse_many"),
                batchLen = lib.Function<BatchLenFn>("kspcfg_batch_len"),
                batchGet = lib.Function<BatchGetFn>("kspcfg_batch_get"),
                batchFree = lib.Function<FreeFn>("kspcfg_batch_free"),
                free = lib.Function<FreeFn>("kspcfg_free"),
                status = lib.Function<StatusFn>("kspcfg_status"),
                text = lib.Function<BufferFn>("kspcfg_text"),
                tokens = lib.Function<BufferFn>("kspcfg_tokens"),
            };
            if (k.parseFile == null || k.parseBytes == null || k.parseMany == null || k.batchLen == null || k.batchGet == null
                || k.batchFree == null || k.free == null || k.status == null || k.text == null || k.tokens == null)
            {
                error = "kspcfg at " + path + " lacks an expected export";
                return null;
            }
            return k;
        }

        /// Preformatted lines for one file, or null when the caller must use the stock parser
        /// (empty file, content outside the fast path, unreadable file, internal error).
        public unsafe List<string[]> FromFile(string path)
        {
            IntPtr p;
            fixed (char* c = path)
                p = parseFile(new IntPtr(c), new UIntPtr((uint)path.Length));
            try
            {
                return ToLines(p);
            }
            finally
            {
                free(p);
            }
        }

        /// Same as FromFile for bytes already in memory (tests).
        public List<string[]> FromBytes(byte[] data)
        {
            IntPtr p = parseBytes(data, new UIntPtr((uint)data.Length));
            try
            {
                return ToLines(p);
            }
            finally
            {
                free(p);
            }
        }

        /// Parses many files on worker threads. The returned batch must be released with Dispose.
        public unsafe Batch ParseMany(string[] paths, int threads)
        {
            string joined = string.Join("\0", paths);
            IntPtr b;
            fixed (char* c = joined)
                b = parseMany(new IntPtr(c), new UIntPtr((uint)joined.Length), (uint)Math.Max(1, threads));
            return b == IntPtr.Zero ? null : new Batch(this, b);
        }

        /// Pre-parsed files, indexed like the input array. Not thread-safe against Dispose.
        public sealed class Batch : IDisposable
        {
            private readonly KspCfg owner;
            private IntPtr handle;

            internal Batch(KspCfg owner, IntPtr handle)
            {
                this.owner = owner;
                this.handle = handle;
            }

            public int Count
            {
                get { return handle == IntPtr.Zero ? 0 : (int)owner.batchLen(handle).ToUInt32(); }
            }

            /// Lines for entry i, or null when the stock parser must be used for it.
            public List<string[]> Lines(int i)
            {
                if (handle == IntPtr.Zero)
                    return null;
                return owner.ToLines(owner.batchGet(handle, new UIntPtr((uint)i)));
            }

            public void Dispose()
            {
                if (handle != IntPtr.Zero)
                    owner.batchFree(handle);
                handle = IntPtr.Zero;
            }
        }

        private unsafe List<string[]> ToLines(IntPtr parsed)
        {
            if (parsed == IntPtr.Zero || status(parsed) != StatusOk)
                return null;
            IntPtr textPtr, tokPtr;
            text(parsed, out textPtr);
            int count = (int)tokens(parsed, out tokPtr).ToUInt32();
            var lines = new List<string[]>(count);
            char* t = (char*)textPtr;
            Token* tok = (Token*)tokPtr;
            for (int i = 0; i < count; i++)
            {
                string a = Str(t, tok[i].AStart, tok[i].ALen);
                lines.Add(tok[i].Count == 1 ? new[] { a } : new[] { a, Str(t, tok[i].BStart, tok[i].BLen) });
            }
            return lines;
        }

        // The text is already UTF-16, so each string is a straight copy. Braces are shared instances.
        private static unsafe string Str(char* text, uint start, uint len)
        {
            if (len == 0)
                return string.Empty;
            if (len == 1)
            {
                char c = text[start];
                if (c == '{')
                    return "{";
                if (c == '}')
                    return "}";
            }
            return new string(text, (int)start, (int)len);
        }
    }
}
