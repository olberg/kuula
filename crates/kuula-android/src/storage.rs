//! The app's directories, and the built-in carts copied out of the APK.
//!
//! The assets are inside the APK, where the shell cannot list them as
//! files, so each one is copied to `<internal>/carts`, overwriting what is
//! there. That is done when the package is another than the one they were
//! last copied from, which a stamp beside the directory says: the APK's
//! path, size and time, which every install changes, a reinstall of the
//! same version too. A person's own carts go in `<external>/carts`, the
//! app's external files directory, which a file manager, USB and adb
//! reach.

use std::ffi::CString;
use std::io::Read;
use std::path::{Path, PathBuf};

use android_activity::AndroidApp;

/// The assets directory the carts are in.
const ASSET_DIR: &str = "carts";

/// The file, beside the built-in carts, that says which package they were
/// copied from.
const STAMP_FILE: &str = "carts.stamp";

/// Where everything lives.
pub struct Dirs {
    /// The built-in carts, copied from the assets.
    pub builtin_carts: PathBuf,
    /// A person's own carts.
    pub own_carts: PathBuf,
    /// Saves, a directory for each cart.
    pub saves: PathBuf,
    /// The settings file.
    pub settings: PathBuf,
}

/// Make the directories and copy the built-in carts. `None` when the app
/// has no internal data directory, which Android always gives it.
pub fn prepare(app: &AndroidApp) -> Option<Dirs> {
    let internal = app.internal_data_path()?;
    let builtin_carts = internal.join(ASSET_DIR);
    let own_carts = match app.external_data_path() {
        Some(external) => external.join(ASSET_DIR),
        // No external storage: nothing to list there, and nothing is made.
        None => internal.join("no-external-carts"),
    };
    if let Err(e) = std::fs::create_dir_all(&own_carts) {
        log::warn!("cannot make {}: {e}", own_carts.display());
    }
    let stamp_file = internal.join(STAMP_FILE);
    let stamp = crate::activity::package_stamp(app);
    if crate::carts::builtin_up_to_date(&stamp_file, stamp.as_deref(), &builtin_carts) {
        log::info!(
            "the built-in carts in {} are this package's",
            builtin_carts.display()
        );
    } else {
        match copy_assets(app, &builtin_carts) {
            Ok(n) => {
                log::info!("{n} built-in carts in {}", builtin_carts.display());
                // Only after a whole copy: one cut short is done again.
                if let Some(stamp) = &stamp {
                    if let Err(e) = std::fs::write(&stamp_file, stamp) {
                        log::warn!("cannot write {}: {e}", stamp_file.display());
                    }
                }
            }
            Err(e) => {
                log::error!("cannot copy the built-in carts: {e}");
                let _ = std::fs::remove_file(&stamp_file);
            }
        }
    }
    log::info!("your own carts go in {}", own_carts.display());
    Some(Dirs {
        builtin_carts,
        own_carts,
        saves: internal.join("saves"),
        settings: internal.join(crate::settings::FILE),
    })
}

/// Copy every asset under `carts/` into `dest`, and remove what an earlier
/// version left there that this one does not have. The count copied.
fn copy_assets(app: &AndroidApp, dest: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(dest)?;
    let assets = app.asset_manager();
    let dir_name = CString::new(ASSET_DIR).expect("no NUL in a constant");
    let dir = assets
        .open_dir(&dir_name)
        .ok_or_else(|| std::io::Error::other("the APK has no carts directory"))?;
    let names: Vec<CString> = dir.collect();
    let mut copied = Vec::new();
    for name in names {
        let Ok(file_name) = name.to_str() else {
            log::warn!("an asset name is not UTF-8: {name:?}");
            continue;
        };
        // A name that is not one path component would leave `dest`.
        if file_name.is_empty() || file_name.contains(['/', '\\']) {
            continue;
        }
        let path = CString::new(format!("{ASSET_DIR}/{file_name}")).expect("no NUL in a name");
        let Some(mut asset) = assets.open(&path) else {
            log::warn!("cannot open asset {file_name}");
            continue;
        };
        let mut bytes = Vec::with_capacity(asset.length());
        asset.read_to_end(&mut bytes)?;
        std::fs::write(dest.join(file_name), &bytes)?;
        copied.push(file_name.to_string());
    }
    for entry in std::fs::read_dir(dest)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !copied.contains(&name) && entry.path().is_file() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(copied.len())
}
