//! A packed cart: a zip archive read through a pure-Rust decoder
//!. The index is validated when the
//! archive is opened, with the same entry-name rules as the snapshot,
//! and every entry's declared size is held against the snapshot limits
//! before a byte is decompressed. Only stored and deflated entries are
//! accepted; nothing is ever extracted to disk.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use zip::{CompressionMethod, ZipArchive};

use crate::snapshot::{name_problem, SnapshotLimits};
use crate::source::{CartSource, SourceError};

#[derive(Debug, Clone, Copy)]
struct Entry {
    index: usize,
    size: u64,
}

/// A cart in a zip file. Entries are read from the archive on demand;
/// the central directory is parsed once, at [`ZipSource::open`].
pub struct ZipSource {
    path: PathBuf,
    archive: RefCell<ZipArchive<BufReader<File>>>,
    entries: BTreeMap<String, Entry>,
    limits: SnapshotLimits,
    total_bytes: u64,
}

impl std::fmt::Debug for ZipSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZipSource")
            .field("path", &self.path)
            .field("entries", &self.entries)
            .field("limits", &self.limits)
            .field("total_bytes", &self.total_bytes)
            .finish()
    }
}

fn zip_error(path: &str, e: zip::result::ZipError) -> SourceError {
    match e {
        zip::result::ZipError::Io(e) => SourceError::io(path, &e),
        zip::result::ZipError::FileNotFound => SourceError::not_found(path),
        other => SourceError::invalid_path(path, &format!("not a usable zip archive: {other}")),
    }
}

