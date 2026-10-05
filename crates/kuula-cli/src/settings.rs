//! The shell's persistent settings: `settings.kv` beside the save root
//! (`%LOCALAPPDATA%\kuula\settings.kv` on Windows, or under
//! `KUULA_SAVE_ROOT`'s parent). The format is the one every host writes
//! (`kuula_host_common::settings`); this is where the desktop keeps the
//! file. A write failure is reported and otherwise ignored. Nothing here
//! is reachable from a cart.

use std::path::PathBuf;

use kuula_core::shell::Settings;

pub use kuula_host_common::settings::{load_file, save_file};
use kuula_host_common::settings::{parse, render, FILE};

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
