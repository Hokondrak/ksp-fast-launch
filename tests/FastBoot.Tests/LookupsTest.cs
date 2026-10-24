using System;
using System.Collections.Generic;
using System.Linq;
using System.Runtime.Serialization;
using FastBoot.Lookups;
using HarmonyLib;
using UnityEngine;

namespace FastBoot.Tests
{
    /// 1. NameIndex against the stock algorithm (first live item whose name is equal) through random lists and
    ///    every kind of List<T> change, with duplicate names, dead items and missing names.
    /// 2. The Harmony patch on the real GameDatabase methods. Real AudioClips need the engine, so the list holds
    ///    only missing entries, which stock skips.
    internal static class LookupsTest
    {
        private sealed class Item
        {
            public string Name;
            public bool Alive = true;
        }

        public static int Run(Options o)
        {
            int failures = Fuzz(o.Cases);
            failures += Patch();
            return failures == 0 ? 0 : 1;
        }

        private static Item Stock(List<Item> items, string name)
        {
            for (int i = 0; i < items.Count; i++)
                if (items[i] != null && items[i].Alive && items[i].Name == name)
                    return items[i];
            return null;
        }

        private static int Fuzz(int cases)
        {
            NameIndex<Item>.CheckRuntime();
            var rng = new System.Random(12345);
            var names = Enumerable.Range(0, 40).Select(i => "Sounds/clip" + i).Concat(new[] { "", "Sounds/CLIP1" }).ToArray();
            Func<string> anyName = () => rng.Next(20) == 0 ? null : names[rng.Next(names.Length)];
            Func<Item> newItem = () => rng.Next(15) == 0 ? null : new Item { Name = names[rng.Next(names.Length)] };
            var index = new NameIndex<Item>(x => x.Name, x => x != null && x.Alive);
            var lists = new[] { new List<Item>(), new List<Item>() };
            int checks = 0, wrong = 0;
            for (int step = 0; step < Math.Max(cases, 1000); step++)
            {
                List<Item> list = lists[rng.Next(lists.Length)];
                switch (rng.Next(9))
                {
                    case 0: case 1: list.Add(newItem()); break;
                    case 2: if (list.Count > 0) list.Insert(rng.Next(list.Count), newItem()); break;
                    case 3: if (list.Count > 0) list.RemoveAt(rng.Next(list.Count)); break;
                    case 4: if (list.Count > 0) list[rng.Next(list.Count)] = newItem(); break;
                    case 5: // destroy an item in place, which List<T> does not see
                        if (list.Count > 0)
                        {
                            Item it = list[rng.Next(list.Count)];
                            if (it != null)
                                it.Alive = false;
                        }
                        break;
                    case 6: if (rng.Next(40) == 0) list.Clear(); break;
                    default: break; // lookups only
                }
                for (int k = 0; k < 3; k++)
                {
                    string name = anyName();
                    Item expected = Stock(list, name), got = index.Find(list, name);
                    checks++;
                    if (!ReferenceEquals(expected, got) && wrong++ < 5)
                        Console.WriteLine("WRONG at step {0}: '{1}' gave {2}, stock {3}", step, name ?? "(null)",
                            got == null ? "null" : got.Name, expected == null ? "null" : expected.Name);
                }
            }
            Console.WriteLine("name index: {0} lookups through random list changes, {1} differ from stock; {2} index builds", checks, wrong, index.Rebuilds);
            return wrong == 0 ? 0 : 1;
        }

        private static int Patch()
        {
            if (!AudioLookupPatch.Install())
                return 1;
            var db = (GameDatabase)FormatterServices.GetUninitializedObject(typeof(GameDatabase));
            db.databaseAudio = new List<AudioClip> { null, null };
            bool ok = db.GetAudioClip("Sounds/none") == null && !db.ExistsAudioClip("Sounds/none") && db.GetAudioClip(null) == null;
            var info = Harmony.GetPatchInfo(AudioLookupPatch.Target("GetAudioClip"));
            bool patched = info != null && info.Prefixes.Any(p => p.owner == AudioLookupPatch.HarmonyId) && AudioLookupPatch.Calls == 3;
            Console.WriteLine("GameDatabase audio patch: {0}, lookups through it: {1}", patched ? "installed" : "MISSING", AudioLookupPatch.Calls);
            return ok && patched ? 0 : 1;
        }
    }
}
