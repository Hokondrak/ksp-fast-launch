using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text;

namespace FastBoot.DragCubes
{
    /// What a part's drag cubes are rendered from, hashed. Two boots that give a part the same fingerprint render
    /// it from the same inputs, so its cubes from the earlier boot are still valid.
    ///
    /// Inputs, deliberately broad (a false "changed" only costs a re-render):
    /// - the part's final config after ModuleManager (models, scale, variants, every module's settings);
    /// - every file, by size and timestamp, in each folder its models and texture overrides come from (the part's
    ///   own folder for `mesh =` parts and parts without a MODEL node);
    /// - the assembly file of every module the part lists (module code can change the model before rendering);
    /// - a global key: KSP's own assembly and every assembly that patches drag cube generation.
    internal static class Fingerprint
    {
        public const string Format = "fastboot-dragcube-1";

        /// Folders whose files can change how `part` renders. `partFile` is the .cfg the part is defined in.
        public static List<string> ModelFolders(ConfigNode part, string partFile, string gameData)
        {
            var folders = new List<string>();
            string own = Path.GetDirectoryName(partFile);
            ConfigNode[] models = part.GetNodes("MODEL");
            foreach (ConfigNode model in models)
            {
                string path = model.GetValue("model");
                if (!string.IsNullOrEmpty(path))
                    folders.Add(Path.GetDirectoryName(Path.Combine(gameData, path)));
                // texture = <name>, <url of the replacement>
                foreach (string texture in model.GetValues("texture"))
                {
                    int comma = texture.IndexOf(',');
                    string url = comma < 0 ? null : texture.Substring(comma + 1).Trim();
                    if (!string.IsNullOrEmpty(url))
                        folders.Add(Path.GetDirectoryName(Path.Combine(gameData, url)));
                }
            }
            if (models.Length == 0 || part.HasValue("mesh"))
                folders.Add(own);
            return folders.Select(f => Path.GetFullPath(f)).Distinct(StringComparer.OrdinalIgnoreCase).OrderBy(f => f, StringComparer.OrdinalIgnoreCase).ToList();
        }

        /// Module names the part lists, in order.
        public static List<string> Modules(ConfigNode part)
        {
            return part.GetNodes("MODULE").Select(m => m.GetValue("name") ?? "").ToList();
        }

        /// Size and timestamp of `path`, or "missing".
        public static string Stamp(string path)
        {
            var f = new FileInfo(path);
            return f.Exists ? f.Length.ToString(CultureInfo.InvariantCulture) + "@" + f.LastWriteTimeUtc.Ticks.ToString(CultureInfo.InvariantCulture) : "missing";
        }

        /// Every file directly in `folder` with its stamp, in a stable order.
        public static string FolderStamp(string folder)
        {
            if (!Directory.Exists(folder))
                return folder + ": missing";
            var sb = new StringBuilder(folder).Append(':');
            foreach (FileInfo f in new DirectoryInfo(folder).GetFiles().OrderBy(f => f.Name, StringComparer.Ordinal))
                sb.Append(' ').Append(f.Name).Append('=').Append(f.Length).Append('@').Append(f.LastWriteTimeUtc.Ticks);
            return sb.ToString();
        }

        public static string Compute(string globalKey, string configText, IEnumerable<string> folderStamps, IEnumerable<string> moduleStamps)
        {
            var sb = new StringBuilder();
            sb.Append(Format).Append('\n').Append(globalKey).Append('\n').Append(configText).Append('\n');
            foreach (string s in folderStamps)
                sb.Append("F ").Append(s).Append('\n');
            foreach (string s in moduleStamps)
                sb.Append("M ").Append(s).Append('\n');
            using (var sha = SHA256.Create())
            {
                byte[] hash = sha.ComputeHash(Encoding.UTF8.GetBytes(sb.ToString()));
                return BitConverter.ToString(hash).Replace("-", "").ToLowerInvariant();
            }
        }
    }
}
