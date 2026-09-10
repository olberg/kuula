//! The allocation ledger and the buffer slab
//! that hands out handles. Every cart-visible buffer lives here; the
//! screen is slot 0.

use std::collections::HashMap;
use std::fmt;

use crate::buf::{Buf, BufKind, MAX_BUF_DIM};

/// Provisional graphics budget: all live sheets, maps and cart buffers.
pub const GRAPHICS_CAP: usize = 8 * 1024 * 1024;

/// One category of the process-wide allocation ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger {
    used: usize,
    cap: usize,
}

impl Ledger {
    pub fn new(cap: usize) -> Ledger {
        Ledger { used: 0, cap }
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Reserve `bytes` or fail without changing anything.
    pub fn reserve(&mut self, bytes: usize) -> Result<(), GfxError> {
        match self.used.checked_add(bytes) {
            Some(total) if total <= self.cap => {
                self.used = total;
                Ok(())
            }
            _ => Err(GfxError::BudgetExceeded {
                requested: bytes,
                used: self.used,
                cap: self.cap,
            }),
        }
    }

    pub fn release(&mut self, bytes: usize) {
        self.used = self.used.saturating_sub(bytes);
    }
}

/// A handle to a buffer in the slab: slot index in the low 32 bits and a
/// generation in the high 32, so a released slot is never mistaken for
/// its successor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BufId(u64);

impl BufId {
    pub const SCREEN: BufId = BufId(0);

    fn new(slot: usize, generation: u32) -> BufId {
        BufId(((generation as u64) << 32) | slot as u64)
    }

    fn slot(self) -> usize {
        (self.0 & 0xffff_ffff) as usize
    }

    fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    pub fn raw(self) -> u64 {
        self.0
    }

