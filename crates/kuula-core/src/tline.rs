//! `tline`: a line of screen pixels that samples the sheet along a line of
//! its own. It draws the spans of a perspective floor (a row, with the
//! sheet stepping across it) and the columns of a textured wall (a column,
//! with the sheet stepping down it) with the same call.
//!
//! The screen line is the clipped DDA the shapes use, walked by an error
//! term; the texture position is a 16.16 fixed-point pair that is stepped
//! and wrapped inside its region. The loop holds additions and
//! comparisons only: the divisions are done once for a line, to find the
//! part of it that is inside the clip and where the texture stands there.
//!
//! Once for a line is still a thousand times a frame for a view drawn in
//! columns and rows, and on a 32-bit processor a division of 128 bits is a
//! long library routine. So the setup divides only where it has to: a row
//! or a column, and any line that starts inside the clip, needs no
//! division for its path, and the texture's position is reduced in 64
//! bits, or not at all when it is in its region already. The 128-bit
//! arithmetic is what is left for a slanted line cut by the clip and for
//! numbers too large for 64 bits.

use crate::buf::Buf;
use crate::pen::Pen;
use crate::raster::Surface;

/// One texel is `1 << FRACTION_BITS` in the fixed-point position and step.
pub const FRACTION_BITS: u32 = 16;

/// Largest magnitude of a position or step, in fixed point. A cart's number
/// beyond it saturates, so the arithmetic below cannot overflow.
const FIXED_LIMIT: f64 = (1u64 << 48) as f64;

/// A texel position or step as the 16.16 fixed-point number the line walks
/// with: floored, saturated, NaN as zero. Where the result fits 32 bits it
/// is made without a library call, as [`crate::buf::to_int`] is.
pub fn to_fixed(v: f64) -> i64 {
    let scaled = v * (1u64 << FRACTION_BITS) as f64;
    if (-2_147_483_648.0..2_147_483_648.0).contains(&scaled) {
        return crate::buf::to_int(scaled);
    }
    if v.is_nan() {
        0
    } else {
        scaled.floor().clamp(-FIXED_LIMIT, FIXED_LIMIT) as i64
    }
}

/// Where the sheet is sampled: a rectangle of it that the position wraps
/// inside, and the position and step in 16.16 fixed point, counted from the
/// rectangle's corner.
#[derive(Debug, Clone, Copy)]
pub struct Texture {
    pub u: i64,
    pub v: i64,
    pub du: i64,
    pub dv: i64,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// How many pixels each step of the line draws, side by side across it:
    /// down from a row, to the right of a column. 1 is a line.
    pub thick: i32,
}

/// `ceil(n / d)` for `d > 0`.
fn ceil_div(n: i128, d: i128) -> i128 {
    -((-n).div_euclid(d))
}

/// `v` reduced into `0..wrap`, without a division when it is there.
fn reduced(v: i64, wrap: i64) -> i32 {
    if (0..wrap).contains(&v) {
        v as i32
    } else {
        v.rem_euclid(wrap) as i32
    }
}

/// `(p + first * step)` reduced into `0..wrap`: where the texture stands at
/// the first step that is drawn. In 64 bits when the product fits them.
fn texel_at(p: i64, first: i64, step: i64, wrap: i64) -> i32 {
    match first.checked_mul(step).and_then(|m| m.checked_add(p)) {
        Some(at) => reduced(at, wrap),
        None => (p as i128 + first as i128 * step as i128).rem_euclid(wrap as i128) as i32,
    }
}

/// The minor axis's offset and the error term at step `first` of a line
/// `n` steps long that moves `along` on its minor axis: the quotient and
/// the remainder of `2 * first * along + n` by `2 * n`. A row, a column
/// and a line drawn from its own first step have 0 and `n`.
fn path_at(first: i64, along: i64, n: i64) -> (i64, i64) {
    if n == 0 {
        return (0, 0);
    }
    if first == 0 || along == 0 {
        return (0, n);
    }
    let at = first
        .checked_mul(along)
        .and_then(|m| m.checked_mul(2))
        .and_then(|m| m.checked_add(n));
    match at {
        Some(at) => (at / (2 * n), at % (2 * n)),
        None => {
            let at = 2 * first as i128 * along as i128 + n as i128;
            ((at / (2 * n as i128)) as i64, (at % (2 * n as i128)) as i64)
        }
    }
}

