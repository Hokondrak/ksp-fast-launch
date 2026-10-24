using System;
using System.Collections.Generic;
using System.Reflection;
using System.Reflection.Emit;

namespace FastBoot.Lookups
{
    /// First-match lookup by name over a List<T>, giving what a front-to-back scan for the first live item with
    /// that name gives, in constant time.
    ///
    /// The index is rebuilt whenever the list changes: List<T> bumps a private version counter on every Add,
    /// Insert, Remove, Clear and indexer write, and that counter is checked on every lookup. A hit is re-checked
    /// (still alive, same name) before it is returned, so an item destroyed since the build falls back to a
    /// rebuild. What it cannot see is an item renamed after the build; KSP's databases name items once, at load.
    internal sealed class NameIndex<T> where T : class
    {
        private static readonly Func<List<T>, int> ListVersion = VersionGetter();

        private static Func<List<T>, int> VersionGetter()
        {
            FieldInfo field = typeof(List<T>).GetField("_version", BindingFlags.Instance | BindingFlags.NonPublic);
            if (field == null || field.FieldType != typeof(int))
                throw new MissingFieldException(typeof(List<T>).FullName, "_version");
            var getter = new DynamicMethod("ListVersion", typeof(int), new[] { typeof(List<T>) }, typeof(NameIndex<T>), true);
            ILGenerator il = getter.GetILGenerator();
            il.Emit(OpCodes.Ldarg_0);
            il.Emit(OpCodes.Ldfld, field);
            il.Emit(OpCodes.Ret);
            return (Func<List<T>, int>)getter.CreateDelegate(typeof(Func<List<T>, int>));
        }

        private readonly Func<T, string> nameOf;
        private readonly Func<T, bool> isAlive;
        private readonly Dictionary<string, int> first = new Dictionary<string, int>();
        private List<T> indexed;
        private int version;

        public int Rebuilds { get; private set; }

        public NameIndex(Func<T, string> nameOf, Func<T, bool> isAlive)
        {
            this.nameOf = nameOf;
            this.isAlive = isAlive;
        }

        /// Fails (throws) if this runtime's List<T> has no version counter, so callers can leave stock alone.
        public static void CheckRuntime()
        {
            ListVersion(new List<T>());
        }

        public T Find(List<T> items, string name)
        {
            if (name == null)
                return Scan(items, null);
            if (!ReferenceEquals(items, indexed) || ListVersion(items) != version)
                Rebuild(items);
            int i;
            if (!first.TryGetValue(name, out i))
                return null;
            T item = items[i];
            if (isAlive(item) && nameOf(item) == name)
                return item;
            // Destroyed or renamed since the build: the index no longer reflects the items.
            Rebuild(items);
            return first.TryGetValue(name, out i) ? items[i] : null;
        }

        private void Rebuild(List<T> items)
        {
            first.Clear();
            for (int i = 0; i < items.Count; i++)
            {
                T item = items[i];
                if (!isAlive(item))
                    continue;
                string n = nameOf(item);
                if (n != null && !first.ContainsKey(n))
                    first.Add(n, i);
            }
            indexed = items;
            version = ListVersion(items);
            Rebuilds++;
        }

        /// The stock algorithm, for the cases the index does not cover.
        private T Scan(List<T> items, string name)
        {
            for (int i = 0; i < items.Count; i++)
            {
                T item = items[i];
                if (isAlive(item) && nameOf(item) == name)
                    return item;
            }
            return null;
        }
    }
}
