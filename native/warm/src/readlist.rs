//! `readlist.txt`: the files KSP opened during a boot, in first-open order, relative to the KSP folder.
//!
//! One path per line with '/' separators; lines starting with '#' are comments. The file is advisory: entries
//! that no longer exist are skipped, so a stale list after adding or removing mods is harmless.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

const HEADER: &str = "# FastBoot read list v1: files KSP opened while booting, in order, relative to the KSP folder.\n\
# Written at the main menu by the early-start loader and used to warm the file cache on the next launch.\n";

/// Parses a list. Absolute paths and paths that climb out of the KSP folder are dropped.
pub fn parse(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(PathBuf::from)
        .filter(|p| p.components().all(|c| matches!(c, Component::Normal(_))))
        .collect()
}

/// Reads the list at `path`; a missing or unreadable file is an empty list.
pub fn load(path: &Path) -> Vec<PathBuf> {
    fs::read_to_string(path).map(|t| parse(&t)).unwrap_or_default()
}

/// Writes `entries` (relative, '/'-separated) through a temporary file, so a crash never leaves half a list.
pub fn save(path: &Path, entries: &[String]) -> io::Result<()> {
    let mut text = String::with_capacity(HEADER.len() + entries.iter().map(|e| e.len() + 1).sum::<usize>());
    text.push_str(HEADER);
    for e in entries {
        text.push_str(e);
        text.push('\n');
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_escapes() {
        let dir = std::env::temp_dir().join(format!("fastboot-readlist-{}", std::process::id()));
        let path = dir.join("readlist.txt");
        let entries = vec!["GameData/A/a.cfg".to_string(), "KSP_x64_Data/resources.assets".to_string()];
        save(&path, &entries).unwrap();
        let back: Vec<String> = load(&path).iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(back, entries);
        let _ = fs::remove_dir_all(&dir);

        let parsed = parse("# comment\n\nGameData/ok.cfg\n../outside.cfg\nGameData/../x.cfg\n/abs/path.cfg\nC:/abs.cfg\n");
        assert_eq!(parsed, vec![PathBuf::from("GameData/ok.cfg")]);
    }

    #[test]
    fn missing_file_is_empty() {
        assert!(load(Path::new("definitely/not/here/readlist.txt")).is_empty());
    }
}
