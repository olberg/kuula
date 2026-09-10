//! Save slots. A [`SaveStore`] holds eight
//! slots of bounded bytes; the cart never sees a path. [`MemoryStore`] is
//! the default and what headless runs use; [`FileStore`] writes atomically
//! under a runtime-owned directory keyed by a save identity the host
//! assigns, never by the manifest id.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Slots per cart, numbered 0 to `SLOT_COUNT - 1`.
pub const SLOT_COUNT: u8 = 8;
/// Most bytes one slot holds.
pub const SLOT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveError {
    /// Stable code: `save_slot`, `save_size` or `save_io`.
    pub code: &'static str,
    pub message: String,
}

impl SaveError {
    pub const SLOT: &'static str = "save_slot";
    pub const SIZE: &'static str = "save_size";
    pub const IO: &'static str = "save_io";

    pub fn new(code: &'static str, message: impl Into<String>) -> SaveError {
        SaveError {
            code,
            message: message.into(),
        }
    }

    fn io(what: &str, e: &std::io::Error) -> SaveError {
        SaveError::new(Self::IO, format!("{what}: {e}"))
    }
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for SaveError {}

/// Check a slot number and a payload size, shared by every store.
pub fn check_slot(slot: u8) -> Result<(), SaveError> {
    if slot < SLOT_COUNT {
        Ok(())
    } else {
        Err(SaveError::new(
            SaveError::SLOT,
            format!("slot {slot} is not 0 to {}", SLOT_COUNT - 1),
        ))
    }
}

pub fn check_size(len: usize) -> Result<(), SaveError> {
    if len <= SLOT_BYTES {
        Ok(())
    } else {
        Err(SaveError::new(
            SaveError::SIZE,
            format!("{len} bytes is more than a slot holds ({SLOT_BYTES})"),
        ))
    }
}

pub trait SaveStore {
    /// The bytes of a slot, or `None` when it has never been written.
    fn read(&mut self, slot: u8) -> Result<Option<Vec<u8>>, SaveError>;
    /// Replace a slot's bytes; durable when this returns.
    fn write(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError>;
    /// Failures the store kept from the cart (a [`WriteThroughStore`]'s
    /// disk errors), for the host to report; empty for stores that fail
    /// in the open.
    fn take_failures(&mut self) -> Vec<SaveError> {
        Vec::new()
    }
    /// Every slot that holds bytes, in slot order, for a transcript
    /// header or a worker's initial store. A slot that cannot be read is
    /// left out.
    fn all_slots(&mut self) -> Vec<(u8, Vec<u8>)> {
        (0..SLOT_COUNT)
            .filter_map(|slot| self.read(slot).ok().flatten().map(|b| (slot, b)))
            .collect()
    }
}

/// Slots that live and die with the console.
#[derive(Debug, Default, Clone)]
pub struct MemoryStore {
    slots: [Option<Vec<u8>>; SLOT_COUNT as usize],
}

impl MemoryStore {
    pub fn new() -> MemoryStore {
        MemoryStore::default()
    }

    /// A store holding `slots`; a bad slot number or an oversized entry
    /// is refused, so bytes from another process are checked here.
    pub fn from_slots(slots: &[(u8, Vec<u8>)]) -> Result<MemoryStore, SaveError> {
        let mut store = MemoryStore::new();
        for (slot, bytes) in slots {
            store.write(*slot, bytes)?;
        }
        Ok(store)
    }
}

/// Most disk failures a write-through store keeps before dropping the
/// oldest.
const MAX_KEPT_FAILURES: usize = 16;

/// Memory semantics for the cart, persistence behind it. Reads and writes
/// go to a [`MemoryStore`] seeded from `disk` when the store is built;
/// each write is then forwarded to `disk`, and a failure there is kept
/// for the host to report through `take_failures` rather than shown to
/// the cart. That makes a cart's save results the same on every desktop
/// path and in a replay: the only errors `save` can return are the slot
/// and size checks, which do not depend on the host.
///
/// A slot that could not be read when the store was built starts empty
/// for the cart, but its disk file is left alone: a write to it stays in
/// memory and is reported as a failure, so a locked or oversized slot
/// file is never replaced by the fresh game the cart started because it
/// saw nothing there.
pub struct WriteThroughStore {
    memory: MemoryStore,
    disk: Box<dyn SaveStore>,
    failures: Vec<SaveError>,
    /// Slots whose disk file failed to read; writes to them stay in memory.
    unreadable: [bool; SLOT_COUNT as usize],
}

impl WriteThroughStore {
    /// Seed from `disk`; a slot that cannot be read starts empty, the
    /// error is kept, and the slot is protected from later writes.
    pub fn new(mut disk: Box<dyn SaveStore>) -> WriteThroughStore {
        let mut memory = MemoryStore::new();
        let mut failures = Vec::new();
        let mut unreadable = [false; SLOT_COUNT as usize];
        for slot in 0..SLOT_COUNT {
            match disk.read(slot) {
                Ok(Some(bytes)) => memory.slots[slot as usize] = Some(bytes),
                Ok(None) => {}
                Err(e) => {
                    failures.push(e);
                    unreadable[slot as usize] = true;
                }
            }
        }
        WriteThroughStore {
            memory,
            disk,
            failures,
            unreadable,
        }
    }

