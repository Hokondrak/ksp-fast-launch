using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;

namespace FastBoot.Tests
{
    /// Stock KSP parsing pieces, reached by reflection, plus a field-by-field tree comparison.
    internal static class StockParser
    {
        private const BindingFlags Private = BindingFlags.Static | BindingFlags.NonPublic;

        public static readonly Func<string[], List<string[]>> PreFormat =
            (Func<string[], List<string[]>>)Delegate.CreateDelegate(typeof(Func<string[], List<string[]>>),
                typeof(ConfigNode).GetMethod("PreFormatConfig", Private));

        public static readonly Func<List<string[]>, ConfigNode> RecurseFormat =
            (Func<List<string[]>, ConfigNode>)Delegate.CreateDelegate(typeof(Func<List<string[]>, ConfigNode>),
                typeof(ConfigNode).GetMethod("RecurseFormat", Private, null, new[] { typeof(List<string[]>) }, null));

        /// Stock LoadFromStringArray without localization: null for no tokens, else RecurseFormat.
        public static ConfigNode Tree(List<string[]> lines)
        {
            return lines == null || lines.Count == 0 ? null : RecurseFormat(lines);
        }

        /// First difference between two token lists, or null.
        public static string DiffTokens(List<string[]> stock, List<string[]> other)
        {
            int n = Math.Min(stock.Count, other.Count);
            for (int i = 0; i < n; i++)
                if (!stock[i].SequenceEqual(other[i]))
                    return "token " + i + ": stock [" + string.Join("|", stock[i]) + "] other [" + string.Join("|", other[i]) + "]";
            return stock.Count == other.Count ? null : "token count stock " + stock.Count + " other " + other.Count;
        }

        /// First difference between two trees: name, id, ordered values (duplicates kept), ordered children.
        public static string DiffTrees(ConfigNode a, ConfigNode b, string path = "")
        {
            if (a == null || b == null)
                return a == null && b == null ? null : path + ": null vs node";
            if (a.name != b.name) return path + ": name '" + a.name + "' vs '" + b.name + "'";
            if (a.id != b.id) return path + "/" + a.name + ": id '" + a.id + "' vs '" + b.id + "'";
            if (a.values.Count != b.values.Count) return path + "/" + a.name + ": " + a.values.Count + " vs " + b.values.Count + " values";
            for (int i = 0; i < a.values.Count; i++)
            {
                ConfigNode.Value x = a.values[i], y = b.values[i];
                if (x.name != y.name || x.value != y.value)
                    return path + "/" + a.name + " value " + i + ": '" + x.name + "=" + x.value + "' vs '" + y.name + "=" + y.value + "'";
            }
            if (a.nodes.Count != b.nodes.Count) return path + "/" + a.name + ": " + a.nodes.Count + " vs " + b.nodes.Count + " children";
            for (int i = 0; i < a.nodes.Count; i++)
            {
                string d = DiffTrees(a.nodes[i], b.nodes[i], path + "/" + a.name + "[" + i + "]");
                if (d != null) return d;
            }
            return null;
        }
    }
}
