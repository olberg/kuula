use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Where cart files come from. There is one implementation, a
/// directory; later ones add packed carts and content hashing.
pub trait CartSource {
    /// Read a file by its cart-relative path, e.g. `main.lua`.
    fn read(&self, path: &str) -> Result<Vec<u8>, SourceError>;

    /// Whether a file exists, without reading it.
    fn exists(&self, path: &str) -> bool {
        self.read(path).is_ok()
    }

    /// The snapshot behind this source, when it is one. A host that hands
    /// a cart to another process needs its entries, not a reader.
    fn snapshot(&self) -> Option<&crate::snapshot::Snapshot> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    /// Stable code: `not_found`, `invalid_path`, `limit_exceeded` or
    /// `io_error`.
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl SourceError {
    pub const NOT_FOUND: &'static str = "not_found";
    pub const INVALID_PATH: &'static str = "invalid_path";
    pub const LIMIT_EXCEEDED: &'static str = "limit_exceeded";
    pub const IO_ERROR: &'static str = "io_error";

    pub fn limit(path: &str, why: impl Into<String>) -> SourceError {
        SourceError {
            code: Self::LIMIT_EXCEEDED,
            path: path.to_string(),
            message: why.into(),
        }
    }

    pub fn io(path: &str, e: &std::io::Error) -> SourceError {
        match e.kind() {
            std::io::ErrorKind::NotFound => SourceError::not_found(path),
            _ => SourceError {
                code: Self::IO_ERROR,
                path: path.to_string(),
                message: e.to_string(),
            },
        }
    }

    pub fn not_found(path: &str) -> SourceError {
        SourceError {
            code: Self::NOT_FOUND,
            path: path.to_string(),
            message: "file not found in cart".to_string(),
        }
    }

    pub fn invalid_path(path: &str, why: &str) -> SourceError {
        SourceError {
            code: Self::INVALID_PATH,
            path: path.to_string(),
            message: why.to_string(),
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}): {}", self.path, self.code, self.message)
    }
}

impl std::error::Error for SourceError {}

/// A cart laid out as files under a directory.
#[derive(Debug, Clone)]
pub struct DirSource {
    root: PathBuf,
}

impl DirSource {
    pub fn new(root: impl Into<PathBuf>) -> DirSource {
        DirSource { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validate a cart-relative path: no absolute paths, no drive or UNC
    /// prefixes, no `..`. Both `/` and `\` separate components.
    fn resolve(&self, path: &str) -> Result<PathBuf, SourceError> {
        if path.is_empty() {
            return Err(SourceError::invalid_path(path, "empty path"));
        }
        if path.starts_with('/') || path.starts_with('\\') {
            return Err(SourceError::invalid_path(
                path,
                "absolute paths are not allowed",
            ));
        }
        let mut out = self.root.clone();
        let mut any = false;
        for piece in path.split(['/', '\\']) {
            if piece.is_empty() {
                continue;
            }
            // `Path::components` on a single piece catches `..`, `.` and
            // Windows prefixes like `C:` while treating everything else as
            // a plain name.
            let mut comps = Path::new(piece).components();
            match (comps.next(), comps.next()) {
                (Some(Component::Normal(name)), None) if !piece.contains(':') => {
                    out.push(name);
                    any = true;
                }
                (Some(Component::CurDir), None) => {}
                _ => {
                    return Err(SourceError::invalid_path(
                        path,
                        "path must be relative and inside the cart",
                    ))
                }
            }
        }
        if !any {
            return Err(SourceError::invalid_path(path, "empty path"));
        }
        Ok(out)
    }
}

impl CartSource for DirSource {
    fn read(&self, path: &str) -> Result<Vec<u8>, SourceError> {
        let full = self.resolve(path)?;
        std::fs::read(&full).map_err(|e| SourceError::io(path, &e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cart() -> (tempdir::TempDir, DirSource) {
        let dir = tempdir::TempDir::new("kuula-source");
        std::fs::write(dir.path().join("main.lua"), b"print('hi')").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("a.txt"), b"a").unwrap();
        let src = DirSource::new(dir.path());
        (dir, src)
    }

    #[test]
    fn reads_main_lua_bytes() {
        let (_d, src) = temp_cart();
        assert_eq!(src.read("main.lua").unwrap(), b"print('hi')");
        assert_eq!(src.read("sub/a.txt").unwrap(), b"a");
        assert_eq!(src.read("sub\\a.txt").unwrap(), b"a");
        assert_eq!(src.read("./main.lua").unwrap(), b"print('hi')");
    }

    #[test]
    fn missing_file_is_not_found_not_a_panic() {
        let (_d, src) = temp_cart();
        let err = src.read("nope.lua").unwrap_err();
        assert_eq!(err.code, "not_found");
        assert_eq!(err.path, "nope.lua");
    }

    #[test]
    fn parent_and_absolute_paths_are_rejected() {
        let (_d, src) = temp_cart();
        for bad in [
            "..\\secret",
            "../secret",
            "sub/../../secret",
            "C:\\secret",
            "C:/secret",
            "/secret",
            "\\secret",
            "\\\\server\\share\\secret",
            "",
            ".",
        ] {
            let err = src.read(bad).unwrap_err();
            assert_eq!(err.code, "invalid_path", "path {bad:?}");
        }
    }

    /// Minimal temp dir so the crate has no dev-dependencies.
    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);

        pub struct TempDir(PathBuf);

        impl TempDir {
            pub fn new(prefix: &str) -> TempDir {
                let n = COUNTER.fetch_add(1, Ordering::SeqCst);
                let p = std::env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()));
                std::fs::create_dir_all(&p).unwrap();
                TempDir(p)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
