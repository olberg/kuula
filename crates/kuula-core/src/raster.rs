//! The software rasteriser: every primitive
//! written once against a borrowed `u8` surface, with the pen's clip,
//! camera, fill pattern and colour table applied in that order.
//!
//! Shapes live here; sheet and text blits are in [`crate::blit`], and the
//! pen state they all read is in [`crate::pen`].

use crate::buf::{Buf, Rect};
pub use crate::pen::{Colour, ColourTable, Fillp, Pen};

/// Circles larger than this are clamped; the loops stay bounded.
const MAX_RADIUS: i32 = 1 << 15;

/// A borrowed rectangle of colour indices.
pub struct Surface<'a> {
    pub pixels: &'a mut [u8],
    pub width: i32,
    pub height: i32,
    /// Pixels written or considered after clipping, for the cycle meter
    /// (drawing is priced by pixels touched after
    /// clipping). A transparent source pixel still counts; an off-clip
    /// one never does.
    pub touched: u64,
}

impl<'a> Surface<'a> {
    pub fn of(buf: &'a mut Buf) -> Option<Surface<'a>> {
        let width = buf.width() as i32;
        let height = buf.height() as i32;
        Some(Surface {
            pixels: buf.as_u8_mut()?,
            width,
            height,
            touched: 0,
        })
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    #[inline]
    fn at(&mut self, x: i32, y: i32) -> &mut u8 {
        &mut self.pixels[y as usize * self.width as usize + x as usize]
    }

    /// Read a pixel; 0 outside.
    pub fn get(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            0
        } else {
            self.pixels[y as usize * self.width as usize + x as usize]
        }
    }
}

/// Write one shape pixel: clip, fill pattern, colour table.
#[inline]
fn plot(s: &mut Surface, pen: &Pen, x: i32, y: i32, c: Colour) {
    if !pen.clip.contains(x, y) {
        return;
    }
    s.touched += 1;
    let src = match pen.fillp.pick(x, y) {
        Some(false) => c.primary,
        Some(true) => c.secondary,
        None => return,
    };
    let dst = s.at(x, y);
    *dst = pen.table.lookup(src, *dst);
}

/// Write one sprite or text pixel: clip and colour table, no pattern.
#[inline]
pub(crate) fn blend(s: &mut Surface, pen: &Pen, x: i32, y: i32, src: u8) {
    if !pen.clip.contains(x, y) {
        return;
    }
    s.touched += 1;
    let dst = s.at(x, y);
    *dst = pen.table.lookup(src, *dst);
}

/// Fill the clip rectangle directly, bypassing the table and pattern.
pub fn cls(s: &mut Surface, pen: &Pen, c: u8) {
    let r = pen.clip.intersect(&s.bounds());
    if r.is_empty() {
        return;
    }
    let c = c & 0x7f;
    for y in r.y..r.bottom() {
        let row = y as usize * s.width as usize;
        s.pixels[row + r.x as usize..row + r.right() as usize].fill(c);
    }
    s.touched += r.w as u64 * r.h as u64;
}

pub fn pset(s: &mut Surface, pen: &Pen, x: i32, y: i32, c: Colour) {
    let (x, y) = pen.world(x, y);
    plot(s, pen, x, y, c);
}

/// Reads the target directly: no camera, no clip.
pub fn pget(s: &Surface, x: i32, y: i32) -> u8 {
    s.get(x, y)
}

pub fn rectfill(s: &mut Surface, pen: &Pen, x0: i32, y0: i32, x1: i32, y1: i32, c: Colour) {
    let (x0, y0) = pen.world(x0, y0);
    let (x1, y1) = pen.world(x1, y1);
    let (left, right) = (x0.min(x1), x0.max(x1));
    let (top, bottom) = (y0.min(y1), y0.max(y1));
    let r = Rect::new(
        left,
        top,
        right.saturating_sub(left).saturating_add(1),
        bottom.saturating_sub(top).saturating_add(1),
    )
    .intersect(&pen.clip);
    if r.is_empty() {
        return;
    }
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            plot(s, pen, x, y, c);
        }
    }
}