    fn keep_failure(&mut self, e: SaveError) {
        if self.failures.len() >= MAX_KEPT_FAILURES {
            self.failures.remove(0);
        }
        self.failures.push(e);
    }
}

impl SaveStore for WriteThroughStore {
    fn read(&mut self, slot: u8) -> Result<Option<Vec<u8>>, SaveError> {
        self.memory.read(slot)
    }

    fn write(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError> {
        self.memory.write(slot, bytes)?;
        if self.unreadable[slot as usize] {
            self.keep_failure(SaveError::new(
                SaveError::IO,
                format!("slot {slot} could not be read at start; the write stays in memory"),
            ));
            return Ok(());
        }
        if let Err(e) = self.disk.write(slot, bytes) {
            self.keep_failure(e);
        }
        Ok(())
    }

    fn take_failures(&mut self) -> Vec<SaveError> {
        std::mem::take(&mut self.failures)
    }
}

impl SaveStore for MemoryStore {
    fn read(&mut self, slot: u8) -> Result<Option<Vec<u8>>, SaveError> {
        check_slot(slot)?;
        Ok(self.slots[slot as usize].clone())
    }

    fn write(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError> {
        check_slot(slot)?;
        check_size(bytes.len())?;
        self.slots[slot as usize] = Some(bytes.to_vec());
        Ok(())
    }
}

/// Whether an identity is a short `[a-z0-9_-]` string, 1 to 64 bytes.
pub fn valid_identity(identity: &str) -> bool {
    !identity.is_empty()
        && identity.len() <= 64
        && identity
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// The save identity of a cart opened from `cart_path`: FNV-1a 64 of the
/// canonical absolute path (or the path as given when it cannot be
/// canonicalised), as 16 hex digits. Two copies of the same cart in two
/// places have two identities; the manifest id plays no part.
pub fn save_identity(cart_path: &Path) -> String {
    let canonical = fs::canonicalize(cart_path).unwrap_or_else(|_| cart_path.to_path_buf());
    let text = canonical.to_string_lossy();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The runtime-owned save root: `KUULA_SAVE_ROOT` when set, else
/// `%LOCALAPPDATA%\kuula\saves` on Windows or `$XDG_DATA_HOME/kuula/saves`
/// (falling back to `~/.local/share`) elsewhere.
pub fn default_save_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("KUULA_SAVE_ROOT") {
        return Some(PathBuf::from(root));
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    base.map(|b| b.join("kuula").join("saves"))
}

/// Slots as files: `<root>/<identity>/slot<N>.kuula`, each write staged
/// in a temp file beside it, synced, renamed over the slot, then the
/// directory synced where the platform supports it.
#[derive(Debug, Clone)]
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    pub fn new(root: PathBuf, identity: &str) -> Result<FileStore, SaveError> {
        if !valid_identity(identity) {
            return Err(SaveError::new(
                SaveError::IO,
                format!("save identity {identity:?} is not a short [a-z0-9_-] string"),
            ));
        }
        Ok(FileStore {
            dir: root.join(identity),
        })
    }

    /// The directory this store writes into.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn slot_path(&self, slot: u8) -> PathBuf {
        self.dir.join(format!("slot{slot}.kuula"))
    }
}

impl SaveStore for FileStore {
    fn read(&mut self, slot: u8) -> Result<Option<Vec<u8>>, SaveError> {
        check_slot(slot)?;
        let path = self.slot_path(slot);
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SaveError::io("stat slot", &e)),
        };
        if meta.len() > SLOT_BYTES as u64 {
            return Err(SaveError::new(
                SaveError::SIZE,
                format!(
                    "slot file holds {} bytes, more than {SLOT_BYTES}",
                    meta.len()
                ),
            ));
        }
        fs::read(&path)
            .map(Some)
            .map_err(|e| SaveError::io("read slot", &e))
    }

    fn write(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError> {
        check_slot(slot)?;
        check_size(bytes.len())?;
        fs::create_dir_all(&self.dir).map_err(|e| SaveError::io("create save directory", &e))?;
        let final_path = self.slot_path(slot);
        let tmp_path = self.dir.join(format!("slot{slot}.kuula.tmp"));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)
                .map_err(|e| SaveError::io("open temp file", &e))?;
            file.write_all(bytes)
                .map_err(|e| SaveError::io("write temp file", &e))?;
            file.sync_all()
                .map_err(|e| SaveError::io("sync temp file", &e))?;
            drop(file);
            fs::rename(&tmp_path, &final_path)
                .map_err(|e| SaveError::io("rename over slot", &e))?;
            sync_dir(&self.dir);
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp_path);
        }
        result
    }
}