/// A textured line from `(x0, y0)` to `(x1, y1)`, both ends included, through
/// the colour table and the clip. Step `i` of the `n + 1` (`n` the longer of
/// the two extents) is at texel `(u + i * du, v + i * dv)`, wrapped into the
/// region, which is first cut to the sheet; a region outside the sheet draws
/// nothing. A step draws `thick` pixels of one texel, side by side on the
/// minor axis, counted up from the line's own (down from a row, right of a
/// column). The work is the part of the line inside the clip.
pub fn tline(
    s: &mut Surface,
    pen: &Pen,
    sheet: &Buf,
    (x0, y0, x1, y1): (i32, i32, i32, i32),
    tex: Texture,
) {
    let Some(src) = sheet.as_u8() else {
        return;
    };
    let (sheet_w, sheet_h) = (sheet.width() as i64, sheet.height() as i64);
    let left = (tex.x as i64).max(0);
    let top = (tex.y as i64).max(0);
    let right = (tex.x as i64 + tex.w as i64).min(sheet_w);
    let bottom = (tex.y as i64 + tex.h as i64).min(sheet_h);
    if right <= left || bottom <= top {
        return;
    }
    // The wrap lengths, in fixed point; sheets are at most 4096 wide, so
    // everything below fits 32 bits.
    let (wrap_u, wrap_v) = (
        (right - left) << FRACTION_BITS,
        (bottom - top) << FRACTION_BITS,
    );

    let (x0, y0) = pen.world(x0, y0);
    let (x1, y1) = pen.world(x1, y1);
    let (dx, dy) = (x1 as i64 - x0 as i64, y1 as i64 - y0 as i64);
    let n = dx.abs().max(dy.abs());
    let horizontal = dx.abs() >= dy.abs();
    let thick = (tex.thick as i64).max(1);
    // Along the longer axis (major) every step is one pixel; the other
    // (minor) moves by a rounded share.
    let (m0, md, m_lo, m_hi, c0, cd, c_lo, c_hi) = if horizontal {
        (
            x0 as i64,
            dx,
            pen.clip.x as i64,
            pen.clip.right() as i64 - 1,
            y0 as i64,
            dy,
            pen.clip.y as i64,
            pen.clip.bottom() as i64 - 1,
        )
    } else {
        (
            y0 as i64,
            dy,
            pen.clip.y as i64,
            pen.clip.bottom() as i64 - 1,
            x0 as i64,
            dx,
            pen.clip.x as i64,
            pen.clip.right() as i64 - 1,
        )
    };

    // A step is in the clip when any of its pixels is: the line's own and
    // the `thick - 1` after it on the minor axis.
    let clip_lo = c_lo;
    let c_lo = c_lo.saturating_sub(thick - 1);
    // The steps whose major coordinate is inside the clip.
    let (a, b) = if md >= 0 {
        (m_lo - m0, m_hi - m0)
    } else {
        (m0 - m_hi, m0 - m_lo)
    };
    let (mut first, mut last) = (a.max(0), b.min(n));
    // The minor coordinate of step `i` is `c0 + sign * t(i)` with
    // `t(i) = (2 i |cd| + n) / 2n` rounded down, which runs from 0 to |cd|
    // and never decreases, so the steps inside the clip are one run.
    let along = cd.abs();
    let sign = if cd < 0 { -1 } else { 1 };
    if along == 0 {
        if c0 < c_lo || c0 > c_hi {
            return;
        }
    } else {
        let (t_lo, t_hi) = if sign > 0 {
            (c_lo - c0, c_hi - c0)
        } else {
            (c0 - c_hi, c0 - c_lo)
        };
        if t_hi < 0 || t_lo > along {
            return;
        }
        let (n128, along128) = (n as i128, along as i128);
        if t_lo > 0 {
            let need = ceil_div(2 * n128 * t_lo as i128 - n128, 2 * along128);
            first = first.max(need.min(i64::MAX as i128) as i64);
        }
        if t_hi < along {
            let past = ceil_div(2 * n128 * (t_hi as i128 + 1) - n128, 2 * along128) - 1;
            last = last.min(past.max(-1) as i64);
        }
    }
    if first > last {
        return;
    }

    // The pixel and the error term at the first step.
    let (minor_at, mut err) = path_at(first, along, n);
    let major_at = m0 + if md >= 0 { first } else { -first };
    let minor_at = c0 + sign * minor_at;
    let (px, py) = if horizontal {
        (major_at, minor_at)
    } else {
        (minor_at, major_at)
    };
    // Where the line's own pixel is in the buffer, as an offset. With a
    // thick line that pixel can be far outside the buffer while the pixels
    // beside it are inside, so the offset is kept in 64 bits until it is
    // one that is drawn to: on a 32-bit processor a word would not hold it.
    let width = s.width as i64;
    let major_stride: i64 = match (horizontal, md >= 0) {
        (true, true) => 1,
        (true, false) => -1,
        (false, true) => width,
        (false, false) => -width,
    };
    let minor_stride: i64 = if horizontal { width * sign } else { sign };
    let across: i64 = if horizontal { width } else { 1 };
    let mut at: i64 = py * width + px;

    // The texture position at the first step, reduced into the region.
    let (mut u, mut v) = (
        texel_at(tex.u, first, tex.du, wrap_u),
        texel_at(tex.v, first, tex.dv, wrap_v),
    );
    let (du, dv) = (reduced(tex.du, wrap_u), reduced(tex.dv, wrap_v));
    let (wrap_u, wrap_v) = (wrap_u as i32, wrap_v as i32);

    let (sheet_w, left, top) = (sheet_w as usize, left as usize, top as usize);
    let table = &pen.table;
    if along == 0 {
        // A row or a column, which is every line of a floor and of a wall.
        // It never leaves its minor coordinate, so there is no error term
        // to carry, and which of its `thick` pixels are inside the clip is
        // the same at every step. Nothing here is wider than the machine's
        // word: on a 32-bit processor the general loop below spends most of
        // its time on its 64-bit counters.
        let (k0, k1) = (
            (clip_lo - minor_at).max(0),
            (c_hi - minor_at).min(thick - 1),
        );
        if k0 > k1 {
            return;
        }
        let (steps, wide) = ((last - first + 1) as usize, (k1 - k0 + 1) as usize);
        // The first pixel that is drawn, which is inside the buffer.
        let mut at = (at + k0 * across) as isize;
        let (across, major_stride) = (across as isize, major_stride as isize);
        for _ in 0..steps {
            let texel = (top + (v >> FRACTION_BITS) as usize) * sheet_w
                + left
                + (u >> FRACTION_BITS) as usize;
            let c = src[texel];
            let mut p = at;
            for _ in 0..wide {
                let dst = &mut s.pixels[p as usize];
                *dst = table.lookup(c, *dst);
                p += across;
            }
            at += major_stride;
            u += du;
            if u >= wrap_u {
                u -= wrap_u;
            }
            v += dv;
            if v >= wrap_v {
                v -= wrap_v;
            }
        }
        s.touched += (steps * wide) as u64;
        return;
    }
    let (step, carry) = (2 * along, 2 * n);
    let count = (last - first + 1) as u64;
    if thick > 1 {
        // A step across the line is one pixel on the minor axis.
        let mut minor = minor_at;
        let mut drawn = 0u64;
        for _ in 0..count {
            let texel = (top + (v >> FRACTION_BITS) as usize) * sheet_w
                + left
                + (u >> FRACTION_BITS) as usize;
            let c = src[texel];
            let (k0, k1) = ((clip_lo - minor).max(0), (c_hi - minor).min(thick - 1));
            if k0 <= k1 {
                let mut p = at + k0 * across;
                for _ in k0..=k1 {
                    let dst = &mut s.pixels[p as usize];
                    *dst = table.lookup(c, *dst);
                    p += across;
                }
                drawn += (k1 - k0 + 1) as u64;
            }
            at += major_stride;
            err += step;
            if err >= carry {
                err -= carry;
                at += minor_stride;
                minor += sign;
            }
            u += du;
            if u >= wrap_u {
                u -= wrap_u;
            }
            v += dv;
            if v >= wrap_v {
                v -= wrap_v;
            }
        }
        s.touched += drawn;
        return;
    }
    // A line one pixel thick: every step of it is inside the buffer.
    let mut at = at as isize;
    let (major_stride, minor_stride) = (major_stride as isize, minor_stride as isize);
    for _ in 0..count {
        let texel =
            (top + (v >> FRACTION_BITS) as usize) * sheet_w + left + (u >> FRACTION_BITS) as usize;
        let dst = &mut s.pixels[at as usize];
        *dst = table.lookup(src[texel], *dst);
        at += major_stride;
        err += step;
        if err >= carry {
            err -= carry;
            at += minor_stride;
        }
        u += du;
        if u >= wrap_u {
            u -= wrap_u;
        }
        v += dv;
        if v >= wrap_v {
            v -= wrap_v;
        }
    }
    s.touched += count;
}