pub fn rect(s: &mut Surface, pen: &Pen, x0: i32, y0: i32, x1: i32, y1: i32, c: Colour) {
    let (x0, y0) = pen.world(x0, y0);
    let (x1, y1) = pen.world(x1, y1);
    let (left, right) = (x0.min(x1), x0.max(x1));
    let (top, bottom) = (y0.min(y1), y0.max(y1));
    hspan(s, pen, left, right, top, c);
    if bottom != top {
        hspan(s, pen, left, right, bottom, c);
    }
    if bottom > top + 1 {
        vspan(s, pen, left, top + 1, bottom - 1, c);
        if right != left {
            vspan(s, pen, right, top + 1, bottom - 1, c);
        }
    }
}

/// Horizontal run in target coordinates, inclusive, clipped.
fn hspan(s: &mut Surface, pen: &Pen, x0: i32, x1: i32, y: i32, c: Colour) {
    if y < pen.clip.y || y >= pen.clip.bottom() {
        return;
    }
    let x0 = x0.max(pen.clip.x);
    let x1 = x1.min(pen.clip.right() - 1);
    for x in x0..=x1 {
        plot(s, pen, x, y, c);
    }
}

fn vspan(s: &mut Surface, pen: &Pen, x: i32, y0: i32, y1: i32, c: Colour) {
    if x < pen.clip.x || x >= pen.clip.right() {
        return;
    }
    let y0 = y0.max(pen.clip.y);
    let y1 = y1.min(pen.clip.bottom() - 1);
    for y in y0..=y1 {
        plot(s, pen, x, y, c);
    }
}

/// A DDA line, inclusive of both ends, walking only the part of the
/// major axis inside the clip so a long line costs the clip, not its
/// length.
pub fn line(s: &mut Surface, pen: &Pen, x0: i32, y0: i32, x1: i32, y1: i32, c: Colour) {
    let (x0, y0) = pen.world(x0, y0);
    let (x1, y1) = pen.world(x1, y1);
    let (dx, dy) = (x1 as i64 - x0 as i64, y1 as i64 - y0 as i64);
    let steps = dx.abs().max(dy.abs());
    if steps == 0 {
        plot(s, pen, x0, y0, c);
        return;
    }
    // Parameter range where the major coordinate is inside the clip.
    let (major0, majord, lo, hi) = if dx.abs() >= dy.abs() {
        (
            x0 as i64,
            dx,
            pen.clip.x as i64,
            pen.clip.right() as i64 - 1,
        )
    } else {
        (
            y0 as i64,
            dy,
            pen.clip.y as i64,
            pen.clip.bottom() as i64 - 1,
        )
    };
    let (a, b) = if majord > 0 {
        (lo - major0, hi - major0)
    } else {
        (major0 - hi, major0 - lo)
    };
    let i0 = a.max(0);
    let i1 = b.min(steps);
    if i0 > i1 {
        return;
    }
    for i in i0..=i1 {
        let x = x0 as i64 + round_div(dx as i128 * i as i128, steps as i128) as i64;
        let y = y0 as i64 + round_div(dy as i128 * i as i128, steps as i128) as i64;
        plot(s, pen, x as i32, y as i32, c);
    }
}

/// `n / d` rounded to nearest, halves away from zero; `d > 0`.
fn round_div(n: i128, d: i128) -> i128 {
    if n >= 0 {
        (n + d / 2) / d
    } else {
        -((-n + d / 2) / d)
    }
}

