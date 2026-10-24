//! What to warm, in what order.
//!
//! First the files the previous boot opened, in the order it opened them (the read list): that is exact, and
//! covers what no rule here can know, such as which asset bundles a mod loads. Then, for files the list does not
//! have (a new mod, or no list yet), KSP's loading order by kind:
//!
//! 1. Unity's own data (`KSP_x64_Data`): engine assets and the managed assemblies.
//! 2. GameData plugin assemblies, then config files (KSP's first config pass runs before any plugin).
//! 3. What GameDatabase loads: audio, then textures, then models.
//!
//! GameData folders named `PluginData` are skipped, as GameDatabase skips them. Files nothing reads at boot
//! (archives, craft files, `.ress` streaming resources) are not part of the rules; if the game does read them,
//! the read list will name them.

use crate::path_key;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The ordered list of files to read, as absolute paths.
pub struct Plan {
    pub files: Vec<PathBuf>,
    /// How many came from the read list.
    pub from_readlist: usize,
}

/// Loading phase of a file KSP reads at boot, or None if the rules do not expect it to be read.
fn phase(rel: &str) -> Option<u8> {
    let key = path_key(rel);
    let ext = key.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    if let Some(rest) = key.strip_prefix("ksp_x64_data/") {
        return match ext {
            "ress" | "resource" | "pdb" => None,
            "dll" if rest.starts_with("managed/") => Some(1),
            _ if rest.contains('/') && !rest.starts_with("managed/") => None,
            _ => Some(0),
        };
    }
    let rest = key.strip_prefix("gamedata/")?;
    if rest.split('/').any(|seg| seg == "plugindata") {
        return None;
    }
    match ext {
        "dll" => Some(2),
        "cfg" => Some(3),
        "wav" | "ogg" => Some(4),
        "dds" | "png" | "jpg" | "jpeg" | "tga" | "mbm" | "truecolor" => Some(5),
        "mu" => Some(6),
        _ => None,
    }
}

/// Deeper than any real mod layout; also ends a symlink loop.
const MAX_DEPTH: usize = 32;

/// Every file under `dir`, sorted by path, as paths relative to `root` with '/' separators. Symlinks and
/// junctions are followed, since players and mod managers link mod folders into GameData.
fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    walk_at(root, dir, out, 0)
}

fn walk_at(root: &Path, dir: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| path_key(&e.file_name().to_string_lossy()));
    for e in entries {
        let path = e.path();
        let Ok(kind) = e.file_type() else { continue };
        let kind = if kind.is_symlink() {
            match fs::metadata(&path) {
                Ok(m) => m.file_type(),
                Err(_) => continue, // dangling link
            }
        } else {
            kind
        };
        if kind.is_dir() {
            walk_at(root, &path, out, depth + 1);
        } else if kind.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// The rule-based order for the KSP install at `root`.
pub fn by_rules(root: &Path) -> Vec<PathBuf> {
    let mut all = Vec::new();
    walk(root, &root.join("KSP_x64_Data"), &mut all);
    walk(root, &root.join("GameData"), &mut all);
    let mut ranked: Vec<(u8, usize, String)> =
        all.into_iter().enumerate().filter_map(|(i, rel)| phase(&rel).map(|p| (p, i, rel))).collect();
    ranked.sort();
    ranked.into_iter().map(|(_, _, rel)| root.join(rel)).collect()
}

/// The read list (entries that still exist) followed by every rule-based file it does not already name.
pub fn build(root: &Path, readlist: &[PathBuf]) -> Plan {
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for rel in readlist {
        let abs = root.join(rel);
        if abs.is_file() && seen.insert(path_key(&rel.to_string_lossy())) {
            files.push(abs);
        }
    }
    let from_readlist = files.len();
    for abs in by_rules(root) {
        let rel = abs.strip_prefix(root).map(|r| r.to_string_lossy().into_owned()).unwrap_or_default();
        if seen.insert(path_key(&rel)) {
            files.push(abs);
        }
    }
    Plan { files, from_readlist }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_install() -> PathBuf {
        let root = std::env::temp_dir().join(format!("fastboot-plan-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&root);
        for f in [
            "KSP_x64_Data/globalgamemanagers",
            "KSP_x64_Data/resources.assets",
            "KSP_x64_Data/sharedassets0.resS",
            "KSP_x64_Data/Managed/Assembly-CSharp.dll",
            "KSP_x64_Data/Plugins/x86_64/lib.dll",
            "GameData/ModB/Parts/part.cfg",
            "GameData/ModB/Parts/model.mu",
            "GameData/ModB/Parts/tex.dds",
            "GameData/ModA/Plugins/ModA.dll",
            "GameData/ModA/Sounds/beep.ogg",
            "GameData/ModA/PluginData/cache.cfg",
            "GameData/ModA/readme.txt",
            "GameData/ModA/archive.zip",
        ] {
            let p = root.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"x").unwrap();
        }
        root
    }

    fn rel(root: &Path, files: &[PathBuf]) -> Vec<String> {
        files.iter().map(|f| f.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/")).collect()
    }

    #[test]
    fn rules_follow_loading_phases() {
        let root = fake_install();
        assert_eq!(
            rel(&root, &by_rules(&root)),
            [
                "KSP_x64_Data/globalgamemanagers",
                "KSP_x64_Data/resources.assets",
                "KSP_x64_Data/Managed/Assembly-CSharp.dll",
                "GameData/ModA/Plugins/ModA.dll",
                "GameData/ModB/Parts/part.cfg",
                "GameData/ModA/Sounds/beep.ogg",
                "GameData/ModB/Parts/tex.dds",
                "GameData/ModB/Parts/model.mu",
            ]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn readlist_comes_first_and_is_not_repeated() {
        let root = fake_install();
        let list = vec![
            PathBuf::from("GameData/ModB/Parts/model.mu"),
            PathBuf::from("GameData/ModA/archive.zip"), // read last boot, so warmed even though no rule wants it
            PathBuf::from("GameData/Gone/removed.cfg"), // no longer exists: skipped
            PathBuf::from("gamedata/modb/parts/MODEL.mu"), // same file, other case: once
        ];
        let plan = build(&root, &list);
        assert_eq!(plan.from_readlist, 2);
        let got = rel(&root, &plan.files);
        assert_eq!(&got[..2], ["GameData/ModB/Parts/model.mu", "GameData/ModA/archive.zip"]);
        assert_eq!(got.len(), 9);
        assert_eq!(got.iter().filter(|f| f.ends_with("model.mu")).count(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn follows_junctions() {
        let root = fake_install();
        let elsewhere = root.with_extension("linked");
        let _ = fs::remove_dir_all(&elsewhere);
        fs::create_dir_all(elsewhere.join("Parts")).unwrap();
        fs::write(elsewhere.join("Parts").join("linked.cfg"), b"x").unwrap();
        let link = root.join("GameData").join("ModC");
        let made = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"]).arg(&link).arg(&elsewhere)
            .output().map(|o| o.status.success()).unwrap_or(false);
        if made {
            assert!(rel(&root, &by_rules(&root)).contains(&"GameData/ModC/Parts/linked.cfg".to_string()));
            // rmdir removes the junction itself, never what it points to.
            let _ = std::process::Command::new("cmd").args(["/c", "rmdir"]).arg(&link).status();
        }
        let _ = fs::remove_dir_all(&elsewhere);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_install_is_an_empty_plan() {
        let plan = build(Path::new("definitely/not/a/ksp"), &[]);
        assert!(plan.files.is_empty());
    }
}