#[cfg(test)]
// Tests end a `Surface` borrow with `drop` before reading the buffer.
#[allow(clippy::drop_non_drop)]
mod tests {
    use super::*;
    use crate::buf::{BufKind, Rect};
    use crate::raster::testing::*;

    /// A sheet whose pixel at `(x, y)` is `1 + (x + 3 y) % 15`, so no pixel
    /// is the transparent 0 and neighbours differ.
    fn striped(w: u32, h: u32) -> Buf {
        let mut b = Buf::new(BufKind::U8, w, h).unwrap();
        for y in 0..h {
            for x in 0..w {
                b.set(x as i32, y as i32, (1 + (x + 3 * y) % 15) as f64);
            }
        }
        b
    }

    fn texture(u: f64, v: f64, du: f64, dv: f64, region: (i32, i32, i32, i32)) -> Texture {
        Texture {
            u: to_fixed(u),
            v: to_fixed(v),
            du: to_fixed(du),
            dv: to_fixed(dv),
            x: region.0,
            y: region.1,
            w: region.2,
            h: region.3,
            thick: 1,
        }
    }

    fn thick(tex: Texture, thick: i32) -> Texture {
        Texture { thick, ..tex }
    }

    /// `tline` as it reads from the definition, a division for every pixel
    /// and a test of every pixel against the clip.
    fn tline_by_division(
        s: &mut Surface,
        pen: &Pen,
        sheet: &Buf,
        (x0, y0, x1, y1): (i32, i32, i32, i32),
        tex: Texture,
    ) {
        let src = sheet.as_u8().unwrap();
        let sheet_w = sheet.width() as i64;
        let left = (tex.x as i64).max(0);
        let top = (tex.y as i64).max(0);
        let right = (tex.x as i64 + tex.w as i64).min(sheet_w);
        let bottom = (tex.y as i64 + tex.h as i64).min(sheet.height() as i64);
        if right <= left || bottom <= top {
            return;
        }
        let (wrap_u, wrap_v) = ((right - left) << 16, (bottom - top) << 16);
        let (x0, y0) = pen.world(x0, y0);
        let (x1, y1) = pen.world(x1, y1);
        let (dx, dy) = (x1 as i128 - x0 as i128, y1 as i128 - y0 as i128);
        let n = dx.abs().max(dy.abs());
        let horizontal = dx.abs() >= dy.abs();
        for i in 0..=n {
            let (x, y) = if n == 0 {
                (x0 as i128, y0 as i128)
            } else if horizontal {
                let t = (2 * i * dy.abs() + n) / (2 * n);
                (
                    x0 as i128 + if dx >= 0 { i } else { -i },
                    y0 as i128 + if dy >= 0 { t } else { -t },
                )
            } else {
                let t = (2 * i * dx.abs() + n) / (2 * n);
                (
                    x0 as i128 + if dx >= 0 { t } else { -t },
                    y0 as i128 + if dy >= 0 { i } else { -i },
                )
            };
            let u = (tex.u as i128 + i * tex.du as i128).rem_euclid(wrap_u as i128) as i64;
            let v = (tex.v as i128 + i * tex.dv as i128).rem_euclid(wrap_v as i128) as i64;
            let c = src[((top + (v >> 16)) * sheet_w + left + (u >> 16)) as usize];
            for k in 0..tex.thick.max(1) as i128 {
                let (x, y) = if horizontal { (x, y + k) } else { (x + k, y) };
                if x < pen.clip.x as i128
                    || x >= pen.clip.right() as i128
                    || y < pen.clip.y as i128
                    || y >= pen.clip.bottom() as i128
                {
                    continue;
                }
                s.touched += 1;
                let at = &mut s.pixels[y as usize * s.width as usize + x as usize];
                *at = pen.table.lookup(c, *at);
            }
        }
    }

