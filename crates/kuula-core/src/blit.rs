//! Blits through the colour table: sprite cells, scaled sprites, map
//! layers and system-font text. No fill pattern applies to these.

use crate::buf::{Buf, Rect};
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
    // The source column of each destination column, and the same for rows,
    // is walked instead of worked out: a division for every pixel is a
    // library call on a 32-bit machine, and most sprites are not scaled.
    let mut rows = Walk::new(dest.y - dy, sh, dh, flip_y);
    let (width, table) = (s.width as usize, &pen.table);
    let (x0, x1) = (dest.x as usize, dest.right() as usize);
    // A sprite at its own width, which is every `spr` and every map tile:
    // the destination columns that have a source column are one run, and
    // the source columns are the same run, forwards or backwards.
    let run = (sw == dw).then(|| {
        // The first source column, and the destination columns it and
        // the sheet's last column land on.
        let (first, lo, hi) = if flip_x {
            let last = dx as i64 + sx as i64 + dw as i64 - 1;
            (
                sx as i64 + dw as i64 - 1 - (dest.x - dx) as i64,
                last - (sheet_w - 1),
                last,
            )
        } else {
            let zero = dx as i64 - sx as i64;
            (sx as i64 + (dest.x - dx) as i64, zero, zero + sheet_w - 1)
        };
        let a = (dest.x as i64).max(lo);
        let b = (dest.right() as i64 - 1).min(hi);
        // The source column under destination column `a`.
        let skipped = a - dest.x as i64;
        let from = if flip_x {
            first - skipped
        } else {
            first + skipped
        };
        (a, b, from)
    });
    // Scaled across: the walk, started once and taken up again by each row.
    let columns = run
        .is_none()
        .then(|| Walk::new(dest.x - dx, sw, dw, flip_x));
    for py in dest.y..dest.bottom() {
        let row = sy as i64 + rows.next();
        if row < 0 || row >= sheet_h {
            continue;
        }
        let from = &src[(row * sheet_w) as usize..][..sheet_w as usize];
        // `dest` is inside the clip, which is inside the target.
        let line = &mut s.pixels[py as usize * width..][..width];
        if let Some((a, b, first)) = run {
            if a > b {
                continue;
            }
            let n = (b - a + 1) as usize;
            let to = &mut line[a as usize..][..n];
            if flip_x {
                let from = &from[(first + 1) as usize - n..][..n];
                for (dst, &c) in to.iter_mut().zip(from.iter().rev()) {
                    *dst = table.lookup(c, *dst);
                }
            } else {
                let from = &from[first as usize..][..n];
                for (dst, &c) in to.iter_mut().zip(from) {
                    *dst = table.lookup(c, *dst);
                }
            }
            s.touched += n as u64;
            continue;
        }
        let Some(mut columns) = columns else {
            continue;
        };
        for dst in &mut line[x0..x1] {
            let col = sx as i64 + columns.next();
            if col < 0 || col >= sheet_w {
                continue;
            }
            s.touched += 1;
            *dst = table.lookup(from[col as usize], *dst);
        }
    }
}

/// `i * from / to` for `i` counting up from `first`, or, flipped, for
/// `to - 1 - i`: the source offset of each step along a destination edge
/// of `to` pixels that shows `from` source pixels. One division at the
/// start, then additions; every value is the one the division would give.
#[derive(Clone, Copy)]
struct Walk {
    /// The offset the next call returns, and its remainder over `to`.
    at: i64,
    rem: i64,
    /// What a step adds to each, with its sign.
    step: i64,
    step_rem: i64,
    to: i64,
}

impl Walk {
    fn new(first: i32, from: i32, to: i32, flip: bool) -> Walk {
        let (from, to) = (from as i64, to as i64);
        let (i, sign) = if flip {
            (to - 1 - first as i64, -1)
        } else {
            (first as i64, 1)
        };
        // At its own size, the usual case, the offset is the step itself
        // and the divisions below would only say so.
        if from == to {
            return Walk {
                at: i,
                rem: 0,
                step: sign,
                step_rem: 0,
                to,
            };
        }
        Walk {
            at: (i * from).div_euclid(to),
            rem: (i * from).rem_euclid(to),
            step: sign * (from / to),
            step_rem: sign * (from % to),
            to,
        }
    }

