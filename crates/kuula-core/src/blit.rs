//! Blits through the colour table: sprite cells, scaled sprites, map
//! layers and system-font text. No fill pattern applies to these.

use crate::buf::{Buf, Rect};
use crate::font::{self, GLYPH_WIDTH};
use crate::pen::Pen;
use crate::raster::{blend, Surface};

/// Side of a sprite cell.
pub const CELL: i32 = 8;

/// Sprite cell `n` of `sheet`, `w` by `h` cells, optionally flipped.
#[allow(clippy::too_many_arguments)]
pub fn spr(
    s: &mut Surface,
    pen: &Pen,
    sheet: &Buf,
    n: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    flip_x: bool,
    flip_y: bool,
) {
    let per_row = sheet.width() as i32 / CELL;
    if n < 0 || per_row == 0 || w <= 0 || h <= 0 {
        return;
    }
    // Reject cells below the sheet before any multiplication so a huge
    // `n` can neither overflow nor wrap onto a real cell.
    let (col, row) = (n % per_row, n / per_row);
    if (row as i64) * (CELL as i64) >= sheet.height() as i64 {
        return;
    }
    let (sx, sy) = (col * CELL, row * CELL);
    let (sw, sh) = (w.saturating_mul(CELL), h.saturating_mul(CELL));
    sspr(s, pen, sheet, sx, sy, sw, sh, x, y, sw, sh, flip_x, flip_y);
}

/// Scaled, flipped copy through the colour table. Source pixel for
/// destination column `i` is `sx + i * sw / dw`. Work is bounded by the
/// visible destination rectangle.
#[allow(clippy::too_many_arguments)]
pub fn sspr(
    s: &mut Surface,
    pen: &Pen,
    sheet: &Buf,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    dx: i32,
    dy: i32,
    dw: i32,
    dh: i32,
    flip_x: bool,
    flip_y: bool,
) {
    let Some(src) = sheet.as_u8() else {
        return;
    };
    if sw <= 0 || sh <= 0 || dw <= 0 || dh <= 0 {
        return;
    }
    let (dx, dy) = pen.world(dx, dy);
    let dest = Rect::new(dx, dy, dw, dh).intersect(&pen.clip);
    if dest.is_empty() {
        return;
    }
    let (sheet_w, sheet_h) = (sheet.width() as i64, sheet.height() as i64);
    for py in dest.y..dest.bottom() {
        let j = (py - dy) as i64;
        let j = if flip_y { dh as i64 - 1 - j } else { j };
        let row = sy as i64 + j * sh as i64 / dh as i64;
        if row < 0 || row >= sheet_h {
            continue;
        }
        for px in dest.x..dest.right() {
            let i = (px - dx) as i64;
            let i = if flip_x { dw as i64 - 1 - i } else { i };
            let col = sx as i64 + i * sw as i64 / dw as i64;
            if col < 0 || col >= sheet_w {
                continue;
            }
            let c = src[(row * sheet_w + col) as usize];
            blend(s, pen, px, py, c);
        }
    }
}

/// Draw `cell_w` by `cell_h` cells of a map layer starting at
/// `(cell_x, cell_y)` with its top-left at `(sx, sy)`. Cells are read
/// from `cells`, which the caller has already sliced for the layer with
/// `map_w` columns and `map_h` rows. Negative cells draw nothing.
#[allow(clippy::too_many_arguments)]
pub fn map(
    s: &mut Surface,
    pen: &Pen,
    sheet: &Buf,
    cells: &[i16],
    map_w: i32,
    map_h: i32,
    tile: i32,
    cell_x: i32,
    cell_y: i32,
    sx: i32,
    sy: i32,
    cell_w: i32,
    cell_h: i32,
) {
    if tile <= 0 || cell_w <= 0 || cell_h <= 0 {
        return;
    }
    let (ox, oy) = pen.world(sx, sy);
    // Only the cells whose tiles touch the clip are visited.
    let first_col = ((pen.clip.x - ox) as i64).div_euclid(tile as i64).max(0);
    let last_col = ((pen.clip.right() - 1 - ox) as i64)
        .div_euclid(tile as i64)
        .min(cell_w as i64 - 1);
    let first_row = ((pen.clip.y - oy) as i64).div_euclid(tile as i64).max(0);
    let last_row = ((pen.clip.bottom() - 1 - oy) as i64)
        .div_euclid(tile as i64)
        .min(cell_h as i64 - 1);
    let per_row = sheet.width() as i32 / tile;
    if per_row == 0 {
        return;
    }
    for r in first_row..=last_row {
        let my = cell_y as i64 + r;
        if my < 0 || my >= map_h as i64 {
            continue;
        }
        for c in first_col..=last_col {
            let mx = cell_x as i64 + c;
            if mx < 0 || mx >= map_w as i64 {
                continue;
            }
            let t = cells[(my * map_w as i64 + mx) as usize];
            if t < 0 {
                continue;
            }
            let t = t as i32;
            let tsx = (t % per_row) * tile;
            let tsy = (t / per_row) * tile;
            let px = ox as i64 + c * tile as i64;
            let py = oy as i64 + r * tile as i64;
            if px < i32::MIN as i64
                || px > i32::MAX as i64
                || py < i32::MIN as i64
                || py > i32::MAX as i64
            {
                continue;
            }
            // Undo the camera: sspr applies it again.
            let (px, py) = (px as i32 + pen.camera.0, py as i32 + pen.camera.1);
            sspr(
                s, pen, sheet, tsx, tsy, tile, tile, px, py, tile, tile, false, false,
            );
        }
    }
}

