//! A cart package: the bytes a deploy sends, validated the way the
//! receiver validates them. The receiver's check and the sender's local
//! check are the same functions, so a package that passes here is not
//! refused over its content there.

use std::fmt::Write;
use std::path::Path;

use kuula_core::console::MAIN_FILE;
use kuula_core::manifest::{Manifest, ManifestError, MANIFEST_FILE};
use kuula_core::source::SourceError;
use kuula_core::{Snapshot, SnapshotLimits};
use sha2::{Digest, Sha256};

use super::wire::{check_name, DeployCode, Refusal, MAX_PACKAGE};

/// `<code> <path>: <message>`: the core's own error, as the result
/// detail carries it.
pub fn source_detail(e: &SourceError) -> String {
    if e.path.is_empty() {
        format!("{}: {}", e.code, e.message)
    } else {
        format!("{} {}: {}", e.code, e.path, e.message)
    }
}

/// The content rules beyond the archive's: `main.lua` is there and
/// `cart.toml`, when present, parses. The detail on failure.
pub fn check_snapshot(snap: &Snapshot) -> Result<(), String> {
    if snap.get(MAIN_FILE).is_none() {
        return Err(format!(
            "not_found {MAIN_FILE}: the package has no {MAIN_FILE}"
        ));
    }
    if let Some(bytes) = snap.get(MANIFEST_FILE) {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| format!("manifest_error {MANIFEST_FILE}: not UTF-8"))?;
        Manifest::parse(text).map_err(|e| match e.line {
            Some(line) => format!(
                "{} {MANIFEST_FILE}:{line}: {}",
                ManifestError::CODE,
                e.message
            ),
            None => format!("{} {MANIFEST_FILE}: {}", ManifestError::CODE, e.message),
        })?;
    }
    Ok(())
}

/// Open a packed cart under the standard limits and check it; the
/// detail on failure.
pub fn check_package_file(path: &Path) -> Result<Snapshot, String> {
    let snap =
        Snapshot::from_zip(path, SnapshotLimits::default()).map_err(|e| source_detail(&e))?;
    check_snapshot(&snap)?;
    Ok(snap)
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// A validated package and its identity.
#[derive(Debug, Clone)]
pub struct Package {
    name: String,
    bytes: Vec<u8>,
    digest: [u8; 32],
}

impl Package {
    /// Pack a snapshot the way `kuula build` does.
    pub fn from_snapshot(name: &str, snap: &Snapshot) -> Result<Package, Refusal> {
        check_name(name)?;
        check_snapshot(snap).map_err(|d| Refusal::new(DeployCode::Invalid, d))?;
        let bytes = kuula_core::zipsource::pack(snap)
            .map_err(|e| Refusal::new(DeployCode::Invalid, source_detail(&e)))?;
        Package::new(name, bytes)
    }

    /// Take a packed cart as it is, after the same checks the receiver
    /// will make.
    pub fn from_zip_file(name: &str, path: &Path) -> Result<Package, Refusal> {
        check_name(name)?;
        let len = std::fs::metadata(path)
            .map_err(|e| {
                Refusal::new(
                    DeployCode::Invalid,
                    format!("io_error: {}: {e}", path.display()),
                )
            })?
            .len();
        if len > u64::from(MAX_PACKAGE) {
            return Err(Refusal::new(
                DeployCode::TooLarge,
                format!("package of {len} bytes, at most {MAX_PACKAGE}"),
            ));
        }
        check_package_file(path).map_err(|d| Refusal::new(DeployCode::Invalid, d))?;
        let bytes = std::fs::read(path).map_err(|e| {
            Refusal::new(
                DeployCode::Invalid,
                format!("io_error: {}: {e}", path.display()),
            )
        })?;
        Package::new(name, bytes)
    }

    fn new(name: &str, bytes: Vec<u8>) -> Result<Package, Refusal> {
        super::wire::check_len(u32::try_from(bytes.len()).unwrap_or(u32::MAX))?;
        let digest = Sha256::digest(&bytes).into();
        Ok(Package {
            name: name.to_string(),
            bytes,
            digest,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    pub fn digest_hex(&self) -> String {
        hex(&self.digest)
    }
}