    pub fn from_raw(raw: u64) -> BufId {
        BufId(raw)
    }
}

struct Slot {
    generation: u32,
    buf: Option<Buf>,
    /// Ledger bytes charged for this buffer; 0 for the screen.
    charged: usize,
}

/// Every live buffer, addressed by handle, plus the ledger and the
/// name cache for loaded assets.
pub struct Resources {
    slots: Vec<Slot>,
    free: Vec<usize>,
    ledger: Ledger,
    names: HashMap<String, BufId>,
}

impl Resources {
    /// A slab whose slot 0 is a zeroed `u8` screen of the given size.
    pub fn new(width: u32, height: u32) -> Resources {
        let screen = Buf::new(BufKind::U8, width, height).expect("screen size is small");
        Resources {
            slots: vec![Slot {
                generation: 0,
                buf: Some(screen),
                charged: 0,
            }],
            free: Vec::new(),
            ledger: Ledger::new(GRAPHICS_CAP),
            names: HashMap::new(),
        }
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn screen(&self) -> &Buf {
        self.slots[0]
            .buf
            .as_ref()
            .expect("screen is never released")
    }

    pub fn screen_mut(&mut self) -> &mut Buf {
        self.slots[0]
            .buf
            .as_mut()
            .expect("screen is never released")
    }

    /// Allocate a zeroed cart buffer, charged to the ledger.
    pub fn alloc(&mut self, kind: BufKind, width: u32, height: u32) -> Result<BufId, GfxError> {
        if width == 0 || height == 0 || width > MAX_BUF_DIM || height > MAX_BUF_DIM {
            return Err(GfxError::BadDimensions {
                width,
                height,
                max: MAX_BUF_DIM,
            });
        }
        let bytes = Buf::bytes_for(kind, width, height).ok_or(GfxError::BadDimensions {
            width,
            height,
            max: MAX_BUF_DIM,
        })?;
        self.ledger.reserve(bytes)?;
        let buf = Buf::new(kind, width, height).expect("checked above");
        Ok(self.insert(buf, bytes))
    }

    /// Put an already-decoded buffer into the slab, charging its bytes.
    /// Callers that decode first must call [`Ledger::reserve`] through
    /// [`Resources::reserve`] before decoding and pass `charged = true`.
    pub fn insert_charged(&mut self, buf: Buf) -> BufId {
        let bytes = buf.bytes();
        self.insert(buf, bytes)
    }

    /// Reserve bytes ahead of a decode. On failure nothing is charged; on
    /// success the caller must either `insert_charged` a buffer of exactly
    /// that size or `unreserve`.
    pub fn reserve(&mut self, bytes: usize) -> Result<(), GfxError> {
        self.ledger.reserve(bytes)
    }

    pub fn unreserve(&mut self, bytes: usize) {
        self.ledger.release(bytes)
    }

    fn insert(&mut self, buf: Buf, charged: usize) -> BufId {
        match self.free.pop() {
            Some(slot) => {
                let s = &mut self.slots[slot];
                s.buf = Some(buf);
                s.charged = charged;
                BufId::new(slot, s.generation)
            }
            None => {
                self.slots.push(Slot {
                    generation: 0,
                    buf: Some(buf),
                    charged,
                });
                BufId::new(self.slots.len() - 1, 0)
            }
        }
    }

    /// Free a buffer and return its bytes to the ledger. The screen is
    /// protected; an already-freed id is `BufReleased`.
    pub fn free(&mut self, id: BufId) -> Result<(), GfxError> {
        if id == BufId::SCREEN {
            return Err(GfxError::BufProtected);
        }
        let slot = id.slot();
        let s = self
            .slots
            .get_mut(slot)
            .filter(|s| s.generation == id.generation() && s.buf.is_some())
            .ok_or(GfxError::BufReleased)?;
        s.buf = None;
        self.ledger.release(s.charged);
        s.charged = 0;
        s.generation = s.generation.wrapping_add(1);
        self.free.push(slot);
        self.names.retain(|_, v| *v != id);
        Ok(())
    }

    /// Free an id if it is still live; used by the guest's graveyard where
    /// a double free is normal.
    pub fn free_if_live(&mut self, id: BufId) {
        let _ = self.free(id);
    }

    pub fn is_live(&self, id: BufId) -> bool {
        self.get(id).is_ok()
    }

    pub fn get(&self, id: BufId) -> Result<&Buf, GfxError> {
        self.slots
            .get(id.slot())
            .filter(|s| s.generation == id.generation())
            .and_then(|s| s.buf.as_ref())
            .ok_or(GfxError::BufReleased)
    }

    pub fn get_mut(&mut self, id: BufId) -> Result<&mut Buf, GfxError> {
        self.slots
            .get_mut(id.slot())
            .filter(|s| s.generation == id.generation())
            .and_then(|s| s.buf.as_mut())
            .ok_or(GfxError::BufReleased)
    }

    /// Borrow two distinct buffers, the first mutably. `Aliased` when they
    /// are the same buffer.
    pub fn pair_mut(&mut self, dst: BufId, src: BufId) -> Result<(&mut Buf, &Buf), GfxError> {
        self.get(dst)?;
        self.get(src)?;
        let (d, s) = (dst.slot(), src.slot());
        if d == s {
            return Err(GfxError::Aliased);
        }
        let (lo, hi) = self.slots.split_at_mut(d.max(s));
        let (a, b) = if d < s {
            (&mut lo[d], &mut hi[0])
        } else {
            (&mut hi[0], &mut lo[s])
        };
        Ok((a.buf.as_mut().unwrap(), b.buf.as_ref().unwrap()))
    }

    pub fn named(&self, name: &str) -> Option<BufId> {
        self.names.get(name).copied().filter(|id| self.is_live(*id))
    }

    pub fn set_name(&mut self, name: &str, id: BufId) {
        self.names.insert(name.to_string(), id);
    }

    /// Number of live buffers including the screen.
    pub fn live_count(&self) -> usize {
        self.slots.iter().filter(|s| s.buf.is_some()).count()
    }
}

/// Every error the graphics side can raise. The code is the stable,
/// agent-facing name; `Display` is `code: detail`.
#[derive(Debug, Clone, PartialEq)]
pub enum GfxError {
    BudgetExceeded {
        requested: usize,
        used: usize,
        cap: usize,
    },
    BadDimensions {
        width: u32,
        height: u32,
        max: u32,
    },
    BufReleased,
    BufProtected,
    Aliased,
    KindMismatch {
        expected: BufKind,
        got: BufKind,
    },
    NoSheet,
    NotAMap,
    Palette(crate::palette::PaletteError),
    AssetNotFound {
        path: String,
    },
    AssetInvalid {
        path: String,
        why: String,
    },
    AssetTooLarge {
        path: String,
        why: String,
    },
}

impl GfxError {
    pub fn code(&self) -> &'static str {
        match self {
            GfxError::BudgetExceeded { .. } => "graphics_budget_exceeded",
            GfxError::BadDimensions { .. } => "buf_bad_dimensions",
            GfxError::BufReleased => "buf_released",
            GfxError::BufProtected => "buf_protected",
            GfxError::Aliased => "buf_aliased",
            GfxError::KindMismatch { .. } => "buf_kind_mismatch",
            GfxError::NoSheet => "no_sheet",
            GfxError::NotAMap => "not_a_map",
            GfxError::Palette(e) => e.code(),
            GfxError::AssetNotFound { .. } => "asset_not_found",
            GfxError::AssetInvalid { .. } => "asset_invalid",
            GfxError::AssetTooLarge { .. } => "asset_too_large",
        }
    }

    pub fn asset_invalid(path: &str, why: impl Into<String>) -> GfxError {
        GfxError::AssetInvalid {
            path: path.to_string(),
            why: why.into(),
        }
    }

