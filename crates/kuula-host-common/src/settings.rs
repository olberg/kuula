//! The shell's persistent settings as a file: `settings.kv`, one
//! `key = value` a line. Every host that runs the shell reads and writes
//! this one format; where the file lives is the host's own. Unknown keys
//! and malformed lines are skipped, a missing file yields the defaults,
//! and nothing here is reachable from a cart.

use std::path::Path;

use kuula_core::shell::Settings;

/// The file's name.
pub const FILE: &str = "settings.kv";

/// Largest file read.
const MAX_BYTES: u64 = 64 * 1024;

/// Parse the text form over `defaults`; unparsable lines are skipped.
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

/// The text form.
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

    #[test]
    fn a_missing_file_gives_the_defaults_and_a_saved_one_is_read_back() {
        let dir =
            std::env::temp_dir().join(format!("kuula-host-common-{}-settings", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sub").join(FILE);
        assert_eq!(
            load_file(&path, Settings::default(), parse),
            Settings::default()
        );
        let s = Settings {
            scale: 4,
            volume: 10,
            net: false,
        };
        save_file(&path, &render(&s)).unwrap();
        assert_eq!(load_file(&path, Settings::default(), parse), s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
