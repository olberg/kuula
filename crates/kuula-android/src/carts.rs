//! The carts the shell lists. They come from two directories: the app's
//! own (the built-in carts, copied from the assets when the package
//! changes) and the one a person puts their own carts in. A name present
//! in both is listed once, the person's own winning. What is a cart, and
//! how it is opened, is the rule every host shares
//! (`kuula_host_common::carts`).

use std::path::Path;

use kuula_host_common::carts::list_dir;
pub use kuula_host_common::carts::{open_cart, Listed, MAX_CARTS};

/// The longest cart name the app takes from outside itself.
pub const MAX_PLAIN_NAME: usize = 64;

/// Whether `name` is one plain file name of letters, digits, `.`, `_` and
/// `-`: nothing that is a path, and nothing a line of the log could not
/// carry. What an intent may name a cart by.
pub fn is_plain_name(name: &str) -> bool {
    (1..=MAX_PLAIN_NAME).contains(&name.len())
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Whether the built-in carts in `dir` are the ones of the package that is
/// installed now: the stamp written after the last copy, in the file at
/// `stamp_file`, is this package's `stamp`. With no stamp to compare (the
/// package could not be asked) they are taken to be stale, and copied.
pub fn builtin_up_to_date(stamp_file: &Path, stamp: Option<&str>, dir: &Path) -> bool {
    stamp.is_some_and(|stamp| {
        dir.is_dir() && std::fs::read_to_string(stamp_file).is_ok_and(|have| have == stamp)
    })
}

/// The carts of `own` (a person's) and `builtin` (the app's), sorted by
/// name, a name in both taken from `own`, at most [`MAX_CARTS`].
pub fn list(own: &Path, builtin: &Path) -> Vec<Listed> {
    let mut all = list_dir(own);
    for cart in list_dir(builtin) {
        if !all.iter().any(|l| l.entry.name == cart.entry.name) {
            all.push(cart);
        }
    }
    all.sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
    all.truncate(MAX_CARTS);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_name_is_one_file_name_and_nothing_else() {
        for name in ["hello.cart", "alien-invaders.cart", "a", "My_Cart.2.zip"] {
            assert!(is_plain_name(name), "{name}");
        }
        let long = "a".repeat(MAX_PLAIN_NAME + 1);
        for name in [
            "",
            ".",
            "..",
            ".hidden",
            "a/b.cart",
            "a\\b.cart",
            "two words.cart",
            "new\nline",
            "x;rm",
            "caf\u{e9}.cart",
            long.as_str(),
        ] {
            assert!(!is_plain_name(name), "{name:?}");
        }
    }

    #[test]
    fn built_in_carts_are_stale_until_the_stamp_is_this_packages() {
        let root = crate::testdir::new("stamp");
        let (dir, file) = (root.join("carts"), root.join("carts.stamp"));
        let stamp = "/data/app/x/base.apk|10455512|1790000000";
        // Nothing copied yet, and no stamp.
        assert!(!builtin_up_to_date(&file, Some(stamp), &dir));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!builtin_up_to_date(&file, Some(stamp), &dir));
        std::fs::write(&file, stamp).unwrap();
        assert!(builtin_up_to_date(&file, Some(stamp), &dir));
        // Another package (a reinstall of the same version has another
        // path and time), a package that could not be asked, a directory
        // that is gone: copied again.
        assert!(!builtin_up_to_date(
            &file,
            Some("/data/app/y/base.apk|10455512|1790000001"),
            &dir
        ));
        assert!(!builtin_up_to_date(&file, None, &dir));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!builtin_up_to_date(&file, Some(stamp), &dir));
    }

    fn dir_cart(root: &Path, name: &str, title: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.lua"), "function _update() end").unwrap();
        std::fs::write(
            dir.join("cart.toml"),
            format!("[cart]\ntitle = \"{title}\"\n"),
        )
        .unwrap();
    }

    #[test]
    fn a_name_in_both_directories_is_listed_once_and_the_persons_own_wins() {
        let root = crate::testdir::new("carts");
        let (own, builtin) = (root.join("own"), root.join("builtin"));
        dir_cart(&builtin, "shared", "Built in");
        dir_cart(&builtin, "only-builtin", "Only built in");
        dir_cart(&own, "shared", "Mine");
        dir_cart(&own, "only-own", "Only mine");
        std::fs::write(own.join("notes.txt"), "not a cart").unwrap();
        std::fs::create_dir_all(own.join("empty")).unwrap();

        let all = list(&own, &builtin);
        let seen: Vec<(&str, &str)> = all
            .iter()
            .map(|l| (l.entry.name.as_str(), l.entry.title.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                ("only-builtin", "Only built in"),
                ("only-own", "Only mine"),
                ("shared", "Mine"),
            ]
        );
        assert!(all[2].path.starts_with(&own));
    }

    #[test]
    fn a_missing_directory_lists_nothing() {
        let root = crate::testdir::new("nocarts");
        assert!(list(&root.join("a"), &root.join("b")).is_empty());
    }
}
