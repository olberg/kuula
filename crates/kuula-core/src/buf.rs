//! Typed 2D buffers: the one memory type a cart gets (architecture
//! section 8). A sprite sheet, a map layer and the screen are all `Buf`s.

use std::fmt;

/// Largest side of a cart-allocated buffer.
pub const MAX_BUF_DIM: u32 = 4096;

/// Largest side of a sprite sheet.
pub const MAX_SHEET_DIM: u32 = 1024;

/// Largest side of a map layer, in cells.
pub const MAX_MAP_DIM: u32 = 256;

/// Most layers a map may have.
pub const MAX_MAP_LAYERS: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BufKind {
    U8,
    I16,
    I32,
    F32,
}

impl BufKind {
    pub fn parse(name: &str) -> Option<BufKind> {
        match name {
            "u8" => Some(BufKind::U8),
            "i16" => Some(BufKind::I16),
            "i32" => Some(BufKind::I32),
            "f32" => Some(BufKind::F32),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BufKind::U8 => "u8",
            BufKind::I16 => "i16",
            BufKind::I32 => "i32",
            BufKind::F32 => "f32",
        }
    }

    pub fn element_bytes(self) -> usize {
        match self {
            BufKind::U8 => 1,
            BufKind::I16 => 2,
            BufKind::I32 | BufKind::F32 => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BufData {
    U8(Vec<u8>),
    I16(Vec<i16>),
    I32(Vec<i32>),
    F32(Vec<f32>),
}

/// Metadata a map buffer carries so `map()` knows how to read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapInfo {
    /// Cells per layer vertically; the buffer is `layers * rows` tall.
    pub rows: u32,
    pub layers: u32,
    /// 8 or 16.
    pub tile_size: u32,
}

/// A rectangle of typed elements, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Buf {
    kind: BufKind,
    width: u32,
    height: u32,
    data: BufData,
    map: Option<MapInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const EMPTY: Rect = Rect {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    };

    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn right(&self) -> i32 {
        self.x.saturating_add(self.w)
    }

    pub fn bottom(&self) -> i32 {
        self.y.saturating_add(self.h)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    pub fn intersect(&self, other: &Rect) -> Rect {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        if x1 <= x0 || y1 <= y0 {
            Rect::EMPTY
        } else {
            Rect::new(x0, y0, x1 - x0, y1 - y0)
        }
    }
}

impl Buf {
    /// Allocate a zeroed buffer. Callers reserve in the ledger first; this
    /// only checks that the size arithmetic does not overflow.
    pub fn new(kind: BufKind, width: u32, height: u32) -> Option<Buf> {
        let n = (width as usize).checked_mul(height as usize)?;
        n.checked_mul(kind.element_bytes())?;
        let data = match kind {
            BufKind::U8 => BufData::U8(vec![0; n]),
            BufKind::I16 => BufData::I16(vec![0; n]),
            BufKind::I32 => BufData::I32(vec![0; n]),
            BufKind::F32 => BufData::F32(vec![0.0; n]),
        };
        Some(Buf {
            kind,
            width,
            height,
            data,
            map: None,
        })
    }

    pub fn from_u8(width: u32, height: u32, pixels: Vec<u8>) -> Buf {
        assert_eq!(pixels.len(), width as usize * height as usize);
        Buf {
            kind: BufKind::U8,
            width,
            height,
            data: BufData::U8(pixels),
            map: None,
        }
    }

    pub fn from_i16(width: u32, height: u32, cells: Vec<i16>, map: Option<MapInfo>) -> Buf {
        assert_eq!(cells.len(), width as usize * height as usize);
        Buf {
            kind: BufKind::I16,
            width,
            height,
            data: BufData::I16(cells),
            map,
        }
    }

    /// Bytes this buffer's storage costs, for the ledger.
    pub fn bytes_for(kind: BufKind, width: u32, height: u32) -> Option<usize> {
        (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(kind.element_bytes())
    }

    pub fn bytes(&self) -> usize {
        self.width as usize * self.height as usize * self.kind.element_bytes()
    }

    pub fn kind(&self) -> BufKind {
        self.kind
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn map_info(&self) -> Option<MapInfo> {
        self.map
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    pub fn as_u8(&self) -> Option<&[u8]> {
        match &self.data {
            BufData::U8(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_u8_mut(&mut self) -> Option<&mut [u8]> {
        match &mut self.data {
            BufData::U8(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_i16(&self) -> Option<&[i16]> {
        match &self.data {
            BufData::I16(v) => Some(v),
            _ => None,
        }
    }

    /// Every element as little-endian bytes, for the codec's blob form.
    pub fn raw_bytes(&self) -> Vec<u8> {
        match &self.data {
            BufData::U8(v) => v.clone(),
            BufData::I16(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            BufData::I32(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            BufData::F32(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        }
    }

    /// Replace every element from little-endian bytes. `false`, and no
    /// change, when the length is not exactly the buffer's storage.
    pub fn set_raw_bytes(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() != self.bytes() {
            return false;
        }
        match &mut self.data {
            BufData::U8(v) => v.copy_from_slice(bytes),
            BufData::I16(v) => {
                for (x, c) in v.iter_mut().zip(bytes.as_chunks::<2>().0) {
                    *x = i16::from_le_bytes(*c);
                }
            }
            BufData::I32(v) => {
                for (x, c) in v.iter_mut().zip(bytes.as_chunks::<4>().0) {
                    *x = i32::from_le_bytes(*c);
                }
            }
            BufData::F32(v) => {
                for (x, c) in v.iter_mut().zip(bytes.as_chunks::<4>().0) {
                    *x = f32::from_le_bytes(*c);
                }
            }
        }
        true
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        Some(y as usize * self.width as usize + x as usize)
    }

    /// Read one element as a float. `None` outside the buffer.
    pub fn get(&self, x: i32, y: i32) -> Option<f64> {
        let i = self.index(x, y)?;
        Some(match &self.data {
            BufData::U8(v) => v[i] as f64,
            BufData::I16(v) => v[i] as f64,
            BufData::I32(v) => v[i] as f64,
            BufData::F32(v) => v[i] as f64,
        })
    }

    /// Write one element, converting as [`BufKind`] documents. Outside
    /// the buffer is ignored.
    pub fn set(&mut self, x: i32, y: i32, value: f64) {
        if let Some(i) = self.index(x, y) {
            match &mut self.data {
                BufData::U8(v) => v[i] = to_int(value) as u8,
                BufData::I16(v) => v[i] = to_int(value) as i16,
                BufData::I32(v) => v[i] = to_int(value) as i32,
                BufData::F32(v) => v[i] = value as f32,
            }
        }
    }

    pub fn fill(&mut self, value: f64) {
        match &mut self.data {
            BufData::U8(v) => v.fill(to_int(value) as u8),
            BufData::I16(v) => v.fill(to_int(value) as i16),
            BufData::I32(v) => v.fill(to_int(value) as i32),
            BufData::F32(v) => v.fill(value as f32),
        }
    }

    /// Copy a `w` by `h` rectangle from `(sx, sy)` of `src` to `(dx, dy)`
    /// of `self`, clipped to both buffers and to `clip` in destination
    /// coordinates when given. Both must be the same kind.
    #[allow(clippy::too_many_arguments)]
    pub fn copy_from(
        &mut self,
        src: &Buf,
        sx: i32,
        sy: i32,
        w: i32,
        h: i32,
        dx: i32,
        dy: i32,
        clip: Option<Rect>,
    ) -> Result<(), BufError> {
        if src.kind != self.kind {
            return Err(BufError::KindMismatch {
                expected: self.kind,
                got: src.kind,
            });
        }
        let Some(r) = copy_region(self.bounds(), src.bounds(), sx, sy, w, h, dx, dy, clip) else {
            return Ok(());
        };
        let sw = src.width as usize;
        let dw = self.width as usize;
        match (&mut self.data, &src.data) {
            (BufData::U8(d), BufData::U8(s)) => copy_rows(d, dw, s, sw, r),
            (BufData::I16(d), BufData::I16(s)) => copy_rows(d, dw, s, sw, r),
            (BufData::I32(d), BufData::I32(s)) => copy_rows(d, dw, s, sw, r),
            (BufData::F32(d), BufData::F32(s)) => copy_rows(d, dw, s, sw, r),
            _ => unreachable!("kinds checked above"),
        }
        Ok(())
    }

    /// Copy a rectangle within one buffer, overlap-safe.
    #[allow(clippy::too_many_arguments)]
    pub fn copy_within(
        &mut self,
        sx: i32,
        sy: i32,
        w: i32,
        h: i32,
        dx: i32,
        dy: i32,
        clip: Option<Rect>,
    ) {
        let b = self.bounds();
        let Some(r) = copy_region(b, b, sx, sy, w, h, dx, dy, clip) else {
            return;
        };
        let w = self.width as usize;
        match &mut self.data {
            BufData::U8(d) => copy_rows_within(d, w, r),
            BufData::I16(d) => copy_rows_within(d, w, r),
            BufData::I32(d) => copy_rows_within(d, w, r),
            BufData::F32(d) => copy_rows_within(d, w, r),
        }
    }
}

/// Lua numbers to integers: floor, saturate.
pub fn to_int(v: f64) -> i64 {
    if v.is_nan() {
        0
    } else {
        v.floor() as i64
    }
}

/// A resolved copy: source origin, destination origin, size.
#[derive(Debug, Clone, Copy)]
struct CopyRegion {
    sx: usize,
    sy: usize,
    dx: usize,
    dy: usize,
    w: usize,
    h: usize,
}

#[allow(clippy::too_many_arguments)]
fn copy_region(
    dst: Rect,
    src: Rect,
    sx: i32,
    sy: i32,
    w: i32,
    h: i32,
    dx: i32,
    dy: i32,
    clip: Option<Rect>,
) -> Option<CopyRegion> {
    if w <= 0 || h <= 0 {
        return None;
    }
    // Clip the source rectangle to the source bounds and carry the
    // offsets to the destination.
    let s = Rect::new(sx, sy, w, h).intersect(&src);
    if s.is_empty() {
        return None;
    }
    let dx = dx.checked_add(s.x - sx)?;
    let dy = dy.checked_add(s.y - sy)?;
    let mut d = Rect::new(dx, dy, s.w, s.h).intersect(&dst);
    if let Some(c) = clip {
        d = d.intersect(&c);
    }
    if d.is_empty() {
        return None;
    }
    Some(CopyRegion {
        sx: (s.x + (d.x - dx)) as usize,
        sy: (s.y + (d.y - dy)) as usize,
        dx: d.x as usize,
        dy: d.y as usize,
        w: d.w as usize,
        h: d.h as usize,
    })
}

fn copy_rows<T: Copy>(dst: &mut [T], dw: usize, src: &[T], sw: usize, r: CopyRegion) {
    for row in 0..r.h {
        let s = (r.sy + row) * sw + r.sx;
        let d = (r.dy + row) * dw + r.dx;
        dst[d..d + r.w].copy_from_slice(&src[s..s + r.w]);
    }
}

fn copy_rows_within<T: Copy>(data: &mut [T], w: usize, r: CopyRegion) {
    // Walk rows in the order that never overwrites unread source rows;
    // within a row `copy_within` is memmove.
    let rows: Box<dyn Iterator<Item = usize>> = if r.dy > r.sy {
        Box::new((0..r.h).rev())
    } else {
        Box::new(0..r.h)
    };
    for row in rows {
        let s = (r.sy + row) * w + r.sx;
        let d = (r.dy + row) * w + r.dx;
        data.copy_within(s..s + r.w, d);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufError {
    KindMismatch { expected: BufKind, got: BufKind },
}

impl BufError {
    pub fn code(&self) -> &'static str {
        match self {
            BufError::KindMismatch { .. } => "buf_kind_mismatch",
        }
    }
}

impl fmt::Display for BufError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BufError::KindMismatch { expected, got } => write!(
                f,
                "{}: expected a {} buffer, got {}",
                self.code(),
                expected.name(),
                got.name()
            ),
        }
    }
}

impl std::error::Error for BufError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(w: u32, h: u32) -> Buf {
        let mut b = Buf::new(BufKind::U8, w, h).unwrap();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                b.set(x, y, (y * w as i32 + x) as f64);
            }
        }
        b
    }

    #[test]
    fn get_set_convert_per_kind() {
        let mut b = Buf::new(BufKind::U8, 2, 2).unwrap();
        b.set(0, 0, 300.0);
        assert_eq!(b.get(0, 0), Some(44.0));
        b.set(1, 1, -1.0);
        assert_eq!(b.get(1, 1), Some(255.0));
        assert_eq!(b.get(2, 0), None);
        b.set(5, 5, 1.0);

        let mut m = Buf::new(BufKind::I16, 1, 1).unwrap();
        m.set(0, 0, -3.7);
        assert_eq!(m.get(0, 0), Some(-4.0));
        m.set(0, 0, 40000.0);
        assert_eq!(m.get(0, 0), Some(-25536.0));

        let mut f = Buf::new(BufKind::F32, 1, 1).unwrap();
        f.set(0, 0, 0.5);
        assert_eq!(f.get(0, 0), Some(0.5));
        f.set(0, 0, f64::NAN);
        assert!(f.get(0, 0).unwrap().is_nan());

        let mut i = Buf::new(BufKind::I32, 1, 1).unwrap();
        i.set(0, 0, 1e12);
        assert_eq!(i.get(0, 0), Some((1e12 as i64 as i32) as f64));
    }

    #[test]
    fn bytes_are_exact_and_overflow_is_none() {
        assert_eq!(Buf::bytes_for(BufKind::U8, 320, 240), Some(76800));
        assert_eq!(Buf::bytes_for(BufKind::I16, 256, 1024), Some(524288));
        assert_eq!(Buf::bytes_for(BufKind::F32, u32::MAX, u32::MAX), None);
        assert_eq!(Buf::new(BufKind::F32, 3, 3).unwrap().bytes(), 36);
    }

    #[test]
    fn copy_clips_at_every_edge() {
        let src = numbered(4, 4);
        let mut dst = Buf::new(BufKind::U8, 4, 4).unwrap();
        // Source rect hangs off the top-left of src, destination off the
        // bottom-right of dst.
        dst.copy_from(&src, -1, -1, 3, 3, 3, 3, None).unwrap();
        assert_eq!(dst.get(3, 3), Some(0.0));
        assert_eq!(dst.get(2, 2), Some(0.0), "untouched");
        dst.fill(9.0);
        dst.copy_from(&src, 2, 2, 5, 5, -1, -1, None).unwrap();
        assert_eq!(dst.get(0, 0), Some(15.0));
        assert_eq!(dst.get(1, 0), Some(9.0));
        dst.fill(9.0);
        dst.copy_from(&src, 0, 0, 4, 4, 0, 0, Some(Rect::new(1, 1, 2, 2)))
            .unwrap();
        assert_eq!(dst.get(0, 0), Some(9.0));
        assert_eq!(dst.get(1, 1), Some(5.0));
        assert_eq!(dst.get(2, 2), Some(10.0));
        assert_eq!(dst.get(3, 3), Some(9.0));
        dst.copy_from(&src, 0, 0, 0, 4, 0, 0, None).unwrap();
        dst.copy_from(&src, 10, 10, 4, 4, 0, 0, None).unwrap();
    }

    #[test]
    fn overlapping_copy_within_matches_a_copy_through_a_temporary() {
        for (dx, dy) in [(1, 1), (-1, -1), (1, -1), (-1, 1), (0, 2), (2, 0)] {
            let mut a = numbered(6, 6);
            let mut b = numbered(6, 6);
            let temp = a.clone();
            a.copy_within(1, 1, 4, 4, 1 + dx, 1 + dy, None);
            b.copy_from(&temp, 1, 1, 4, 4, 1 + dx, 1 + dy, None)
                .unwrap();
            assert_eq!(a, b, "offset ({dx},{dy})");
        }
    }

    #[test]
    fn copy_between_kinds_is_an_error() {
        let src = Buf::new(BufKind::I16, 2, 2).unwrap();
        let mut dst = Buf::new(BufKind::U8, 2, 2).unwrap();
        let err = dst.copy_from(&src, 0, 0, 1, 1, 0, 0, None).unwrap_err();
        assert_eq!(err.code(), "buf_kind_mismatch");
    }

    #[test]
    fn rect_intersection() {
        let a = Rect::new(0, 0, 10, 10);
        assert_eq!(a.intersect(&Rect::new(5, 5, 10, 10)), Rect::new(5, 5, 5, 5));
        assert!(a.intersect(&Rect::new(10, 0, 5, 5)).is_empty());
        assert!(a.intersect(&Rect::new(-5, -5, 5, 5)).is_empty());
        assert!(a.contains(9, 9));
        assert!(!a.contains(10, 9));
    }
}