    pub fn asset_too_large(path: &str, why: impl Into<String>) -> GfxError {
        GfxError::AssetTooLarge {
            path: path.to_string(),
            why: why.into(),
        }
    }
}

impl From<crate::buf::BufError> for GfxError {
    fn from(e: crate::buf::BufError) -> GfxError {
        match e {
            crate::buf::BufError::KindMismatch { expected, got } => {
                GfxError::KindMismatch { expected, got }
            }
        }
    }
}

impl From<crate::palette::PaletteError> for GfxError {
    fn from(e: crate::palette::PaletteError) -> GfxError {
        GfxError::Palette(e)
    }
}

impl fmt::Display for GfxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            GfxError::BudgetExceeded {
                requested,
                used,
                cap,
            } => write!(f, "{requested} bytes requested with {used} of {cap} in use"),
            GfxError::BadDimensions { width, height, max } => {
                write!(f, "{width}x{height} is not within 1..={max} on each side")
            }
            GfxError::BufReleased => write!(f, "buffer has been released"),
            GfxError::BufProtected => write!(f, "the screen cannot be released"),
            GfxError::Aliased => write!(f, "source and destination are the same buffer"),
            GfxError::KindMismatch { expected, got } => write!(
                f,
                "expected a {} buffer, got {}",
                expected.name(),
                got.name()
            ),
            GfxError::NoSheet => write!(f, "no sheet selected; call sheet(b) first"),
            GfxError::NotAMap => write!(f, "buffer was not loaded with load_map"),
            GfxError::Palette(e) => write!(f, "{e}"),
            GfxError::AssetNotFound { path } => write!(f, "{path} is not in the cart"),
            GfxError::AssetInvalid { path, why } => write!(f, "{path}: {why}"),
            GfxError::AssetTooLarge { path, why } => write!(f, "{path}: {why}"),
        }
    }
}

impl std::error::Error for GfxError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_charges_exact_bytes_and_free_returns_them() {
        let mut r = Resources::new(4, 4);
        assert_eq!(r.ledger().used(), 0, "screen is not charged");
        let a = r.alloc(BufKind::I16, 10, 10).unwrap();
        assert_eq!(r.ledger().used(), 200);
        let b = r.alloc(BufKind::U8, 3, 3).unwrap();
        assert_eq!(r.ledger().used(), 209);
        r.free(a).unwrap();
        assert_eq!(r.ledger().used(), 9);
        r.free(b).unwrap();
        assert_eq!(r.ledger().used(), 0);
        assert_eq!(r.live_count(), 1);
    }

    #[test]
    fn over_cap_fails_without_allocating() {
        let mut r = Resources::new(1, 1);
        let big = r.alloc(BufKind::U8, 4096, 2048).unwrap();
        assert_eq!(r.ledger().used(), GRAPHICS_CAP);
        let err = r.alloc(BufKind::U8, 1, 1).unwrap_err();
        assert_eq!(err.code(), "graphics_budget_exceeded");
        assert_eq!(r.ledger().used(), GRAPHICS_CAP);
        assert_eq!(r.live_count(), 2);
        r.free(big).unwrap();
        assert!(r.alloc(BufKind::U8, 1, 1).is_ok());
    }

    #[test]
    fn bad_dimensions_are_rejected() {
        let mut r = Resources::new(1, 1);
        assert_eq!(
            r.alloc(BufKind::U8, 0, 5).unwrap_err().code(),
            "buf_bad_dimensions"
        );
        assert_eq!(
            r.alloc(BufKind::U8, 4097, 1).unwrap_err().code(),
            "buf_bad_dimensions"
        );
    }

    #[test]
    fn released_ids_stay_invalid() {
        let mut r = Resources::new(1, 1);
        let a = r.alloc(BufKind::U8, 2, 2).unwrap();
        r.free(a).unwrap();
        assert_eq!(r.get(a).unwrap_err().code(), "buf_released");
        assert_eq!(r.free(a).unwrap_err().code(), "buf_released");
        let b = r.alloc(BufKind::U8, 2, 2).unwrap();
        assert_ne!(a, b, "slot reused with a new generation");
        assert!(r.get(a).is_err());
        assert!(r.get(b).is_ok());
    }

    #[test]
    fn screen_is_protected_and_pair_refuses_aliasing() {
        let mut r = Resources::new(2, 2);
        assert_eq!(r.free(BufId::SCREEN).unwrap_err().code(), "buf_protected");
        let a = r.alloc(BufKind::U8, 2, 2).unwrap();
        assert_eq!(r.pair_mut(a, a).unwrap_err().code(), "buf_aliased");
        let (d, s) = r.pair_mut(a, BufId::SCREEN).unwrap();
        d.set(0, 0, 1.0);
        assert_eq!(s.get(0, 0), Some(0.0));
        let (d, s) = r.pair_mut(BufId::SCREEN, a).unwrap();
        d.set(0, 0, 2.0);
        assert_eq!(s.get(0, 0), Some(1.0));
    }

    #[test]
    fn names_follow_liveness() {
        let mut r = Resources::new(1, 1);
        let a = r.alloc(BufKind::U8, 2, 2).unwrap();
        r.set_name("tiles", a);
        assert_eq!(r.named("tiles"), Some(a));
        r.free(a).unwrap();
        assert_eq!(r.named("tiles"), None);
    }
}
