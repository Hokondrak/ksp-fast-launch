//! Records which files under the KSP folder are opened, in first-open order. Fed by the early-start loader's
//! `CreateFileW` hook, so `note` runs on whatever thread opens a file and must stay cheap.

use crate::path_key;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

/// More than any install opens at boot; a bound keeps a runaway caller from growing memory without limit.
const CAPACITY: usize = 200_000;

pub struct Recorder {
    root_key: String,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    seen: HashSet<String>,
    order: Vec<String>,
    open: bool,
}

/// `path` with '/' separators and `.` and `..` segments resolved, without touching the disk: Unity opens files
/// as `KSP_x64_Data/../GameData/...`. The first segment (a drive, or empty for a Unix root) is kept as it is.
fn normalize(path: &str) -> String {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let mut segments = path.split(['\\', '/']);
    let mut out: Vec<&str> = segments.next().into_iter().collect();
    for seg in segments {
        match seg {
            "" | "." => {}
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

impl Recorder {
    pub fn new(root: &Path) -> Recorder {
        let mut root_key = path_key(&normalize(&root.to_string_lossy()));
        if !root_key.ends_with('/') {
            root_key.push('/');
        }
        Recorder { root_key, inner: Mutex::new(Inner { open: true, ..Inner::default() }) }
    }

    /// Notes that `path` (absolute, as passed to the OS, possibly with a `\\?\` prefix) was opened for reading.
    pub fn note(&self, path: &str) {
        let path = normalize(path);
        let key = path_key(&path);
        let Some(rel_key) = key.strip_prefix(&self.root_key) else { return };
        if rel_key.is_empty() {
            return;
        }
        // Lower-casing keeps the byte length for the paths KSP uses, so the tail of `path` is the relative path;
        // anything where it does not is skipped rather than guessed.
        let Some(rel) = path.get(path.len() - rel_key.len()..).filter(|r| path_key(r) == rel_key) else { return };
        let rel = rel.to_string();
        let Ok(mut g) = self.inner.lock() else { return };
        if g.open && g.order.len() < CAPACITY && g.seen.insert(rel_key.to_string()) {
            g.order.push(rel);
        }
    }

    /// Stops recording and returns what was recorded.
    pub fn finish(&self) -> Vec<String> {
        let Ok(mut g) = self.inner.lock() else { return Vec::new() };
        g.open = false;
        std::mem::take(&mut g.order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_first_opens_under_the_root_only() {
        let r = Recorder::new(Path::new(r"D:\Games\KSP"));
        r.note(r"D:\Games\KSP\GameData\A\a.cfg");
        r.note(r"\\?\D:\Games\KSP\KSP_x64_Data\resources.assets");
        r.note(r"d:\games\ksp\gamedata\a\A.CFG"); // same file again
        r.note(r"D:\Games\KSP2\GameData\x.cfg"); // sibling folder with a common prefix
        r.note(r"C:\Windows\Fonts\arial.ttf");
        r.note(r"D:\Games\KSP\");
        r.note(r"D:\Games\KSP\KSP_x64_Data\..\GameData\B\b.unity3d");
        r.note(r"D:\Games\KSP\GameData\B\.\b.unity3d"); // the same file, spelled differently
        r.note(r"D:\Games\KSP\..\KSP\GameData\C\c.cfg");
        r.note(r"D:\Games\KSP\..\..\..\..\outside.cfg");
        assert_eq!(r.finish(), ["GameData/A/a.cfg", "KSP_x64_Data/resources.assets", "GameData/B/b.unity3d", "GameData/C/c.cfg"]);
        r.note(r"D:\Games\KSP\GameData\late.cfg");
        assert!(r.finish().is_empty(), "nothing is recorded after finish");
    }
}