pub fn circ(s: &mut Surface, pen: &Pen, cx: i32, cy: i32, r: i32, c: Colour) {
    let (cx, cy) = pen.world(cx, cy);
    let r = r.clamp(0, MAX_RADIUS);
    if r == 0 {
        plot(s, pen, cx, cy, c);
        return;
    }
    if !circle_visible(pen, cx, cy, r) {
        return;
    }
    // Midpoint circle.
    let (mut x, mut y) = (r, 0);
    let mut err = 1 - r;
    while x >= y {
        for (px, py) in [
            (cx + x, cy + y),
            (cx - x, cy + y),
            (cx + x, cy - y),
            (cx - x, cy - y),
            (cx + y, cy + x),
            (cx - y, cy + x),
            (cx + y, cy - x),
            (cx - y, cy - x),
        ] {
            plot(s, pen, px, py, c);
        }
        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
}

pub fn circfill(s: &mut Surface, pen: &Pen, cx: i32, cy: i32, r: i32, c: Colour) {
    let (cx, cy) = pen.world(cx, cy);
    let r = r.clamp(0, MAX_RADIUS);
    if !circle_visible(pen, cx, cy, r) {
        return;
    }
    // Scanlines inside the clip; the half-width comes from the same
    // integer circle the outline uses so `circ` sits on `circfill`.
    let widths = half_widths(r);
    let y0 = (cy - r).max(pen.clip.y);
    let y1 = (cy + r).min(pen.clip.bottom() - 1);
    for y in y0..=y1 {
        let hw = widths[(y - cy).unsigned_abs() as usize];
        hspan(s, pen, cx - hw, cx + hw, y, c);
    }
}

/// Pixels' worth of work a circle of radius `r` costs before clipping:
/// the midpoint walk plots about six points per unit of radius.
pub fn circle_work(r: i32) -> u64 {
    r.clamp(0, MAX_RADIUS) as u64 * crate::meter::price::CIRCLE_WORK_PER_RADIUS
}

fn circle_visible(pen: &Pen, cx: i32, cy: i32, r: i32) -> bool {
    let bbox = Rect::new(
        cx.saturating_sub(r),
        cy.saturating_sub(r),
        r.saturating_mul(2).saturating_add(1),
        r.saturating_mul(2).saturating_add(1),
    );
    !bbox.intersect(&pen.clip).is_empty()
}

/// Half-width of the midpoint circle at each `|dy|` from 0 to `r`.
fn half_widths(r: i32) -> Vec<i32> {
    let mut w = vec![0; r as usize + 1];
    let (mut x, mut y) = (r, 0);
    let mut err = 1 - r;
    while x >= y {
        w[y as usize] = w[y as usize].max(x);
        w[x as usize] = w[x as usize].max(y);
        y += 1;
        if err < 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err += 2 * (y - x) + 1;
        }
    }
    w
}

/// Shared fixtures for the rasteriser and blit tests.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::buf::BufKind;
    use crate::resources::BufId;

    pub(crate) fn surface(w: u32, h: u32) -> (Buf, Pen) {
        let buf = Buf::new(BufKind::U8, w, h).unwrap();
        let pen = Pen::new(BufId::SCREEN, w, h);
        (buf, pen)
    }

    pub(crate) fn pixels(buf: &Buf) -> Vec<u8> {
        buf.as_u8().unwrap().to_vec()
    }

    /// Rows as strings of hex digits, '.' for zero.
    pub(crate) fn ascii(buf: &Buf) -> Vec<String> {
        let w = buf.width() as usize;
        pixels(buf)
            .chunks(w)
            .map(|row| {
                row.iter()
                    .map(|&p| {
                        if p == 0 {
                            '.'
                        } else {
                            char::from_digit(p as u32 % 16, 16).unwrap()
                        }
                    })
                    .collect()
            })
            .collect()
    }

    pub(crate) fn sheet_4x4() -> Buf {
        // 8x8 sheet: cell 0 is a 2x2 checker of 1/2 with 0 elsewhere,
        // cell 1 is solid 3.
        let mut b = Buf::new(BufKind::U8, 16, 8).unwrap();
        b.set(0, 0, 1.0);
        b.set(1, 1, 2.0);
        for y in 0..8 {
            for x in 8..16 {
                b.set(x, y, 3.0);
            }
        }
        b
    }
}