/// Directory fsync: meaningful on Unix, not available on Windows, where
/// the rename is already committed by the time it returns.
#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("kuula-save-{}-{n}", std::process::id()));
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn memory_store_round_trips_and_checks_bounds() {
        let mut m = MemoryStore::new();
        assert_eq!(m.read(0).unwrap(), None);
        m.write(0, b"abc").unwrap();
        assert_eq!(m.read(0).unwrap().as_deref(), Some(&b"abc"[..]));
        assert_eq!(m.read(7).unwrap(), None);
        assert_eq!(m.read(8).unwrap_err().code, "save_slot");
        assert_eq!(m.write(8, b"").unwrap_err().code, "save_slot");
        assert_eq!(
            m.write(1, &vec![0; SLOT_BYTES + 1]).unwrap_err().code,
            "save_size"
        );
        m.write(1, &vec![0; SLOT_BYTES]).unwrap();
    }

    #[test]
    fn file_store_writes_atomically_and_twice() {
        let d = TempDir::new();
        let mut s = FileStore::new(d.0.clone(), "abc-123_x").unwrap();
        assert_eq!(s.read(3).unwrap(), None);
        s.write(3, b"first").unwrap();
        s.write(3, b"second, longer").unwrap();
        assert_eq!(s.read(3).unwrap().unwrap(), b"second, longer");
        let names: Vec<String> = fs::read_dir(s.dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["slot3.kuula"], "no temp file left behind");
        assert_eq!(
            fs::read(d.0.join("abc-123_x").join("slot3.kuula")).unwrap(),
            b"second, longer"
        );
        // A second store on the same directory sees the same slot.
        let mut again = FileStore::new(d.0.clone(), "abc-123_x").unwrap();
        assert_eq!(again.read(3).unwrap().unwrap(), b"second, longer");
        assert_eq!(again.read(8).unwrap_err().code, "save_slot");
    }

    #[test]
    fn file_store_rejects_bad_identities_and_oversized_slots() {
        let d = TempDir::new();
        for bad in ["", "ABC", "a/b", "..", "a b", &"x".repeat(65)] {
            assert!(FileStore::new(d.0.clone(), bad).is_err(), "{bad:?}");
        }
        let mut s = FileStore::new(d.0.clone(), "id").unwrap();
        assert_eq!(
            s.write(0, &vec![0; SLOT_BYTES + 1]).unwrap_err().code,
            "save_size"
        );
        fs::create_dir_all(s.dir()).unwrap();
        fs::write(s.dir().join("slot0.kuula"), vec![0; SLOT_BYTES + 1]).unwrap();
        assert_eq!(s.read(0).unwrap_err().code, "save_size");
    }

    #[test]
    fn write_through_store_keeps_an_unreadable_slot_off_disk() {
        let d = TempDir::new();
        let mut disk = FileStore::new(d.0.clone(), "id").unwrap();
        disk.write(1, b"readable").unwrap();
        // Slot 0 on disk is oversized, so it cannot be read at start.
        fs::write(disk.dir().join("slot0.kuula"), vec![7; SLOT_BYTES + 1]).unwrap();
        let mut store = WriteThroughStore::new(Box::new(disk));
        let start = store.take_failures();
        assert_eq!(start.len(), 1);
        assert_eq!(start[0].code, "save_size");
        // The cart sees an empty slot 0 and the readable slot 1.
        assert_eq!(store.read(0).unwrap(), None);
        assert_eq!(store.read(1).unwrap().unwrap(), b"readable");
        // A save to slot 0 succeeds for the cart, stays in memory, and
        // is reported; the file the player had is untouched.
        store.write(0, b"fresh game").unwrap();
        assert_eq!(store.read(0).unwrap().unwrap(), b"fresh game");
        let kept = store.take_failures();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].code, "save_io");
        assert_eq!(
            fs::read(d.0.join("id").join("slot0.kuula")).unwrap().len(),
            SLOT_BYTES + 1
        );
        // A readable slot still writes through.
        store.write(1, b"still persisted").unwrap();
        assert!(store.take_failures().is_empty());
        assert_eq!(
            fs::read(d.0.join("id").join("slot1.kuula")).unwrap(),
            b"still persisted"
        );
        assert_eq!(store.all_slots().len(), 2);
    }

    #[test]
    fn identity_is_stable_hex_of_the_canonical_path() {
        let d = TempDir::new();
        let a = save_identity(&d.0);
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert!(valid_identity(&a));
        let relative = d.0.join("..").join(d.0.file_name().unwrap());
        assert_eq!(save_identity(&relative), a, "canonicalised first");
        assert_ne!(save_identity(&d.0.join("other")), a);
        assert_eq!(save_identity(Path::new("does/not/exist")).len(), 16);
    }
}
