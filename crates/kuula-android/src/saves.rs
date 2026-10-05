//! Where a cart's saves go: a directory per cart under the app's internal
//! data, named by the cart's identity (a hash of its path, as on the
//! desktop). A store that cannot be made leaves the saves in memory, for
//! this run, and says so.

use std::path::Path;

use kuula_core::{FileStore, MemoryStore, SaveStore, WriteThroughStore};

/// The save store for the cart at `cart_path`, under `root`.
pub fn store(root: &Path, cart_path: &Path) -> Box<dyn SaveStore> {
    let identity = kuula_core::save::save_identity(cart_path);
    match FileStore::new(root.to_path_buf(), &identity) {
        Ok(store) => Box::new(WriteThroughStore::new(Box::new(store))),
        Err(e) => {
            log::warn!("saves stay in memory: {e}");
            Box::new(MemoryStore::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_written_is_read_back_by_a_new_store_for_the_same_cart() {
        let root = crate::testdir::new("saves");
        let cart = root.join("carts").join("one.cart");
        let saves = root.join("saves");
        let mut first = store(&saves, &cart);
        first.write(0, b"progress").unwrap();
        let mut second = store(&saves, &cart);
        assert_eq!(second.read(0).unwrap().as_deref(), Some(&b"progress"[..]));
        let mut other = store(&saves, &root.join("carts").join("two.cart"));
        assert_eq!(other.read(0).unwrap(), None);
    }
}
