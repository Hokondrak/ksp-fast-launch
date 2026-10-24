//! Reads one boolean from `GameData/FastBoot/FastBoot.cfg`, in the same tiny `key = value` format the C# side
//! reads, so the native loader honours the same switches before any managed code runs.

use std::path::Path;

/// `key`'s value if the file has `key = true|false` (case-insensitive value, `//` comments), else `default`.
pub fn read_bool(cfg: &Path, key: &str, default: bool) -> bool {
    let Ok(text) = std::fs::read_to_string(cfg) else { return default };
    parse_bool(&text, key).unwrap_or(default)
}

fn parse_bool(text: &str, key: &str) -> Option<bool> {
    let mut found = None;
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("");
        let Some((k, v)) = line.split_once('=') else { continue };
        if k.trim() == key {
            match v.trim().to_ascii_lowercase().as_str() {
                "true" => found = Some(true),
                "false" => found = Some(false),
                _ => {}
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_settings_format() {
        let cfg = "FASTBOOT_SETTINGS\n{\n    // Warmup = false\n    Warmup = False // off\n    WarmupX = true\n}\n";
        assert_eq!(parse_bool(cfg, "Warmup"), Some(false));
        assert_eq!(parse_bool(cfg, "Missing"), None);
        assert_eq!(parse_bool("Warmup = maybe", "Warmup"), None);
        assert!(read_bool(Path::new("no/such/FastBoot.cfg"), "Warmup", true));
    }
}