    #[test]
    fn a_fixed_point_number_floors_and_saturates() {
        assert_eq!(to_fixed(1.0), 65536);
        assert_eq!(to_fixed(-0.5), -32768);
        assert_eq!(to_fixed(0.999999), 65535);
        assert_eq!(to_fixed(f64::NAN), 0);
        assert_eq!(to_fixed(f64::INFINITY), 1 << 48);
        assert_eq!(to_fixed(f64::NEG_INFINITY), -(1 << 48));
        // Whichever way it is made, it is the floor of 65536 times the
        // number, saturated.
        let plainly = |v: f64| {
            if v.is_nan() {
                0
            } else {
                (v * 65536.0).floor().clamp(-FIXED_LIMIT, FIXED_LIMIT) as i64
            }
        };
        let mut v = -40_000.0f64;
        while v < 40_000.0 {
            for scale in [1.0, 1e-3, 1e-6, 1e6, 1e12] {
                let n = v * scale;
                assert_eq!(to_fixed(n), plainly(n), "{n:?}");
            }
            v += 37.218_75;
        }
        for n in [
            32_767.999_99,
            32768.0,
            -32768.0,
            -32_768.000_01,
            1e300,
            -1e300,
        ] {
            assert_eq!(to_fixed(n), plainly(n), "{n:?}");
        }
    }

