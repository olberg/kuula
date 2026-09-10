//! A bounded, immutable snapshot of a cart. The
//! console only ever reads from a snapshot, so an edit to the directory
//! after `run` cannot reach a live cart, and every limit is applied while
//! the snapshot is built rather than at each read.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::source::{CartSource, SourceError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotLimits {
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
}

impl SnapshotLimits {
    /// The limits every cart is held to; `Default` returns the same.
    pub const DEFAULT: SnapshotLimits = SnapshotLimits {
        max_files: 4096,
        max_file_bytes: 16 * 1024 * 1024,
        max_total_bytes: 64 * 1024 * 1024,
    };
}

impl Default for SnapshotLimits {
    fn default() -> SnapshotLimits {
        SnapshotLimits::DEFAULT
    }
}

/// Portable entry names mapped to their bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    files: BTreeMap<String, Vec<u8>>,
    total_bytes: usize,
}

/// Which entries a snapshot takes from a cart directory: the served
/// roots only. Everything else in the directory is ignored.
pub fn is_served(name: &str) -> bool {
    match name {
        "main.lua" | "cart.toml" => true,
        _ => {
            if let Some(rest) = name.strip_prefix("src/") {
                rest.ends_with(".lua")
            } else if let Some(rest) = name.strip_prefix("gfx/") {
                rest.ends_with(".png") && !rest.contains('/')
            } else if let Some(rest) = name.strip_prefix("map/") {
                rest.ends_with(".json") && !rest.contains('/')
            } else if let Some(rest) = name
                .strip_prefix("sfx/")
                .or_else(|| name.strip_prefix("music/"))
            {
                rest.ends_with(".trk") && !rest.contains('/')
            } else if let Some(rest) = name.strip_prefix("samples/") {
                rest.ends_with(".wav") && !rest.contains('/')
            } else {
                false
            }
        }
    }
}

/// Reserved device names on Windows, matched case-insensitively on the
/// stem before any extension.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Why a name is not a portable entry name, or `None` if it is.
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("empty name");
    }
    if name.len() > 255 {
        return Some("name longer than 255 bytes");
    }
    if name.starts_with('/') {
        return Some("absolute path");
    }
    for piece in name.split('/') {
        if piece.is_empty() {
            return Some("empty path component");
        }
        if piece == "." || piece == ".." {
            return Some("'.' or '..' component");
        }
        if piece.ends_with('.') || piece.ends_with(' ') {
            return Some("component ends with a dot or space");
        }
        if piece.starts_with(' ') {
            return Some("component starts with a space");
        }
        for c in piece.chars() {
            if c.is_control() {
                return Some("control character in name");
            }
            if matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                return Some("character not allowed on every platform");
            }
        }
        let stem = piece.split('.').next().unwrap_or(piece);
        if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
            return Some("reserved device name");
        }
    }
    None
}

impl Snapshot {
    pub fn empty() -> Snapshot {
        Snapshot::default()
    }

    /// Build from in-memory entries, applying the name rules and limits.
    pub fn from_entries<I, S>(entries: I, limits: SnapshotLimits) -> Result<Snapshot, SourceError>
    where
        I: IntoIterator<Item = (S, Vec<u8>)>,
        S: AsRef<str>,
    {
        let mut snap = Snapshot::default();
        let mut folded = std::collections::HashSet::new();
        for (name, bytes) in entries {
            let name = name.as_ref();
            snap.add(name, bytes, &limits, &mut folded)?;
        }
        Ok(snap)
    }

    fn add(
        &mut self,
        name: &str,
        bytes: Vec<u8>,
        limits: &SnapshotLimits,
        folded: &mut std::collections::HashSet<String>,
    ) -> Result<(), SourceError> {
        if let Some(why) = name_problem(name) {
            return Err(SourceError::invalid_path(name, why));
        }
        if !folded.insert(name.to_ascii_lowercase()) {
            return Err(SourceError::invalid_path(
                name,
                "collides with another entry when case is folded",
            ));
        }
        if self.files.len() >= limits.max_files {
            return Err(SourceError::limit(
                name,
                format!("cart has more than {} files", limits.max_files),
            ));
        }
        if bytes.len() > limits.max_file_bytes {
            return Err(SourceError::limit(
                name,
                format!("file is larger than {} bytes", limits.max_file_bytes),
            ));
        }
        let total = self.total_bytes.saturating_add(bytes.len());
        if total > limits.max_total_bytes {
            return Err(SourceError::limit(
                name,
                format!("cart is larger than {} bytes", limits.max_total_bytes),
            ));
        }
        self.total_bytes = total;
        self.files.insert(name.to_string(), bytes);
        Ok(())
    }

