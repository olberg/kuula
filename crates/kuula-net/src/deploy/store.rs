//! The installation's deploy files, under `<data>/deploy/`: the
//! development key and the receiver's list of approved developers. Plain
//! files the host owns; no cart, worker or `sys` call reaches them.
//!
//! - `key`: 32 raw bytes, the Ed25519 secret behind the installation's
//!   development endpoint id. Created on first use with owner-only
//!   permissions where the OS has them. Deleting it resets the identity.
//! - `approved.kv`: approved developer endpoint ids, one per line in
//!   hex, at most 32. A line that does not parse as an id is skipped.

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};

use iroh::{EndpointId, SecretKey};

/// Most approved developers.
pub const MAX_APPROVED: usize = 32;

/// Largest approved list read; far above 32 ids.
const MAX_FILE: u64 = 16 * 1024;

/// Why a store call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The text is not an endpoint id.
    BadId(String),
    /// The approved list already holds [`MAX_APPROVED`] ids.
    Full,
    /// A file could not be read or written.
    Io(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::BadId(why) => f.write_str(why),
            StoreError::Full => write!(f, "the approved list is full ({MAX_APPROVED} ids)"),
            StoreError::Io(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for StoreError {}

fn io(path: &Path, what: &str, e: &std::io::Error) -> StoreError {
    StoreError::Io(format!("cannot {what} {}: {e}", path.display()))
}

/// An endpoint id in its canonical lower-case hex form.
pub fn parse_id(text: &str) -> Result<String, StoreError> {
    EndpointId::from_str(&text.trim().to_ascii_lowercase())
        .map(|id| id.to_string())
        .map_err(|_| StoreError::BadId("not an endpoint id (64 hex characters)".into()))
}

/// The ids in an approved list: unparsable lines are skipped, repeats
/// kept once, and at most [`MAX_APPROVED`] are taken.
pub fn parse_approved(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let Ok(id) = parse_id(line) else { continue };
        if !out.contains(&id) && out.len() < MAX_APPROVED {
            out.push(id);
        }
    }
    out
}

/// The deploy directory of one installation.
#[derive(Debug, Clone)]
pub struct DeployStore {
    dir: PathBuf,
}

impl DeployStore {
    /// The store under `<data>/deploy`, where `<data>` is the directory
    /// `settings.kv` lives in.
    pub fn new(data: &Path) -> DeployStore {
        DeployStore {
            dir: data.join("deploy"),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn key_path(&self) -> PathBuf {
        self.dir.join("key")
    }

    fn approved_path(&self) -> PathBuf {
        self.dir.join("approved.kv")
    }

    /// The development key, created on first use.
    pub fn secret_key(&self) -> Result<SecretKey, StoreError> {
        let path = self.key_path();
        match read_key(&path) {
            Ok(Some(key)) => return Ok(key),
            Ok(None) => {}
            Err(e) => return Err(e),
        }
        create_dir(&self.dir)?;
        let key = SecretKey::generate();
        match create_private(&path, &key.to_bytes()) {
            Ok(()) => Ok(key),
            // Another process created it first: use theirs.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => read_key(&path)?
                .ok_or_else(|| StoreError::Io(format!("{} vanished", path.display()))),
            Err(e) => Err(io(&path, "write", &e)),
        }
    }

    /// The development endpoint id: what a receiver's pairing names.
    pub fn id(&self) -> Result<String, StoreError> {
        Ok(self.secret_key()?.public().to_string())
    }

    /// The approved ids; empty when the file is missing or unreadable.
    pub fn approved(&self) -> Vec<String> {
        let path = self.approved_path();
        let Ok(file) = fs::File::open(&path) else {
            return Vec::new();
        };
        let mut text = String::new();
        // A file over the cap, or one that is not UTF-8, approves nobody.
        if file.take(MAX_FILE + 1).read_to_string(&mut text).is_err()
            || text.len() as u64 > MAX_FILE
        {
            return Vec::new();
        }
        parse_approved(&text)
    }

    /// Whether `id` (canonical hex) is approved.
    pub fn is_approved(&self, id: &str) -> bool {
        self.approved().iter().any(|a| a == id)
    }

    /// Approve an id; `Ok(false)` when it already was.
    pub fn approve(&self, id: &str) -> Result<bool, StoreError> {
        let id = parse_id(id)?;
        let mut list = self.approved();
        if list.contains(&id) {
            return Ok(false);
        }
        if list.len() >= MAX_APPROVED {
            return Err(StoreError::Full);
        }
        list.push(id);
        self.write_approved(&list)?;
        Ok(true)
    }

    /// Revoke an id; `Ok(false)` when it was not approved.
    pub fn revoke(&self, id: &str) -> Result<bool, StoreError> {
        let id = parse_id(id)?;
        let mut list = self.approved();
        let before = list.len();
        list.retain(|a| *a != id);
        if list.len() == before {
            return Ok(false);
        }
        self.write_approved(&list)?;
        Ok(true)
    }

    fn write_approved(&self, list: &[String]) -> Result<(), StoreError> {
        create_dir(&self.dir)?;
        let path = self.approved_path();
        let tmp = self.dir.join("approved.kv.tmp");
        let mut text = list.join("\n");
        if !text.is_empty() {
            text.push('\n');
        }
        fs::write(&tmp, text).map_err(|e| io(&tmp, "write", &e))?;
        fs::rename(&tmp, &path).map_err(|e| io(&path, "replace", &e))
    }
}

fn create_dir(dir: &Path) -> Result<(), StoreError> {
    fs::create_dir_all(dir).map_err(|e| io(dir, "create", &e))
}

/// The key, `None` when there is no file; a file of the wrong size is an
/// error rather than a silent new identity.
fn read_key(path: &Path) -> Result<Option<SecretKey>, StoreError> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(path, "read", &e)),
    };
    let mut bytes = Vec::new();
    file.take(33)
        .read_to_end(&mut bytes)
        .map_err(|e| io(path, "read", &e))?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
        StoreError::Io(format!(
            "{} is not a 32-byte key; delete it to start a new identity",
            path.display()
        ))
    })?;
    Ok(Some(SecretKey::from_bytes(&bytes)))
}

