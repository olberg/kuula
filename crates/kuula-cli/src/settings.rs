//! The shell's persistent settings: `settings.kv` beside the save root
//! (`%LOCALAPPDATA%\kuula\settings.kv` on Windows, or under
//! `KUULA_SAVE_ROOT`'s parent). One `key = value` per line; unknown
//! keys are kept as they are, a missing or malformed file yields the
//! defaults with a note on stderr, and a write failure is reported and
//! otherwise ignored. Nothing here is reachable from a cart.

use std::path::{Path, PathBuf};

use kuula_core::shell::Settings;

pub const FILE: &str = "settings.kv";

/// Largest file read.
const MAX_BYTES: u64 = 64 * 1024;

/// Where the file lives, if a save root is known.
pub fn path() -> Option<PathBuf> {
    let root = kuula_core::save::default_save_root()?;
    Some(root.parent()?.join(FILE))
}

/// The directory `settings.kv` lives in: where the deploy key and
/// approved list (`deploy/`) are kept.
#[cfg(feature = "net")]
pub fn data_dir() -> Option<PathBuf> {
    Some(path()?.parent()?.to_path_buf())
}

/// Parse the text form; unparsable lines are skipped.
pub fn parse(text: &str, defaults: Settings) -> Settings {
    let mut s = defaults;
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "scale" => {
                if let Ok(n) = v.parse::<u32>() {
                    if (1..=4).contains(&n) {
                        s.scale = n;
                    }
                }
            }
            "volume" => {
                if let Ok(n) = v.parse::<u32>() {
                    if n <= 100 {
                        s.volume = n;
                    }
                }
            }
            "net" => match v {
                "on" | "true" => s.net = true,
                "off" | "false" => s.net = false,
                _ => {}
            },
            _ => {}
        }
    }
    s
}

pub fn render(s: &Settings) -> String {
    format!(
        "scale = {}\nvolume = {}\nnet = {}\n",
        s.scale,
        s.volume,
        if s.net { "on" } else { "off" }
    )
}

/// A `key = value` file parsed over `defaults`: a missing file yields the
/// defaults silently, an oversized or unreadable one with a note on stderr.
pub fn load_file<T>(path: &Path, defaults: T, parse: impl FnOnce(&str, T) -> T) -> T {
    match std::fs::metadata(path) {
        Ok(m) if m.len() > MAX_BYTES => {
            eprintln!("settings: {} is too large; using defaults", path.display());
            return defaults;
        }
        Ok(_) => {}
        Err(_) => return defaults,
    }
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text, defaults),
        Err(e) => {
            eprintln!("settings: cannot read {}: {e}", path.display());
            defaults
        }
    }
}

/// Write a settings file, creating its directory.
pub fn save_file(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}

/// The settings on disk over `defaults`, or the defaults.
pub fn load(defaults: Settings) -> Settings {
    match path() {
        Some(path) => load_file(&path, defaults, parse),
        None => defaults,
    }
}

/// Write the settings; a failure is reported, not fatal.
pub fn save(s: &Settings) {
    let Some(path) = path() else {
        return;
    };
    if let Err(e) = save_file(&path, &render(s)) {
        eprintln!("settings: cannot write {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tolerance() {
        let s = Settings {
            scale: 3,
            volume: 40,
            net: true,
        };
        assert_eq!(parse(&render(&s), Settings::default()), s);
        let lenient = parse(
            "garbage\nscale = 9\nvolume = x\nnet = on\nfuture = 1\n",
            Settings::default(),
        );
        assert_eq!(
            lenient,
            Settings {
                scale: 2,
                volume: 100,
                net: true
            }
        );
        assert_eq!(parse("", Settings::default()), Settings::default());
    }
}