    /// Ingest the served entries under `root`. Symlinks and other reparse
    /// points, including directories, are refused rather than followed.
    pub fn from_dir(root: &Path, limits: SnapshotLimits) -> Result<Snapshot, SourceError> {
        let mut snap = Snapshot::default();
        let mut folded = std::collections::HashSet::new();
        let meta = fs::symlink_metadata(root).map_err(|e| SourceError::io("", &e))?;
        if !meta.is_dir() || is_reparse(&meta) {
            return Err(SourceError::invalid_path(
                "",
                "cart root must be a plain directory",
            ));
        }
        let mut stack = vec![(root.to_path_buf(), String::new())];
        let mut visited = 0usize;
        while let Some((dir, prefix)) = stack.pop() {
            let entries = fs::read_dir(&dir).map_err(|e| SourceError::io(&prefix, &e))?;
            for entry in entries {
                let entry = entry.map_err(|e| SourceError::io(&prefix, &e))?;
                visited += 1;
                if visited > limits.max_files * 4 {
                    return Err(SourceError::limit(
                        &prefix,
                        "cart directory has too many entries",
                    ));
                }
                let file_name = entry.file_name();
                let Some(file_name) = file_name.to_str() else {
                    return Err(SourceError::invalid_path(&prefix, "name is not UTF-8"));
                };
                if file_name.starts_with('.') {
                    continue;
                }
                let name = if prefix.is_empty() {
                    file_name.to_string()
                } else {
                    format!("{prefix}/{file_name}")
                };
                let meta =
                    fs::symlink_metadata(entry.path()).map_err(|e| SourceError::io(&name, &e))?;
                if meta.file_type().is_symlink() || is_reparse(&meta) {
                    return Err(SourceError::invalid_path(
                        &name,
                        "symlinks and reparse points are not allowed in a cart",
                    ));
                }
                if meta.is_dir() {
                    if name_problem(&name).is_none() && could_serve_under(&name) {
                        stack.push((entry.path(), name));
                    }
                    continue;
                }
                if !meta.is_file() || !is_served(&name) {
                    continue;
                }
                if meta.len() > limits.max_file_bytes as u64 {
                    return Err(SourceError::limit(
                        &name,
                        format!("file is larger than {} bytes", limits.max_file_bytes),
                    ));
                }
                let bytes = fs::read(entry.path()).map_err(|e| SourceError::io(&name, &e))?;
                snap.add(&name, bytes, &limits, &mut folded)?;
            }
        }
        Ok(snap)
    }

    /// Ingest the served entries of a packed cart. The archive is indexed
    /// and bounded by [`crate::zipsource::ZipSource::open`] first, so the
    /// same name rules and limits apply as to a directory.
    pub fn from_zip(path: &Path, limits: SnapshotLimits) -> Result<Snapshot, SourceError> {
        let zip = crate::zipsource::ZipSource::open(path, limits)?;
        let mut snap = Snapshot::default();
        let mut folded = std::collections::HashSet::new();
        for name in zip.names() {
            if !is_served(name) {
                continue;
            }
            let bytes = zip.read(name)?;
            snap.add(name, bytes, &limits, &mut folded)?;
        }
        Ok(snap)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.files.get(name).map(Vec::as_slice)
    }

    /// The entries, for sending a snapshot elsewhere.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.files.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
    }
}

/// Directories worth descending into: `src` (any depth), `gfx`, `map`,
/// `sfx`, `music` and `samples`.
fn could_serve_under(dir: &str) -> bool {
    dir == "src"
        || dir.starts_with("src/")
        || matches!(dir, "gfx" | "map" | "sfx" | "music" | "samples")
}

