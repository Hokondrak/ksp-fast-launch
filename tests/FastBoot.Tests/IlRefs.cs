using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;

namespace FastBoot.Tests
{
    /// Developer tool, not a test: lists every method, in KSP's assembly and in every plugin under GameData, whose
    /// IL references a given member (field, method or property accessor). Used to find everyone who reads a field
    /// before changing when it is set.
    ///
    ///   FastBoot.Tests.exe refs --ksp <KSP> --dist <dist> --out "AvailablePart.iconPrefab"
    internal static class IlRefs
    {
        private static readonly Dictionary<short, OpCode> OpCodesByValue = typeof(OpCodes)
            .GetFields(BindingFlags.Public | BindingFlags.Static)
            .Select(f => (OpCode)f.GetValue(null))
            .ToDictionary(o => o.Value);

        public static int Run(Options o)
        {
            string target = o.Out;
            int dot = target == null ? -1 : target.LastIndexOf('.');
            if (dot <= 0)
            {
                Console.WriteLine("pass the member as --out Type.Member");
                return 2;
            }
            string typeName = target.Substring(0, dot), member = target.Substring(dot + 1);
            var assemblies = new List<Assembly> { typeof(ConfigNode).Assembly };
            foreach (string dll in Directory.GetFiles(o.GameData, "*.dll", SearchOption.AllDirectories))
            {
                if (dll.IndexOf(Path.DirectorySeparatorChar + "PluginData" + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase) >= 0)
                    continue;
                try
                {
                    assemblies.Add(Assembly.LoadFrom(dll));
                }
                catch (Exception)
                {
                    // native DLLs and broken assemblies
                }
            }
            int hits = 0, unreadable = 0;
            foreach (Assembly a in assemblies.Distinct())
            {
                Type[] types;
                try
                {
                    types = a.GetTypes();
                }
                catch (ReflectionTypeLoadException ex)
                {
                    types = ex.Types.Where(t => t != null).ToArray();
                }
                catch (Exception)
                {
                    unreadable++;
                    continue;
                }
                foreach (Type t in types)
                {
                    const BindingFlags all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
                    MethodBase[] methods;
                    try
                    {
                        methods = t.GetMethods(all).Cast<MethodBase>().Concat(t.GetConstructors(all)).ToArray();
                    }
                    catch (Exception)
                    {
                        continue;
                    }
                    foreach (MethodBase m in methods)
                    {
                        string how = References(m, typeName, member);
                        if (how == null)
                            continue;
                        hits++;
                        Console.WriteLine("{0,-28} {1}.{2}  [{3}]", a.GetName().Name, t.FullName, m.Name, how);
                    }
                }
            }
            Console.WriteLine("{0} methods reference {1} ({2} assemblies scanned, {3} unreadable)", hits, target, assemblies.Count, unreadable);
            return 0;
        }

        /// The opcodes `m` uses on Type.member ("ldfld", "stfld", "call"...), or null if it never touches it.
        private static string References(MethodBase m, string typeName, string member)
        {
            MethodBody body;
            try
            {
                body = m.GetMethodBody();
            }
            catch (Exception)
            {
                return null;
            }
            if (body == null)
                return null;
            byte[] il = body.GetILAsByteArray();
            var uses = new SortedSet<string>();
            int i = 0;
            while (i < il.Length)
            {
                short code = il[i++];
                if (code == 0xFE && i < il.Length)
                    code = (short)(0xFE00 | il[i++]);
                OpCode op;
                if (!OpCodesByValue.TryGetValue(code, out op))
                    return uses.Count > 0 ? string.Join(",", uses.ToArray()) : null;
                switch (op.OperandType)
                {
                    case OperandType.InlineField:
                    case OperandType.InlineMethod:
                    case OperandType.InlineTok:
                        int token = BitConverter.ToInt32(il, i);
                        try
                        {
                            MemberInfo r = m.Module.ResolveMember(token, m.DeclaringType != null && m.DeclaringType.IsGenericType ? m.DeclaringType.GetGenericArguments() : null,
                                m.IsGenericMethod ? m.GetGenericArguments() : null);
                            string name = r.Name;
                            if (name.StartsWith("get_") || name.StartsWith("set_"))
                                name = name.Substring(4);
                            if (name == member && r.DeclaringType != null && r.DeclaringType.Name == typeName)
                                uses.Add(op.Name);
                        }
                        catch (Exception)
                        {
                            // a reference into an assembly that cannot be resolved here
                        }
                        i += 4;
                        break;
                    case OperandType.InlineNone: break;
                    case OperandType.ShortInlineBrTarget:
                    case OperandType.ShortInlineI:
                    case OperandType.ShortInlineVar: i += 1; break;
                    case OperandType.InlineVar: i += 2; break;
                    case OperandType.InlineI8:
                    case OperandType.InlineR: i += 8; break;
                    case OperandType.InlineSwitch: i += 4 + 4 * BitConverter.ToInt32(il, i); break;
                    default: i += 4; break;
                }
            }
            return uses.Count > 0 ? string.Join(",", uses.ToArray()) : null;
        }
    }
}