    #[inline]
    fn next(&mut self) -> i64 {
        let at = self.at;
        self.at += self.step;
        self.rem += self.step_rem;
        if self.rem >= self.to {
            self.rem -= self.to;
            self.at += 1;
        } else if self.rem < 0 {
            self.rem += self.to;
            self.at -= 1;
        }
        at
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

/// Text with the pen's system face: camera, clip and colour table, no
/// fill pattern. Returns the x after the last glyph in cart coordinates.
pub fn print(s: &mut Surface, pen: &Pen, text: &str, x: i32, y: i32, c: u8) -> i32 {
    let font = pen.font.font();
    let (wx, wy) = pen.world(x, y);
    let mut cx = wx;
    let mut end = x;
    for ch in text.chars() {
        let g = font.glyph(ch);
        for row in 0..g.height {
            for col in 0..g.width {
                if g.pixel(col, row) {
                    blend(s, pen, cx.saturating_add(col), wy.saturating_add(row), c);
                }
            }
        }
        cx = cx.saturating_add(g.width);
        end = end.saturating_add(g.width);
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
    fn a_walk_gives_what_the_division_gives() {
        // Every scale up and down to 40, from every first column, both ways.
        for from in 1..=40 {
            for to in 1..=40 {
                for first in 0..to {
                    for flip in [false, true] {
                        let mut walk = Walk::new(first, from, to, flip);
                        for i in first..to {
                            let i = if flip { to - 1 - i } else { i } as i64;
                            assert_eq!(
                                walk.next(),
                                i * from as i64 / to as i64,
                                "{from} over {to} from {first}, flipped {flip}, at {i}"
                            );
                        }
                    }
                }
            }
        }
        // Sizes near the largest a call can name.
        let (from, to) = (i32::MAX, i32::MAX - 7);
        let mut walk = Walk::new(to - 3, from, to, false);
        for i in to - 3..to {
            assert_eq!(walk.next(), i as i64 * from as i64 / to as i64);
        }
    }

    /// `sspr` as it was first written, a division for every pixel: what
    /// the walk and the run must reproduce, pixel for pixel and count for
    /// count.
    #[allow(clippy::too_many_arguments)]
    fn sspr_by_division(
        s: &mut Surface,
        pen: &Pen,
        sheet: &Buf,
        (sx, sy, sw, sh): (i32, i32, i32, i32),
        (dx, dy, dw, dh): (i32, i32, i32, i32),
        (flip_x, flip_y): (bool, bool),
    ) {
        let src = sheet.as_u8().unwrap();
        let (dx, dy) = pen.world(dx, dy);
        let dest = Rect::new(dx, dy, dw, dh).intersect(&pen.clip);
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
                blend(s, pen, px, py, src[(row * sheet_w + col) as usize]);
            }
        }
    }

    #[test]
    fn sspr_draws_what_a_division_for_every_pixel_drew() {
        // A sheet with a different value in every pixel, 0 (transparent
        // by default) among them, and a target that is not blank, so a
        // transparent source pixel is seen to leave what was there.
        let mut sheet = Buf::new(crate::buf::BufKind::U8, 12, 10).unwrap();
        for y in 0..10 {
            for x in 0..12 {
                sheet.set(x, y, ((x * 7 + y * 13) % 16) as f64);
            }
        }
        let mut cases = 0;
        // Source rectangles inside, across and outside the sheet's edges;
        // destinations across every edge of the clip; every flip; at their
        // own size, stretched and shrunk in either direction.
        for (sx, sy, sw, sh) in [
            (0, 0, 8, 8),
            (3, 2, 5, 4),
            (-2, -1, 6, 5),
            (9, 7, 6, 6),
            (12, 0, 4, 4),
        ] {
            for (dw, dh) in [
                (sw, sh),
                (sw, sh * 2),
                (sw * 2, sh),
                (sw * 3 / 2, sh / 2 + 1),
                (1, 1),
            ] {
                for dx in [-9, -3, 0, 2, 11, 14, 16] {
                    for dy in [-7, -1, 0, 5, 13] {
                        for flips in [(false, false), (true, false), (false, true), (true, true)] {
                            for (clip, camera) in [
                                (Rect::new(0, 0, 16, 14), (0, 0)),
                                (Rect::new(3, 2, 9, 8), (0, 0)),
                                (Rect::new(0, 0, 16, 14), (2, -3)),
                            ] {
                                let draw = |walked: bool| {
                                    let (mut buf, mut pen) = surface(16, 14);
                                    buf.as_u8_mut().unwrap().fill(5);
                                    pen.clip = clip;
                                    pen.camera = camera;
                                    let mut s = Surface::of(&mut buf).unwrap();
                                    if walked {
                                        sspr(
                                            &mut s, &pen, &sheet, sx, sy, sw, sh, dx, dy, dw, dh,
                                            flips.0, flips.1,
                                        );
                                    } else {
                                        sspr_by_division(
                                            &mut s,
                                            &pen,
                                            &sheet,
                                            (sx, sy, sw, sh),
                                            (dx, dy, dw, dh),
                                            flips,
                                        );
                                    }
                                    let touched = s.touched;
                                    drop(s);
                                    (pixels(&buf), touched)
                                };
                                assert_eq!(
                                    draw(true),
                                    draw(false),
                                    "source {sx},{sy} {sw}x{sh} to {dx},{dy} {dw}x{dh}, flips {flips:?}, clip {clip:?}, camera {camera:?}"
                                );
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 5 * 5 * 7 * 5 * 4 * 3);
    }

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
        let (mut buf, mut pen) = surface(12, 10);
        assert_eq!(
            pen.font,
            crate::font::FontId::Small,
            "a small target starts small"
        );
        let mut s = Surface::of(&mut buf).unwrap();
        assert_eq!(print(&mut s, &pen, "A", 1, 1, 7), 1 + 8);
        assert_eq!(print(&mut s, &pen, "", 5, 5, 7), 5);
        drop(s);
        let a = ascii(&buf);
        assert_eq!(a[0], "............");
        assert_eq!(a[1], "....77......");
        assert_eq!(a[2], "...7777.....");
        assert_eq!(a[3], "..77..77....");
        assert_eq!(a[5], "..777777....");
        assert_eq!(a[8], "............");
        // Camera and clip apply; the return value is in cart coordinates.
        buf.fill(0.0);
        pen.camera = (1, 1);
        pen.clip = Rect::new(0, 0, 2, 8);
        let mut s = Surface::of(&mut buf).unwrap();
        assert_eq!(print(&mut s, &pen, "AB", 1, 1, 7), 1 + 2 * 8);
        drop(s);
        assert_eq!(ascii(&buf)[0], "............");
        assert_eq!(ascii(&buf)[2], ".7..........");
        // The large face is twice as tall and its wide glyphs advance 16.
        buf.fill(0.0);
        pen.camera = (0, 0);
        pen.clip = Rect::new(0, 0, 12, 10);
        pen.font = crate::font::FontId::Large;
        let mut s = Surface::of(&mut buf).unwrap();
        assert_eq!(print(&mut s, &pen, "|\u{23e9}", 0, 0, 7), 24);
        drop(s);
        assert!(ascii(&buf)[9].starts_with("...7"), "{:?}", ascii(&buf));
        // Far away coordinates are safe.
        let mut s = Surface::of(&mut buf).unwrap();
        print(&mut s, &pen, "hello", i32::MAX - 1, i32::MAX - 1, 7);
        print(&mut s, &pen, "hello", i32::MIN, i32::MIN, 7);
    }
}