#[cfg(windows)]
fn is_reparse(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse(meta: &fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

impl CartSource for Snapshot {
    fn snapshot(&self) -> Option<&Snapshot> {
        Some(self)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, SourceError> {
        // Names are exact; `\` is not a separator inside a snapshot.
        match self.files.get(path) {
            Some(b) => Ok(b.clone()),
            None => Err(SourceError::not_found(path)),
        }
    }

    fn exists(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("kuula-snapshot-{}-{n}", std::process::id()));
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn write(&self, rel: &str, bytes: &[u8]) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, bytes).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn served_roots_only() {
        for good in [
            "main.lua",
            "cart.toml",
            "src/a.lua",
            "src/deep/b.lua",
            "gfx/tiles.png",
            "map/level.json",
            "sfx/hit.trk",
            "music/song.trk",
            "samples/kick.wav",
        ] {
            assert!(is_served(good), "{good}");
        }
        for bad in [
            "README.md",
            "gfx/sub/x.png",
            "gfx/x.jpg",
            "map/x.txt",
            "sfx/x.wav",
            "samples/sub/x.wav",
            "src/x.txt",
            "other/main.lua",
            "main.lua.bak",
        ] {
            assert!(!is_served(bad), "{bad}");
        }
    }

    #[test]
    fn name_rules() {
        for good in ["main.lua", "src/a_b-c.lua", "gfx/Tiles2.png", "src/x.y.lua"] {
            assert_eq!(name_problem(good), None, "{good}");
        }
        for bad in [
            "",
            "/main.lua",
            "src//a.lua",
            "src/../a.lua",
            "./main.lua",
            "src/a.lua.",
            "src/a.lua ",
            "src/ a.lua",
            "src\\a.lua",
            "src/a:b.lua",
            "src/a?.lua",
            "src/con.lua",
            "src/LPT1.lua",
            "src/a\u{0}.lua",
            "src/a\n.lua",
        ] {
            assert!(name_problem(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn from_dir_takes_served_files_and_skips_the_rest() {
        let d = TempDir::new();
        d.write("main.lua", b"m");
        d.write("cart.toml", b"");
        d.write("src/util.lua", b"u");
        d.write("src/nested/deep.lua", b"d");
        d.write("gfx/tiles.png", b"p");
        d.write("gfx/notes.txt", b"n");
        d.write("map/a.json", b"j");
        d.write("README.md", b"r");
        d.write(".git/HEAD", b"h");
        d.write("assets/x.png", b"x");
        let s = Snapshot::from_dir(&d.0, SnapshotLimits::default()).unwrap();
        let names: Vec<_> = s.names().collect();
        assert_eq!(
            names,
            [
                "cart.toml",
                "gfx/tiles.png",
                "main.lua",
                "map/a.json",
                "src/nested/deep.lua",
                "src/util.lua"
            ]
        );
        assert_eq!(s.read("src/util.lua").unwrap(), b"u");
        assert_eq!(s.read("README.md").unwrap_err().code, "not_found");
        assert_eq!(s.read("src\\util.lua").unwrap_err().code, "not_found");
        assert_eq!(s.total_bytes(), 5);
        assert!(s.exists("main.lua"));
    }

    #[test]
    fn limits_trip_with_stable_codes() {
        let big = SnapshotLimits {
            max_files: 2,
            max_file_bytes: 3,
            max_total_bytes: 5,
        };
        let e = Snapshot::from_entries([("main.lua", vec![0; 4])], big).unwrap_err();
        assert_eq!(e.code, "limit_exceeded");
        let e = Snapshot::from_entries([("main.lua", vec![0; 3]), ("src/a.lua", vec![0; 3])], big)
            .unwrap_err();
        assert_eq!(e.code, "limit_exceeded");
        assert_eq!(e.path, "src/a.lua");
        let e = Snapshot::from_entries(
            [
                ("main.lua", vec![0; 1]),
                ("src/a.lua", vec![0; 1]),
                ("src/b.lua", vec![0; 1]),
            ],
            big,
        )
        .unwrap_err();
        assert_eq!(e.code, "limit_exceeded");
        let ok = Snapshot::from_entries([("main.lua", vec![0; 2]), ("src/a.lua", vec![0; 3])], big)
            .unwrap();
        assert_eq!(ok.len(), 2);
    }

    #[test]
    fn case_collisions_and_bad_names_are_rejected() {
        let e = Snapshot::from_entries(
            [("src/A.lua", vec![]), ("src/a.lua", vec![])],
            SnapshotLimits::default(),
        )
        .unwrap_err();
        assert_eq!(e.code, "invalid_path");
        let e = Snapshot::from_entries([("src/nul.lua", vec![])], SnapshotLimits::default())
            .unwrap_err();
        assert_eq!(e.code, "invalid_path");
    }

    #[cfg(windows)]
    #[test]
    fn symlinked_files_are_refused() {
        let d = TempDir::new();
        d.write("main.lua", b"m");
        let target = d.0.join("outside.lua");
        fs::write(&target, b"secret").unwrap();
        fs::create_dir_all(d.0.join("src")).unwrap();
        // Creating a file symlink needs developer mode or admin; when it
        // is not available the test still checks the plain directory.
        match std::os::windows::fs::symlink_file(&target, d.0.join("src").join("link.lua")) {
            Ok(()) => {
                let e = Snapshot::from_dir(&d.0, SnapshotLimits::default()).unwrap_err();
                assert_eq!(e.code, "invalid_path");
                assert!(e.message.contains("reparse"), "{}", e.message);
            }
            Err(_) => {
                let s = Snapshot::from_dir(&d.0, SnapshotLimits::default()).unwrap();
                assert_eq!(s.len(), 1);
            }
        }
    }
}