/// Create `path` holding `bytes`, readable by its owner only, failing
/// with `AlreadyExists` if it exists. The bytes go into a file of their
/// own first, which is then linked to `path`: a second process starting
/// at the same moment finds the key whole or not at all, never
/// half-written. A file system without hard links (FAT) gets the key
/// written in place, as the last resort.
fn create_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let part = path.with_file_name(format!(
        "key.{}.{}.part",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    write_private(&part, bytes)?;
    let linked = fs::hard_link(&part, path);
    let _ = fs::remove_file(&part);
    match linked {
        Err(e) if e.kind() != ErrorKind::AlreadyExists => write_private(path, bytes),
        other => other,
    }
}

/// Write a new file readable by its owner only, failing if it exists.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kuula-store-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn id(n: u8) -> String {
        SecretKey::from_bytes(&[n; 32]).public().to_string()
    }

    #[test]
    fn the_key_is_created_once_and_deleting_it_resets_the_identity() {
        let data = scratch("key");
        let store = DeployStore::new(&data);
        let first = store.id().unwrap();
        assert_eq!(store.id().unwrap(), first);
        assert_eq!(fs::read(store.dir().join("key")).unwrap().len(), 32);
        assert_eq!(first.len(), 64);
        fs::remove_file(store.dir().join("key")).unwrap();
        assert_ne!(store.id().unwrap(), first);
        // A damaged key is an error, not a new identity.
        fs::write(store.dir().join("key"), b"short").unwrap();
        assert!(store
            .secret_key()
            .unwrap_err()
            .to_string()
            .contains("32-byte"));
        let _ = fs::remove_dir_all(&data);
    }

    /// Processes that start together on a fresh installation agree on one
    /// key, and none of them reads it half-written.
    #[test]
    fn a_key_created_by_many_at_once_is_one_key_read_whole() {
        const THREADS: usize = 16;
        for round in 0..20 {
            let data = scratch(&format!("race{round}"));
            let store = DeployStore::new(&data);
            let start = std::sync::Barrier::new(THREADS);
            let keys: Vec<Result<String, StoreError>> = std::thread::scope(|s| {
                let asked: Vec<_> = (0..THREADS)
                    .map(|_| {
                        s.spawn(|| {
                            start.wait();
                            store.id()
                        })
                    })
                    .collect();
                asked.into_iter().map(|t| t.join().unwrap()).collect()
            });
            let first = keys[0].clone().expect("the first reader got a key");
            assert!(keys.iter().all(|k| k.as_ref() == Ok(&first)), "{keys:?}");
            assert_eq!(store.id().unwrap(), first);
            // Only the key is left: every writer removed its own file.
            let left: Vec<String> = fs::read_dir(store.dir())
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(left, ["key"]);
            let _ = fs::remove_dir_all(&data);
        }
    }

    #[test]
    fn approve_revoke_and_the_bound() {
        let data = scratch("approved");
        let store = DeployStore::new(&data);
        assert!(store.approved().is_empty());
        assert!(store.approve(&id(1)).unwrap());
        assert!(!store.approve(&id(1)).unwrap());
        assert!(store.is_approved(&id(1)) && !store.is_approved(&id(2)));
        assert!(store.revoke(&id(1)).unwrap());
        assert!(!store.revoke(&id(1)).unwrap());
        assert!(store.approved().is_empty());
        for n in 1..=MAX_APPROVED as u8 {
            store.approve(&id(n)).unwrap();
        }
        assert_eq!(store.approve(&id(200)), Err(StoreError::Full));
        assert_eq!(store.approved().len(), MAX_APPROVED);
        assert!(matches!(
            store.approve("nonsense"),
            Err(StoreError::BadId(_))
        ));
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn unparsable_lines_are_skipped_and_the_list_is_bounded() {
        let a = id(1);
        let text = format!("garbage\n\n{a}\n# comment\n{a}\n123\n{}\n", id(2));
        assert_eq!(parse_approved(&text), [a, id(2)]);
        let many: String = (0..100u8).map(|n| format!("{}\n", id(n))).collect();
        assert_eq!(parse_approved(&many).len(), MAX_APPROVED);
        // Hex in either case names the same id.
        let upper = id(3).to_uppercase();
        assert_eq!(parse_id(&upper).unwrap(), id(3));
    }
}
