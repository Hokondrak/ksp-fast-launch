using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;

namespace FastBoot.Tests
{
    /// Developer tool, not a test: prints the IL of a KSP method, so a patch can be checked against exactly what
    /// stock does without a decompiler.
    ///
    ///   FastBoot.Tests.exe il --ksp <KSP> --dist <dist> --out "GameDatabase.GetAudioClip"
    internal static class IlDump
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
                Console.WriteLine("pass the method as --out Type.Method");
                return 2;
            }
            Type type = typeof(ConfigNode).Assembly.GetType(target.Substring(0, dot), false);
            if (type == null)
            {
                Console.WriteLine("no type " + target.Substring(0, dot) + " in Assembly-CSharp");
                return 1;
            }
            const BindingFlags all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
            var methods = type.GetMethods(all).Where(m => m.Name == target.Substring(dot + 1)).ToArray();
            if (methods.Length == 0)
            {
                Console.WriteLine("no method " + target);
                return 1;
            }
            foreach (MethodInfo m in methods)
                Dump(m);
            return 0;
        }

        private static void Dump(MethodInfo m)
        {
            Console.WriteLine(m.DeclaringType.FullName + "." + m.Name + "(" + string.Join(", ", m.GetParameters().Select(p => p.ParameterType.Name + " " + p.Name).ToArray()) + ")");
            MethodBody body = m.GetMethodBody();
            if (body == null)
            {
                Console.WriteLine("  (no body)");
                return;
            }
            byte[] il = body.GetILAsByteArray();
            int i = 0;
            while (i < il.Length)
            {
                int at = i;
                short code = il[i++];
                if (code == 0xFE)
                    code = (short)(0xFE00 | il[i++]);
                OpCode op;
                if (!OpCodesByValue.TryGetValue(code, out op))
                {
                    Console.WriteLine(string.Format("  IL_{0:X4}: ?? 0x{1:X}", at, code));
                    continue;
                }
                string arg = Operand(m.Module, op, il, ref i);
                Console.WriteLine(string.Format("  IL_{0:X4}: {1}{2}", at, op.Name, arg.Length > 0 ? " " + arg : ""));
            }
        }

        private static string Operand(Module module, OpCode op, byte[] il, ref int i)
        {
            switch (op.OperandType)
            {
                case OperandType.InlineNone:
                    return "";
                case OperandType.ShortInlineBrTarget:
                    return string.Format("IL_{0:X4}", i + 1 + (sbyte)il[i++]);
                case OperandType.ShortInlineI:
                case OperandType.ShortInlineVar:
                    return il[i++].ToString();
                case OperandType.InlineVar:
                    i += 2;
                    return BitConverter.ToInt16(il, i - 2).ToString();
                case OperandType.InlineBrTarget:
                    i += 4;
                    return string.Format("IL_{0:X4}", i + BitConverter.ToInt32(il, i - 4));
                case OperandType.InlineI:
                    i += 4;
                    return BitConverter.ToInt32(il, i - 4).ToString();
                case OperandType.InlineI8:
                    i += 8;
                    return BitConverter.ToInt64(il, i - 8).ToString();
                case OperandType.ShortInlineR:
                    i += 4;
                    return BitConverter.ToSingle(il, i - 4).ToString();
                case OperandType.InlineR:
                    i += 8;
                    return BitConverter.ToDouble(il, i - 8).ToString();
                case OperandType.InlineString:
                    i += 4;
                    return "\"" + module.ResolveString(BitConverter.ToInt32(il, i - 4)) + "\"";
                case OperandType.InlineSwitch:
                    int n = BitConverter.ToInt32(il, i);
                    i += 4 + 4 * n;
                    return "(" + n + " targets)";
                default: // tokens: method, field, type, member, sig
                    i += 4;
                    int token = BitConverter.ToInt32(il, i - 4);
                    try
                    {
                        MemberInfo member = module.ResolveMember(token);
                        return (member.DeclaringType != null ? member.DeclaringType.Name + "::" : "") + member.Name;
                    }
                    catch (Exception)
                    {
                        return string.Format("token 0x{0:X8}", token);
                    }
            }
        }
    }
}
