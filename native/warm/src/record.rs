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

impl Recorder {
    pub fn new(root: &Path) -> Recorder {
        let mut root_key = path_key(&root.to_string_lossy());
        if !root_key.ends_with('/') {
            root_key.push('/');
        }
        Recorder { root_key, inner: Mutex::new(Inner { open: true, ..Inner::default() }) }
    }

    /// Notes that `path` (absolute, as passed to the OS, possibly with a `\\?\` prefix) was opened for reading.
    pub fn note(&self, path: &str) {
        let path = path.strip_prefix(r"\\?\").unwrap_or(path);
        let key = path_key(path);
        let Some(rel_key) = key.strip_prefix(&self.root_key) else { return };
        if rel_key.is_empty() {
            return;
        }
        let rel = path[path.len() - rel_key.len()..].replace('\\', "/");
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
        assert_eq!(r.finish(), ["GameData/A/a.cfg", "KSP_x64_Data/resources.assets"]);
        r.note(r"D:\Games\KSP\GameData\late.cfg");
        assert!(r.finish().is_empty(), "nothing is recorded after finish");
    }
}