/// Text with the system font: camera, clip and colour table, no fill
/// pattern. Returns the x after the last glyph in cart coordinates.
pub fn print(s: &mut Surface, pen: &Pen, text: &str, x: i32, y: i32, c: u8) -> i32 {
    let (wx, wy) = pen.world(x, y);
    let mut cx = wx;
    let mut end = x;
    for ch in text.chars() {
        let g = font::glyph(ch);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    blend(
                        s,
                        pen,
                        cx.saturating_add(col),
                        wy.saturating_add(row as i32),
                        c,
                    );
                }
            }
        }
        cx = cx.saturating_add(GLYPH_WIDTH);
        end = end.saturating_add(GLYPH_WIDTH);
    }
    end
}

#[cfg(test)]
// Tests end a `Surface` borrow with `drop` before reading the buffer.
#[allow(clippy::drop_non_drop)]
mod tests {
    use super::*;
    use crate::raster::testing::*;

    #[test]
    fn spr_flips_and_goes_through_the_table() {
        let sheet = sheet_4x4();
        let (mut buf, mut pen) = surface(8, 8);
        let mut s = Surface::of(&mut buf).unwrap();
        spr(&mut s, &pen, &sheet, 0, 0, 0, 1, 1, false, false);
        drop(s);
        assert_eq!(ascii(&buf)[0], "1.......");
        assert_eq!(ascii(&buf)[1], ".2......");
        buf.fill(0.0);
        let mut s = Surface::of(&mut buf).unwrap();
        spr(&mut s, &pen, &sheet, 0, 0, 0, 1, 1, true, false);
        drop(s);
        assert_eq!(ascii(&buf)[0], ".......1");
        assert_eq!(ascii(&buf)[1], "......2.");
        buf.fill(0.0);
        let mut s = Surface::of(&mut buf).unwrap();
        spr(&mut s, &pen, &sheet, 0, 0, 0, 1, 1, true, true);
        drop(s);
        assert_eq!(ascii(&buf)[7], ".......1");
        assert_eq!(ascii(&buf)[6], "......2.");
        // Transparent 0 keeps the background; remap changes the ink.
        buf.fill(9.0);
        pen.table.palt(0, true);
        pen.table.pal_map(1, 4);
        let mut s = Surface::of(&mut buf).unwrap();
        spr(&mut s, &pen, &sheet, 0, 0, 0, 1, 1, false, false);
        drop(s);
        assert_eq!(ascii(&buf)[0], "49999999");
        assert_eq!(ascii(&buf)[1], "92999999");
        // Two cells wide, negative index and off-sheet index.
        buf.fill(0.0);
        let mut s = Surface::of(&mut buf).unwrap();
        spr(&mut s, &pen, &sheet, 0, -4, 0, 2, 1, false, false);
        spr(&mut s, &pen, &sheet, -1, 0, 0, 1, 1, false, false);
        spr(&mut s, &pen, &sheet, 7, 0, 4, 1, 1, false, false);
        drop(s);
        assert!(ascii(&buf).iter().all(|r| r == "....3333"));
    }

    #[test]
    fn spr_below_the_sheet_draws_nothing_even_for_huge_indices() {
        let sheet = sheet_4x4();
        let (mut buf, pen) = surface(8, 8);
        let mut s = Surface::of(&mut buf).unwrap();
        // Two cells per row, one row: cell 2 is already off the sheet and
        // i32::MAX would overflow the row multiplication if unchecked.
        spr(&mut s, &pen, &sheet, 2, 0, 0, 1, 1, false, false);
        spr(&mut s, &pen, &sheet, i32::MAX, 0, 0, 1, 1, false, false);
        spr(&mut s, &pen, &sheet, 536_870_912, 0, 0, 1, 1, false, false);
        drop(s);
        assert!(ascii(&buf).iter().all(|row| row == "........"));
    }

