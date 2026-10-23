//! `early.log`: one file per launch, written line by line so it survives a crash.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static FILE: Mutex<Option<File>> = Mutex::new(None);

/// Starts a fresh log at `path`. Without one, `line` is a no-op: logging must never stop the game.
pub fn open(path: &Path) {
    let _ = path.parent().map(std::fs::create_dir_all);
    let f = OpenOptions::new().create(true).write(true).truncate(true).open(path).ok();
    if let Ok(mut g) = FILE.lock() {
        *g = f;
    }
}

pub fn line(message: &str) {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    if let Ok(mut g) = FILE.lock() {
        if let Some(f) = g.as_mut() {
            let _ = writeln!(f, "{} {}", ms, message);
        }
    }
}
