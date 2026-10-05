//! The shell's persistent settings: `settings.kv` under the app's internal
//! data directory, in the format every host writes
//! (`kuula_host_common::settings`). A missing file yields the defaults,
//! and a write failure is reported and otherwise ignored. Nothing here is
//! reachable from a cart.

use std::path::Path;

use kuula_core::shell::Settings;
pub use kuula_host_common::settings::FILE;
use kuula_host_common::settings::{load_file, parse, render, save_file};

/// The settings in the file at `path` over `defaults`; the defaults when it
/// is missing, too large or unreadable (which a line in the log says: the
/// app's stderr goes there).
pub fn load(path: &Path, defaults: Settings) -> Settings {
    load_file(path, defaults, parse)
}

/// Write the settings to `path`, creating its directory. A failure is
/// logged, not fatal.
pub fn save(path: &Path, s: &Settings) {
    if let Err(e) = save_file(path, &render(s)) {
        log::warn!("settings: cannot write {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_gives_the_defaults_and_a_saved_one_is_read_back() {
        let dir = crate::testdir::new("settings");
        let path = dir.join("sub").join(FILE);
        assert_eq!(load(&path, Settings::default()), Settings::default());
        let s = Settings {
            scale: 4,
            volume: 10,
            net: false,
        };
        save(&path, &s);
        assert_eq!(load(&path, Settings::default()), s);
    }
}