#[cfg(test)]
// Tests end a `Surface` borrow with `drop` before reading the buffer.
#[allow(clippy::drop_non_drop)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn pset_pget_camera_and_clip() {
        let (mut buf, mut pen) = surface(8, 8);
        {
            let mut s = Surface::of(&mut buf).unwrap();
            pset(&mut s, &pen, 1, 1, Colour::solid(5));
            assert_eq!(pget(&s, 1, 1), 5);
            assert_eq!(pget(&s, -1, 1), 0);
            assert_eq!(pget(&s, 8, 8), 0);
        }
        pen.camera = (2, 3);
        {
            let mut s = Surface::of(&mut buf).unwrap();
            pset(&mut s, &pen, 5, 5, Colour::solid(6));
            assert_eq!(pget(&s, 3, 2), 6, "camera subtracts");
            assert_eq!(pget(&s, 5, 5), 0);
        }
        pen.camera = (0, 0);
        pen.clip = Rect::new(2, 2, 2, 2);
        {
            let mut s = Surface::of(&mut buf).unwrap();
            pset(&mut s, &pen, 0, 0, Colour::solid(7));
            pset(&mut s, &pen, 3, 3, Colour::solid(7));
            pset(&mut s, &pen, 4, 4, Colour::solid(7));
            assert_eq!(pget(&s, 0, 0), 0);
            assert_eq!(pget(&s, 3, 3), 7);
            assert_eq!(pget(&s, 4, 4), 0);
        }
    }

    #[test]
    fn cls_fills_only_the_clip_and_ignores_the_table() {
        let (mut buf, mut pen) = surface(4, 4);
        pen.table.palt(3, true);
        pen.fillp = Fillp {
            pattern: 0xffff,
            transparent: true,
        };
        pen.clip = Rect::new(1, 1, 2, 2);
        let mut s = Surface::of(&mut buf).unwrap();
        cls(&mut s, &pen, 3 | 0x80);
        drop(s);
        assert_eq!(ascii(&buf), ["....", ".33.", ".33.", "...."]);
    }

    #[test]
    fn rectfill_and_rect_in_any_corner_order() {
        let (mut buf, pen) = surface(6, 5);
        let mut s = Surface::of(&mut buf).unwrap();
        rectfill(&mut s, &pen, 4, 3, 1, 1, Colour::solid(2));
        drop(s);
        assert_eq!(
            ascii(&buf),
            ["......", ".2222.", ".2222.", ".2222.", "......"]
        );
        buf.fill(0.0);
        let mut s = Surface::of(&mut buf).unwrap();
        rect(&mut s, &pen, 4, 3, 1, 1, Colour::solid(2));
        rect(&mut s, &pen, 0, 4, 0, 4, Colour::solid(9));
        drop(s);
        assert_eq!(
            ascii(&buf),
            ["......", ".2222.", ".2..2.", ".2222.", "9....."]
        );
        // Partially and fully off the surface.
        buf.fill(0.0);
        let mut s = Surface::of(&mut buf).unwrap();
        rectfill(&mut s, &pen, -5, -5, 0, 0, Colour::solid(1));
        rectfill(&mut s, &pen, 100, 100, 200, 200, Colour::solid(1));
        rect(&mut s, &pen, -1, -1, 6, 5, Colour::solid(1));
        drop(s);
        assert_eq!(pixels(&buf).iter().filter(|&&p| p == 1).count(), 1);
    }

    #[test]
    fn fill_pattern_picks_secondary_or_skips() {
        let (mut buf, mut pen) = surface(4, 2);
        // Checkerboard: bits at even (x + y).
        pen.fillp = Fillp {
            pattern: 0b0101_1010_0101_1010,
            transparent: false,
        };
        let mut s = Surface::of(&mut buf).unwrap();
        rectfill(&mut s, &pen, 0, 0, 3, 1, Colour::from_i64(0x0201));
        drop(s);
        assert_eq!(ascii(&buf), ["1212", "2121"]);
        pen.fillp.transparent = true;
        buf.fill(9.0);
        let mut s = Surface::of(&mut buf).unwrap();
        rectfill(&mut s, &pen, 0, 0, 3, 1, Colour::from_i64(0x0201));
        drop(s);
        assert_eq!(ascii(&buf), ["1919", "9191"]);
        // The pattern is anchored to target coordinates, not the shape.
        buf.fill(0.0);
        let mut s = Surface::of(&mut buf).unwrap();
        rectfill(&mut s, &pen, 1, 0, 3, 1, Colour::from_i64(0x0201));
        drop(s);
        assert_eq!(ascii(&buf), ["..1.", ".1.1"]);
    }

    #[test]
    fn line_endpoints_slopes_and_long_lines() {
        let (mut buf, pen) = surface(5, 5);
        let mut s = Surface::of(&mut buf).unwrap();
        line(&mut s, &pen, 0, 0, 4, 4, Colour::solid(1));
        line(&mut s, &pen, 4, 0, 0, 2, Colour::solid(2));
        line(&mut s, &pen, 2, 2, 2, 2, Colour::solid(3));
        drop(s);
        assert_eq!(ascii(&buf), ["1...2", ".122.", "223..", "...1.", "....1"]);
        buf.fill(0.0);
        let mut s = Surface::of(&mut buf).unwrap();
        // A line far longer than the surface still draws the visible
        // segment and returns promptly.
        line(
            &mut s,
            &pen,
            -1_000_000_000,
            2,
            1_000_000_000,
            2,
            Colour::solid(4),
        );
        line(&mut s, &pen, 1, i32::MIN, 1, i32::MAX, Colour::solid(5));
        line(
            &mut s,
            &pen,
            -1_000_000,
            -1_000_000,
            1_000_000,
            1_000_000,
            Colour::solid(6),
        );
        drop(s);
        assert_eq!(ascii(&buf), ["65...", ".6...", "45644", ".5.6.", ".5..6"]);
    }

    #[test]
    fn circles_are_symmetric_and_filled_matches_outline() {
        let (mut buf, pen) = surface(11, 11);
        let mut s = Surface::of(&mut buf).unwrap();
        circ(&mut s, &pen, 5, 5, 4, Colour::solid(1));
        drop(s);
        let a = ascii(&buf);
        for y in 0..11 {
            assert_eq!(a[y], a[10 - y], "vertical symmetry row {y}");
            let rev: String = a[y].chars().rev().collect();
            assert_eq!(a[y], rev, "horizontal symmetry row {y}");
        }
        assert_eq!(a[5], ".1.......1.");
        assert_eq!(a[1], "....111....");
        let outline = pixels(&buf);
        buf.fill(0.0);
        let mut s = Surface::of(&mut buf).unwrap();
        circfill(&mut s, &pen, 5, 5, 4, Colour::solid(1));
        drop(s);
        let filled = pixels(&buf);
        for (i, (&o, &f)) in outline.iter().zip(&filled).enumerate() {
            if o == 1 {
                assert_eq!(f, 1, "outline pixel {i} is inside the fill");
            }
        }
        // Half-widths of the midpoint circle at r = 4 are 4, 4, 3, 3, 1.
        assert_eq!(filled.iter().filter(|&&p| p == 1).count(), 61);
        // Radius 0 is a point; huge radii do not hang.
        buf.fill(0.0);
        let mut s = Surface::of(&mut buf).unwrap();
        circ(&mut s, &pen, 2, 2, 0, Colour::solid(2));
        circfill(&mut s, &pen, 5, 5, i32::MAX, Colour::solid(3));
        circ(&mut s, &pen, 5, 5, i32::MAX, Colour::solid(4));
        circ(&mut s, &pen, 1_000_000, 5, 10, Colour::solid(4));
        drop(s);
        assert_eq!(pixels(&buf).iter().filter(|&&p| p == 3).count(), 121);
    }
}