    #[test]
    fn a_span_steps_across_the_sheet_and_wraps() {
        let sheet = striped(8, 4);
        let (mut buf, pen) = surface(12, 3);
        let mut s = Surface::of(&mut buf).unwrap();
        // Row 0 of the sheet, a texel a pixel, from texel 6 of 8: it wraps.
        tline(
            &mut s,
            &pen,
            &sheet,
            (0, 1, 11, 1),
            texture(6.0, 0.0, 1.0, 0.0, (0, 0, 8, 4)),
        );
        let touched = s.touched;
        drop(s);
        assert_eq!(touched, 12);
        let want: Vec<u8> = (0..12).map(|i| 1 + ((6 + i) % 8) as u8).collect();
        assert_eq!(&pixels(&buf)[12..24], &want[..]);
        assert!(pixels(&buf)[..12].iter().all(|&p| p == 0));
    }

    #[test]
    fn a_column_goes_down_a_region_and_half_steps_repeat_texels() {
        let sheet = striped(8, 8);
        let (mut buf, pen) = surface(3, 8);
        let mut s = Surface::of(&mut buf).unwrap();
        // The 4x2 region at (2, 4), down it at half a texel a pixel.
        tline(
            &mut s,
            &pen,
            &sheet,
            (1, 0, 1, 7),
            texture(1.0, 0.0, 0.0, 0.5, (2, 4, 4, 2)),
        );
        drop(s);
        let col: Vec<u8> = (0..8).map(|y| pixels(&buf)[y * 3 + 1]).collect();
        // Texel column 3 of the sheet, rows 4, 4, 5, 5, 4, 4, 5, 5.
        let at = |row: u32| 1 + (3 + 3 * row) % 15;
        let want: Vec<u8> = [4, 4, 5, 5, 4, 4, 5, 5]
            .iter()
            .map(|&r| at(r) as u8)
            .collect();
        assert_eq!(col, want);
    }

    #[test]
    fn a_line_goes_through_the_colour_table_and_keeps_what_a_transparent_texel_leaves() {
        let mut sheet = Buf::new(BufKind::U8, 4, 1).unwrap();
        for (x, c) in [0.0, 5.0, 6.0, 0.0].into_iter().enumerate() {
            sheet.set(x as i32, 0, c);
        }
        let (mut buf, mut pen) = surface(4, 1);
        buf.fill(9.0);
        pen.table.palt(0, true);
        pen.table.pal_map(6, 7);
        let mut s = Surface::of(&mut buf).unwrap();
        tline(
            &mut s,
            &pen,
            &sheet,
            (0, 0, 3, 0),
            texture(0.0, 0.0, 1.0, 0.0, (0, 0, 4, 1)),
        );
        let touched = s.touched;
        drop(s);
        assert_eq!(pixels(&buf), [9, 5, 7, 9]);
        assert_eq!(touched, 4, "a transparent texel is still touched");
    }