    #[test]
    fn sspr_scales_with_integer_sampling() {
        let sheet = sheet_4x4();
        let (mut buf, pen) = surface(8, 4);
        let mut s = Surface::of(&mut buf).unwrap();
        // 2x2 source to 8x4 destination: each source pixel covers 4x2.
        sspr(&mut s, &pen, &sheet, 0, 0, 2, 2, 0, 0, 8, 4, false, false);
        drop(s);
        assert_eq!(
            ascii(&buf),
            ["1111....", "1111....", "....2222", "....2222"]
        );
        buf.fill(0.0);
        let mut s = Surface::of(&mut buf).unwrap();
        // Downscale 8x8 cell 1 (solid 3) to 4x2, flipped, partly off.
        sspr(&mut s, &pen, &sheet, 8, 0, 8, 8, -2, 3, 4, 2, true, true);
        sspr(&mut s, &pen, &sheet, 8, 0, 8, 8, 0, 0, 0, 5, false, false);
        drop(s);
        assert_eq!(ascii(&buf)[3], "33......");
        assert_eq!(ascii(&buf)[2], "........");
    }

    #[test]
    fn map_draws_visible_cells_and_skips_negative() {
        let sheet = sheet_4x4();
        let (mut buf, mut pen) = surface(16, 16);
        // 3x2 map: row 0 = [1, -1, 1], row 1 = [0, 1, 1].
        let cells: Vec<i16> = vec![1, -1, 1, 0, 1, 1];
        let mut s = Surface::of(&mut buf).unwrap();
        map(&mut s, &pen, &sheet, &cells, 3, 2, 8, 0, 0, 0, 0, 3, 2);
        drop(s);
        let a = ascii(&buf);
        assert_eq!(a[0], "33333333........");
        assert_eq!(a[7], "33333333........");
        assert_eq!(a[8], "1.......33333333");
        assert_eq!(a[15], "........33333333");
        // With the camera scrolled by 8 the third column appears.
        buf.fill(0.0);
        pen.camera = (8, 0);
        let mut s = Surface::of(&mut buf).unwrap();
        map(&mut s, &pen, &sheet, &cells, 3, 2, 8, 0, 0, 0, 0, 3, 2);
        drop(s);
        let a = ascii(&buf);
        assert_eq!(a[0], "........33333333");
        assert_eq!(a[8], "3333333333333333");
        // Starting at a cell outside the map draws nothing.
        buf.fill(0.0);
        pen.camera = (0, 0);
        let mut s = Surface::of(&mut buf).unwrap();
        map(&mut s, &pen, &sheet, &cells, 3, 2, 8, 5, 5, 0, 0, 3, 2);
        map(&mut s, &pen, &sheet, &cells, 3, 2, 8, -1, -1, 0, 0, 1, 1);
        drop(s);
        assert!(pixels(&buf).iter().all(|&p| p == 0));
    }

    #[test]
    fn print_places_glyphs_and_returns_the_end() {
        let (mut buf, mut pen) = surface(12, 8);
        let mut s = Surface::of(&mut buf).unwrap();
        assert_eq!(print(&mut s, &pen, "A", 1, 1, 7), 1 + GLYPH_WIDTH);
        assert_eq!(print(&mut s, &pen, "", 5, 5, 7), 5);
        drop(s);
        let a = ascii(&buf);
        assert_eq!(a[1], ".777........");
        assert_eq!(a[2], ".7.7........");
        assert_eq!(a[3], ".777........");
        assert_eq!(a[5], ".7.7........");
        assert_eq!(a[6], "............");
        // Camera and clip apply; the return value is in cart coordinates.
        buf.fill(0.0);
        pen.camera = (1, 1);
        pen.clip = Rect::new(0, 0, 2, 8);
        let mut s = Surface::of(&mut buf).unwrap();
        assert_eq!(print(&mut s, &pen, "AB", 1, 1, 7), 1 + 2 * GLYPH_WIDTH);
        drop(s);
        assert_eq!(ascii(&buf)[0], "77..........");
        assert_eq!(ascii(&buf)[1], "7...........");
        // Far away coordinates are safe.
        let mut s = Surface::of(&mut buf).unwrap();
        print(&mut s, &pen, "hello", i32::MAX - 1, i32::MAX - 1, 7);
        print(&mut s, &pen, "hello", i32::MIN, i32::MIN, 7);
    }
}
