//! The carts a shell lists from a directory: directories with a `main.lua`
//! and `.zip` or `.cart` files. Every host that runs the shell lists them
//! by this one rule, so a cart that shows on one shows on another.

use std::path::{Path, PathBuf};

use kuula_core::shell::CartEntry;
use kuula_core::zipsource::ZipSource;
use kuula_core::{CartSource, Fault, Manifest, Snapshot, SnapshotLimits};

/// Most carts a list shows.
pub const MAX_CARTS: usize = 256;

/// A listed cart and where it lives.
#[derive(Debug, Clone)]
pub struct Listed {
    pub entry: CartEntry,
    pub path: PathBuf,
}

/// Whether `path` is a `.zip` or `.cart` file: a packed cart rather than a
/// directory.
pub fn is_archive(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("zip") || e.eq_ignore_ascii_case("cart"))
}

/// The cart at `path`, as a snapshot of its files.
pub fn open_cart(path: &Path) -> Result<Snapshot, Fault> {
    let limits = SnapshotLimits::default();
    let snap = if is_archive(path) {
        Snapshot::from_zip(path, limits)
    } else {
        Snapshot::from_dir(path, limits)
    };
    snap.map_err(|e| Fault::new(Fault::CART_READ_ERROR, &e.path, None, e.message))
}

/// The manifest of the cart at `path`, when it has one that parses. Of a
/// packed cart only that one entry is read: a list of many carts does not
/// inflate every file of every one to show their titles.
fn manifest(path: &Path) -> Option<Manifest> {
    let text = if path.is_dir() {
        std::fs::read_to_string(path.join(kuula_core::manifest::MANIFEST_FILE)).ok()?
    } else {
        let zip = ZipSource::open(path, SnapshotLimits::default()).ok()?;
        String::from_utf8(zip.read(kuula_core::manifest::MANIFEST_FILE).ok()?).ok()?
    };
    Manifest::parse(&text).ok()
}

/// The cart at `path`, when it is one: a directory with a `main.lua`, or a
/// `.zip` or `.cart` file. Its title comes from `cart.toml` when that
/// parses, and is its file name otherwise. A file that is no usable
/// archive is still listed, and fails when it is opened, which the shell
/// shows.
pub fn listed(path: PathBuf) -> Option<Listed> {
    let name = path.file_name().and_then(|n| n.to_str())?.to_string();
    let is_dir_cart = path.is_dir() && path.join(kuula_core::console::MAIN_FILE).is_file();
    if !is_dir_cart && !is_archive(&path) {
        return None;
    }
    let metadata = manifest(&path).unwrap_or_default();
    let title = if metadata.title.is_empty() {
        name.clone()
    } else {
        metadata.title
    };
    Some(Listed {
        entry: CartEntry {
            name,
            title,
            author: metadata.author,
            license: metadata.license,
            network: metadata.services.iter().any(|s| s.as_str() == "net"),
        },
        path,
    })
}

/// The carts directly under `dir`, sorted by name, at most [`MAX_CARTS`].
/// A directory that is missing lists nothing.
pub fn list_dir(dir: &Path) -> Vec<Listed> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(listed)
        .take(MAX_CARTS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system's temporary one.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kuula-host-common-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn packed(files: &[(&str, &str)]) -> Vec<u8> {
        let snap = Snapshot::from_entries(
            files.iter().map(|(n, t)| (*n, t.as_bytes().to_vec())),
            SnapshotLimits::default(),
        )
        .unwrap();
        kuula_core::zipsource::pack(&snap).unwrap()
    }

    #[test]
    fn directories_with_a_main_and_cart_files_are_listed_by_name() {
        let root = scratch("list");
        let dir = root.join("b-dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.lua"), "function _update() end").unwrap();
        std::fs::write(dir.join("cart.toml"), "[cart]\ntitle = \"A Directory\"\n").unwrap();
        std::fs::write(
            root.join("a-packed.cart"),
            packed(&[
                ("main.lua", "function _update() end"),
                ("cart.toml", "[cart]\ntitle = \"Packed\"\n"),
            ]),
        )
        .unwrap();
        std::fs::write(
            root.join("c-untitled.zip"),
            packed(&[("main.lua", "function _update() end")]),
        )
        .unwrap();
        // Not carts: a file of another kind, a directory with no main.lua.
        std::fs::write(root.join("notes.txt"), "not a cart").unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();

        let all = list_dir(&root);
        let seen: Vec<(&str, &str)> = all
            .iter()
            .map(|l| (l.entry.name.as_str(), l.entry.title.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                ("a-packed.cart", "Packed"),
                ("b-dir", "A Directory"),
                ("c-untitled.zip", "c-untitled.zip"),
            ]
        );
        for cart in &all {
            assert!(open_cart(&cart.path).is_ok(), "{}", cart.path.display());
        }
        assert!(is_archive(&all[0].path) && !is_archive(&all[1].path));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_that_is_no_archive_is_listed_by_its_name_and_fails_when_opened() {
        let root = scratch("broken");
        std::fs::write(root.join("~Test~ Hello.cart"), b"nope").unwrap();
        let all = list_dir(&root);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].entry.name, "~Test~ Hello.cart");
        assert_eq!(all[0].entry.title, "~Test~ Hello.cart");
        let fault = open_cart(&all[0].path).unwrap_err();
        assert_eq!(fault.code, Fault::CART_READ_ERROR);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_directory_lists_nothing() {
        assert!(list_dir(&scratch("missing").join("not-there")).is_empty());
    }
}