    #[test]
    fn a_region_is_cut_to_the_sheet_and_one_outside_draws_nothing() {
        let sheet = striped(8, 4);
        let (mut buf, pen) = surface(8, 1);
        let mut s = Surface::of(&mut buf).unwrap();
        // Starts left of the sheet: the cut region is the sheet's first 4 columns.
        tline(
            &mut s,
            &pen,
            &sheet,
            (0, 0, 7, 0),
            texture(0.0, 0.0, 1.0, 0.0, (-4, 0, 8, 1)),
        );
        for outside in [(8, 0, 4, 4), (0, 4, 4, 4), (-9, 0, 4, 4), (0, 0, 0, 4)] {
            tline(
                &mut s,
                &pen,
                &sheet,
                (0, 0, 7, 0),
                texture(0.0, 0.0, 1.0, 0.0, outside),
            );
        }
        let touched = s.touched;
        drop(s);
        assert_eq!(touched, 8);
        assert_eq!(pixels(&buf), [1, 2, 3, 4, 1, 2, 3, 4]);
    }

    #[test]
    fn tline_draws_what_a_division_for_every_pixel_drew() {
        let sheet = striped(12, 10);
        let mut cases = 0;
        // Ends inside, across and beyond every edge of the target and the
        // clip, in every direction and at every slope, with steps that wrap
        // once or many times, backwards and by fractions.
        let ends: [(i32, i32); 9] = [
            (0, 0),
            (15, 13),
            (3, 7),
            (-9, 4),
            (7, -11),
            (24, 5),
            (6, 21),
            (-30, -30),
            (9, 9),
        ];
        let steps = [
            (0.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (0.37, -0.81),
            (-2.5, 3.25),
            (13.9, 0.1),
        ];
        let regions = [(0, 0, 12, 10), (2, 3, 5, 4), (-3, -2, 8, 7), (7, 6, 40, 40)];
        for &(x0, y0) in &ends {
            for &(x1, y1) in &ends {
                for &(du, dv) in &steps {
                    for region in regions {
                        for thickness in [1, 2, 5] {
                            for (clip, camera) in [
                                (Rect::new(0, 0, 16, 14), (0, 0)),
                                (Rect::new(3, 2, 9, 8), (0, 0)),
                                (Rect::new(0, 0, 16, 14), (2, -3)),
                            ] {
                                let draw = |divided: bool| {
                                    let (mut buf, mut pen) = surface(16, 14);
                                    buf.as_u8_mut().unwrap().fill(5);
                                    pen.clip = clip;
                                    pen.camera = camera;
                                    let tex = thick(texture(2.75, -1.5, du, dv, region), thickness);
                                    let mut s = Surface::of(&mut buf).unwrap();
                                    if divided {
                                        tline_by_division(
                                            &mut s,
                                            &pen,
                                            &sheet,
                                            (x0, y0, x1, y1),
                                            tex,
                                        );
                                    } else {
                                        tline(&mut s, &pen, &sheet, (x0, y0, x1, y1), tex);
                                    }
                                    let touched = s.touched;
                                    drop(s);
                                    (pixels(&buf), touched)
                                };
                                assert_eq!(
                                draw(false),
                                draw(true),
                                "({x0},{y0}) to ({x1},{y1}), step ({du},{dv}), region {region:?}, thickness {thickness}, clip {clip:?}, camera {camera:?}"
                            );
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 9 * 9 * 6 * 4 * 3 * 3);
    }

    #[test]
    fn the_work_is_the_clip_not_the_line() {
        let sheet = striped(8, 8);
        let (mut buf, pen) = surface(20, 10);
        let tex = texture(0.0, 0.0, 1.0, 1.0, (0, 0, 8, 8));
        for ends in [
            (i32::MIN, 4, i32::MAX, 4),
            (3, i32::MIN, 3, i32::MAX),
            (i32::MIN, i32::MIN, i32::MAX, i32::MAX),
            (i32::MIN, 1, i32::MAX, i32::MAX),
            (i32::MAX, 3, i32::MIN, i32::MIN),
        ] {
            let mut s = Surface::of(&mut buf).unwrap();
            tline(&mut s, &pen, &sheet, ends, tex);
            assert!(s.touched <= 20, "{ends:?}: {}", s.touched);
        }
        // Wholly off to one side, with the longer extent inside the clip.
        let mut s = Surface::of(&mut buf).unwrap();
        tline(&mut s, &pen, &sheet, (0, -5, 19, -5), tex);
        tline(&mut s, &pen, &sheet, (0, 10, 19, 10), tex);
        assert_eq!(s.touched, 0);
        // Huge steps and positions are only a wrap.
        tline(
            &mut s,
            &pen,
            &sheet,
            (0, 0, 19, 0),
            Texture {
                u: i64::MAX / 4,
                v: i64::MIN / 4,
                du: i64::MAX / 4,
                dv: i64::MIN / 4,
                x: 0,
                y: 0,
                w: 8,
                h: 8,
                thick: 1,
            },
        );
        assert_eq!(s.touched, 20);
        // However thick, a line touches no more than the clip holds.
        let mut s = Surface::of(&mut buf).unwrap();
        for ends in [
            (i32::MIN, 4, i32::MAX, 4),
            (3, i32::MIN, 3, i32::MAX),
            (0, 0, 19, 9),
        ] {
            tline(&mut s, &pen, &sheet, ends, thick(tex, i32::MAX));
        }
        assert!(s.touched <= 3 * 20 * 10, "{}", s.touched);
        let mut s = Surface::of(&mut buf).unwrap();
        tline(&mut s, &pen, &sheet, (0, 4, 19, 4), thick(tex, i32::MAX));
        assert_eq!(
            s.touched,
            20 * 6,
            "a row from the 5th down to the bottom of 10"
        );
    }

    #[test]
    fn a_thick_line_draws_each_texel_across_its_width() {
        let sheet = striped(8, 4);
        // A row, three thick: each texel down three rows, and the rows
        // that are off the target are not drawn.
        let (mut buf, pen) = surface(8, 4);
        let mut s = Surface::of(&mut buf).unwrap();
        tline(
            &mut s,
            &pen,
            &sheet,
            (0, 2, 7, 2),
            thick(texture(0.0, 0.0, 1.0, 0.0, (0, 0, 8, 1)), 3),
        );
        let touched = s.touched;
        drop(s);
        assert_eq!(touched, 16, "two rows of eight");
        let row: Vec<u8> = (1..=8).collect();
        assert_eq!(&pixels(&buf)[..16], vec![0u8; 16].as_slice());
        assert_eq!(&pixels(&buf)[16..24], row.as_slice());
        assert_eq!(&pixels(&buf)[24..], row.as_slice());
        // A column, two thick: the pair of pixel columns share a texel a row.
        let (mut buf, pen) = surface(6, 3);
        let mut s = Surface::of(&mut buf).unwrap();
        tline(
            &mut s,
            &pen,
            &sheet,
            (2, 0, 2, 2),
            thick(texture(0.0, 0.0, 0.0, 1.0, (0, 0, 1, 4)), 2),
        );
        drop(s);
        let at = |row: usize| 1 + ((3 * row) % 15) as u8;
        let want = [at(0), at(0), at(1), at(1), at(2), at(2)];
        let got: Vec<u8> = (0..3)
            .flat_map(|y| [pixels(&buf)[y * 6 + 2], pixels(&buf)[y * 6 + 3]])
            .collect();
        assert_eq!(got, want);
        // A thick line that starts above the clip reaches into it.
        let (mut buf, mut pen) = surface(4, 4);
        pen.clip = Rect::new(0, 2, 4, 2);
        let mut s = Surface::of(&mut buf).unwrap();
        tline(
            &mut s,
            &pen,
            &sheet,
            (0, 1, 3, 1),
            thick(texture(0.0, 0.0, 1.0, 0.0, (0, 0, 8, 1)), 2),
        );
        let touched = s.touched;
        drop(s);
        assert_eq!(touched, 4);
        assert_eq!(&pixels(&buf)[8..12], [1, 2, 3, 4]);
    }
}