impl ZipSource {
    /// Open and index the archive. Every entry name is checked, every
    /// declared size is bounded, and the archive is refused as a whole
    /// when any entry breaks a rule.
    pub fn open(path: &Path, limits: SnapshotLimits) -> Result<ZipSource, SourceError> {
        let file = File::open(path).map_err(|e| SourceError::io("", &e))?;
        let mut archive = ZipArchive::new(BufReader::new(file)).map_err(|e| zip_error("", e))?;
        if archive.len() > limits.max_files {
            return Err(SourceError::limit(
                "",
                format!("archive has more than {} entries", limits.max_files),
            ));
        }
        let mut entries = BTreeMap::new();
        let mut folded = HashSet::new();
        let mut total_bytes: u64 = 0;
        for index in 0..archive.len() {
            let entry = archive
                .by_index_raw(index)
                .map_err(|e| zip_error(&format!("entry {index}"), e))?;
            let Ok(name) = std::str::from_utf8(entry.name_raw()) else {
                return Err(SourceError::invalid_path(
                    &format!("entry {index}"),
                    "entry name is not UTF-8",
                ));
            };
            let name = name.to_string();
            if entry.is_dir() {
                continue;
            }
            if let Some(why) = name_problem(&name) {
                return Err(SourceError::invalid_path(&name, why));
            }
            if !folded.insert(name.to_ascii_lowercase()) {
                return Err(SourceError::invalid_path(
                    &name,
                    "duplicate entry, or collides with another when case is folded",
                ));
            }
            if entry.encrypted() {
                return Err(SourceError::invalid_path(
                    &name,
                    "encrypted entries are not allowed",
                ));
            }
            if entry.is_symlink() {
                return Err(SourceError::invalid_path(
                    &name,
                    "symlink entries are not allowed",
                ));
            }
            match entry.compression() {
                CompressionMethod::Stored | CompressionMethod::Deflated => {}
                other => {
                    return Err(SourceError::invalid_path(
                        &name,
                        &format!("compression {other} is not supported; use stored or deflate"),
                    ))
                }
            }
            let size = entry.size();
            if size > limits.max_file_bytes as u64 {
                return Err(SourceError::limit(
                    &name,
                    format!("entry is larger than {} bytes", limits.max_file_bytes),
                ));
            }
            total_bytes = total_bytes.saturating_add(size);
            if total_bytes > limits.max_total_bytes as u64 {
                return Err(SourceError::limit(
                    &name,
                    format!("archive is larger than {} bytes", limits.max_total_bytes),
                ));
            }
            entries.insert(name, Entry { index, size });
        }
        Ok(ZipSource {
            path: path.to_path_buf(),
            archive: RefCell::new(archive),
            entries,
            limits,
            total_bytes,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Entry names in sorted order, directories excluded.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Declared uncompressed bytes of every entry together.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn limits(&self) -> SnapshotLimits {
        self.limits
    }
}

/// Pack a snapshot as a zip: entries in name order, deflate at a fixed
/// level, the 1980-01-01 timestamp on every one, no extra fields. The
/// same snapshot always yields the same bytes.
pub fn pack(snapshot: &crate::snapshot::Snapshot) -> Result<Vec<u8>, SourceError> {
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
        .last_modified_time(zip::DateTime::default());
    for (name, bytes) in snapshot.entries() {
        w.start_file(name, options)
            .and_then(|()| w.write_all(bytes).map_err(zip::result::ZipError::Io))
            .map_err(|e| zip_error(name, e))?;
    }
    w.finish()
        .map(|c| c.into_inner())
        .map_err(|e| zip_error("", e))
}

impl CartSource for ZipSource {
    fn read(&self, path: &str) -> Result<Vec<u8>, SourceError> {
        let Some(entry) = self.entries.get(path) else {
            return Err(SourceError::not_found(path));
        };
        let mut archive = self.archive.borrow_mut();
        let mut zipped = archive
            .by_index(entry.index)
            .map_err(|e| zip_error(path, e))?;
        if zipped.name_raw() != path.as_bytes() {
            return Err(SourceError::invalid_path(
                path,
                "archive changed since it was indexed",
            ));
        }
        // The declared size was bounded at open; reading one byte past it
        // catches an entry that inflates to more than it claims.
        let mut bytes = Vec::with_capacity(entry.size as usize);
        zipped
            .by_ref()
            .take(entry.size + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| SourceError::io(path, &e))?;
        if bytes.len() as u64 != entry.size {
            return Err(SourceError::limit(
                path,
                "entry does not inflate to its declared size",
            ));
        }
        Ok(bytes)
    }

    fn exists(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("kuula-zip-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Write a zip with the given entries, deflated, and return its path.
    pub(crate) fn make_zip(dir: &Path, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join(name);
        let mut w = ZipWriter::new(File::create(&path).unwrap());
        for (n, bytes) in entries {
            w.start_file(
                *n,
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap();
        path
    }

    #[test]
    fn reads_entries_and_reports_missing_ones() {
        let d = TempDir::new();
        let p = make_zip(
            &d.0,
            "cart.zip",
            &[("main.lua", b"print(1)"), ("src/util.lua", b"return {}")],
        );
        let z = ZipSource::open(&p, SnapshotLimits::default()).unwrap();
        assert_eq!(z.names().collect::<Vec<_>>(), ["main.lua", "src/util.lua"]);
        assert_eq!(z.read("main.lua").unwrap(), b"print(1)");
        assert_eq!(z.read("src/util.lua").unwrap(), b"return {}");
        assert_eq!(z.read("nope.lua").unwrap_err().code, "not_found");
        assert!(z.exists("main.lua"));
        assert!(!z.exists("src\\util.lua"));
        assert_eq!(z.total_bytes(), 17);
    }

    #[test]
    fn traversal_and_bad_names_are_refused() {
        let d = TempDir::new();
        for (i, bad) in [
            "../main.lua",
            "src/../../x.lua",
            "/main.lua",
            "src\\a.lua",
            "src/con.lua",
            "src/a.lua ",
            "src/a:b.lua",
        ]
        .iter()
        .enumerate()
        {
            let p = make_zip(
                &d.0,
                &format!("bad{i}.zip"),
                &[("main.lua", b""), (bad, b"x")],
            );
            let e = ZipSource::open(&p, SnapshotLimits::default()).unwrap_err();
            assert_eq!(e.code, "invalid_path", "{bad}");
            assert_eq!(e.path, *bad);
        }
    }

    #[test]
    fn duplicate_and_case_colliding_names_are_refused() {
        let d = TempDir::new();
        let p = make_zip(
            &d.0,
            "dup.zip",
            &[("main.lua", b"a"), ("src/A.lua", b"b"), ("src/a.lua", b"c")],
        );
        let e = ZipSource::open(&p, SnapshotLimits::default()).unwrap_err();
        assert_eq!(e.code, "invalid_path");
        assert_eq!(e.path, "src/a.lua");
    }

    #[test]
    fn declared_sizes_are_bounded_before_reading() {
        let d = TempDir::new();
        let p = make_zip(&d.0, "big.zip", &[("main.lua", &[0u8; 100])]);
        let small = SnapshotLimits {
            max_files: 4,
            max_file_bytes: 50,
            max_total_bytes: 1000,
        };
        assert_eq!(
            ZipSource::open(&p, small).unwrap_err().code,
            "limit_exceeded"
        );
        let p = make_zip(
            &d.0,
            "many.zip",
            &[("a.lua", b""), ("b.lua", b""), ("c.lua", b"")],
        );
        let two = SnapshotLimits {
            max_files: 2,
            ..SnapshotLimits::default()
        };
        assert_eq!(ZipSource::open(&p, two).unwrap_err().code, "limit_exceeded");
        let p = make_zip(
            &d.0,
            "total.zip",
            &[("a.lua", &[0; 30]), ("b.lua", &[0; 30])],
        );
        let total = SnapshotLimits {
            max_files: 4,
            max_file_bytes: 50,
            max_total_bytes: 50,
        };
        assert_eq!(
            ZipSource::open(&p, total).unwrap_err().code,
            "limit_exceeded"
        );
    }

    #[test]
    fn pack_is_deterministic_and_round_trips() {
        use crate::snapshot::Snapshot;
        let snap = Snapshot::from_entries(
            [
                ("src/b.lua", b"return 2".to_vec()),
                ("main.lua", b"print('hi')".to_vec()),
                ("gfx/t.png", vec![7; 3000]),
            ],
            SnapshotLimits::default(),
        )
        .unwrap();
        let a = pack(&snap).unwrap();
        let b = pack(&snap).unwrap();
        assert_eq!(a, b, "same snapshot, same bytes");
        let d = TempDir::new();
        let p = d.0.join("packed.zip");
        std::fs::write(&p, &a).unwrap();
        let z = ZipSource::open(&p, SnapshotLimits::default()).unwrap();
        assert_eq!(
            z.names().collect::<Vec<_>>(),
            ["gfx/t.png", "main.lua", "src/b.lua"]
        );
        assert_eq!(z.read("gfx/t.png").unwrap(), vec![7; 3000]);
        let back = Snapshot::from_zip(&p, SnapshotLimits::default()).unwrap();
        assert_eq!(back, snap);
    }

    #[test]
    fn not_a_zip_is_an_error_not_a_panic() {
        let d = TempDir::new();
        let p = d.0.join("text.zip");
        std::fs::write(&p, b"this is not a zip").unwrap();
        let e = ZipSource::open(&p, SnapshotLimits::default()).unwrap_err();
        assert!(e.code == "invalid_path" || e.code == "io_error", "{e}");
        let e = ZipSource::open(&d.0.join("missing.zip"), SnapshotLimits::default()).unwrap_err();
        assert_eq!(e.code, "not_found");
    }
}
