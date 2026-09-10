//! Draw state that is not pixels: the pen the API mutates and the
//! rasteriser reads, with its colour table
//! (section 9.2) and fill pattern.

use crate::buf::Rect;
use crate::palette::PALETTE_SIZE;
use crate::resources::BufId;

/// A primary colour and, for the fill pattern, a secondary one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    pub primary: u8,
    pub secondary: u8,
}

impl Colour {
    pub const fn solid(c: u8) -> Colour {
        Colour {
            primary: c & 0x7f,
            secondary: 0,
        }
    }

    /// Low 7 bits are the primary colour, bits 8 to 14 the secondary.
    pub fn from_i64(c: i64) -> Colour {
        Colour {
            primary: (c & 0x7f) as u8,
            secondary: ((c >> 8) & 0x7f) as u8,
        }
    }
}

/// A 4x4 two-colour dither. Bit `(y & 3) * 4 + (x & 3)` set selects the
/// secondary colour, or skips the pixel when `transparent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fillp {
    pub pattern: u16,
    pub transparent: bool,
}

impl Fillp {
    /// `Some(true)` for the secondary colour, `Some(false)` for the
    /// primary, `None` to skip.
    #[inline]
    pub fn pick(&self, x: i32, y: i32) -> Option<bool> {
        if self.pattern == 0 {
            return Some(false);
        }
        let bit = ((y & 3) * 4 + (x & 3)) as u16;
        if self.pattern >> bit & 1 == 0 {
            Some(false)
        } else if self.transparent {
            None
        } else {
            Some(true)
        }
    }
}

/// The 128x128 lookup, kept alongside the
/// remap and transparency it is built from.
#[derive(Clone, PartialEq, Eq)]
pub struct ColourTable {
    table: Box<[[u8; PALETTE_SIZE]; PALETTE_SIZE]>,
    remap: [u8; PALETTE_SIZE],
    transparent: [bool; PALETTE_SIZE],
}

impl std::fmt::Debug for ColourTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColourTable")
            .field("remap", &self.remap)
            .field("transparent", &self.transparent)
            .finish()
    }
}

impl Default for ColourTable {
    fn default() -> ColourTable {
        let mut t = ColourTable {
            table: Box::new([[0; PALETTE_SIZE]; PALETTE_SIZE]),
            remap: [0; PALETTE_SIZE],
            transparent: [false; PALETTE_SIZE],
        };
        t.reset();
        t
    }
}

impl ColourTable {
    pub fn reset(&mut self) {
        for (i, r) in self.remap.iter_mut().enumerate() {
            *r = i as u8;
        }
        self.transparent = [false; PALETTE_SIZE];
        self.rebuild_all();
    }

    /// Draw-time remap of `from` to `to`.
    pub fn pal_map(&mut self, from: u8, to: u8) {
        self.remap[(from & 0x7f) as usize] = to & 0x7f;
        self.rebuild_row((from & 0x7f) as usize);
    }

    pub fn clear_remap(&mut self) {
        for (i, r) in self.remap.iter_mut().enumerate() {
            *r = i as u8;
        }
        self.rebuild_all();
    }

    /// Source colour `i` (after remap) leaves the destination alone.
    pub fn palt(&mut self, i: u8, transparent: bool) {
        self.transparent[(i & 0x7f) as usize] = transparent;
        // Any row whose remap lands on `i` changes.
        for src in 0..PALETTE_SIZE {
            if self.remap[src] == i & 0x7f {
                self.rebuild_row(src);
            }
        }
    }

    pub fn clear_transparency(&mut self) {
        self.transparent = [false; PALETTE_SIZE];
        self.rebuild_all();
    }

    fn rebuild_all(&mut self) {
        for src in 0..PALETTE_SIZE {
            self.rebuild_row(src);
        }
    }

    /// `table[src][dst] = transparent[remap[src]] ? dst : remap[src]`.
    fn rebuild_row(&mut self, src: usize) {
        let mapped = self.remap[src];
        let row = &mut self.table[src];
        if self.transparent[mapped as usize] {
            for (dst, cell) in row.iter_mut().enumerate() {
                *cell = dst as u8;
            }
        } else {
            row.fill(mapped);
        }
    }

    #[inline]
    pub fn lookup(&self, src: u8, dst: u8) -> u8 {
        self.table[(src & 0x7f) as usize][(dst & 0x7f) as usize]
    }
}

/// Draw state that is not pixels: what the API mutates and the
/// rasteriser reads.
#[derive(Debug, Clone)]
pub struct Pen {
    pub target: BufId,
    /// In target coordinates, always inside the target.
    pub clip: Rect,
    pub camera: (i32, i32),
    pub fillp: Fillp,
    pub table: ColourTable,
    pub sheet: Option<BufId>,
}

impl Pen {
    pub fn new(target: BufId, width: u32, height: u32) -> Pen {
        Pen {
            target,
            clip: Rect::new(0, 0, width as i32, height as i32),
            camera: (0, 0),
            fillp: Fillp::default(),
            table: ColourTable::default(),
            sheet: None,
        }
    }

    #[inline]
    pub(crate) fn world(&self, x: i32, y: i32) -> (i32, i32) {
        (
            x.saturating_sub(self.camera.0),
            y.saturating_sub(self.camera.1),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_table_composes_remap_then_transparency() {
        let mut t = ColourTable::default();
        assert_eq!(t.lookup(5, 9), 5);
        t.palt(5, true);
        assert_eq!(t.lookup(5, 9), 9);
        t.pal_map(1, 5);
        assert_eq!(t.lookup(1, 9), 9, "1 maps to 5 which is transparent");
        t.palt(5, false);
        assert_eq!(t.lookup(1, 9), 5);
        t.pal_map(1, 6);
        t.palt(1, true);
        assert_eq!(t.lookup(1, 9), 6, "transparency of the source is not used");
        t.reset();
        assert_eq!(t.lookup(1, 9), 1);
        assert_eq!(t.lookup(5, 9), 5);
        assert_eq!(t.lookup(0x85, 0x89), 5, "both indices masked");
    }
}
