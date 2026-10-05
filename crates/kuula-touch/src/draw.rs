//! The controls, drawn translucent over the picture.

use kuula_core::input::{
    BTN_A, BTN_B, BTN_DOWN, BTN_L1, BTN_L2, BTN_LEFT, BTN_MENU, BTN_R1, BTN_R2, BTN_RIGHT,
    BTN_SELECT, BTN_START, BTN_UP, BTN_X, BTN_Y,
};

use crate::canvas::Canvas;
use crate::layout::{Layout, Rect};

/// The light the controls are drawn in.
const LIGHT: [u8; 3] = [236, 238, 244];
/// The dark under the D-pad's cross and in the letters and the bars.
const DARK: [u8; 3] = [24, 26, 34];

/// Opacity of an idle control, out of 255: about 0.45.
pub const IDLE_ALPHA: u8 = 115;
/// Opacity of a held control, out of 255: about 0.88.
pub const HELD_ALPHA: u8 = 224;
/// Opacity of the D-pad's disc under its cross, about 0.30.
const PAD_BODY_ALPHA: u8 = 77;
/// Opacity of the letters and the bars, about 0.75.
const MARK_ALPHA: u8 = 190;
/// What is left of every opacity over the picture, out of 255: about 0.55.
/// A control that could not be kept off the picture lets it show through.
pub const OVER_PICTURE: u8 = 140;

/// A letter or a digit as 5 by 7 cells, a row to a byte with the leftmost
/// cell in bit 4.
type Glyph = [u8; 7];
const A: Glyph = [
    0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
];
const B: Glyph = [
    0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
];
const C: Glyph = [
    0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
];
const E: Glyph = [
    0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
];
const L: Glyph = [
    0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
];
const R: Glyph = [
    0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
];
const S: Glyph = [
    0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
];
const T: Glyph = [
    0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
];
const X: Glyph = [
    0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
];
const Y: Glyph = [
    0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
];
const ONE: Glyph = [
    0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
];
const TWO: Glyph = [
    0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
];

/// What is written on the control of a button.
fn label(bit: u16) -> &'static [Glyph] {
    match bit {
        BTN_A => &[A],
        BTN_B => &[B],
        BTN_X => &[X],
        BTN_Y => &[Y],
        BTN_L1 => &[L, ONE],
        BTN_R1 => &[R, ONE],
        BTN_L2 => &[L, TWO],
        BTN_R2 => &[R, TWO],
        BTN_START => &[S, T, A, R, T],
        BTN_SELECT => &[S, E, L, E, C, T],
        _ => &[],
    }
}

/// Draw the controls over what `present` left: the D-pad as a disc with a
/// cross on it, A and B as discs with their letters and Menu as a pill
/// with three bars, and of the rest those the layout shows: X and Y as
/// discs, the shoulder buttons, Start and Select as pills with their
/// names. Where a window is too small to keep them apart the round
/// controls are over the pills and Menu is over everything, which is the
/// order a finger is given them in. Each is translucent, [`IDLE_ALPHA`]
/// opaque, and
/// [`HELD_ALPHA`] when its bit is in `pressed`; of the D-pad the arm of
/// each held direction is lit. A control's pixels that lie over the
/// picture keep [`OVER_PICTURE`] of their opacity, held or not, so the
/// picture shows through where the window had no room beside it.
/// Everything is clipped to the canvas, so a control that is partly or
/// wholly outside it is drawn as far as it is inside. Blending is integer
/// arithmetic.
pub fn draw_controls(canvas: &mut Canvas, layout: &Layout, pressed: u16) {
    let (cw, ch) = canvas.visible();
    if cw == 0 || ch == 0 {
        return;
    }
    let mut pen = Pen {
        canvas,
        cw,
        ch,
        picture: layout.picture(),
    };
    let alpha = |held: bool| if held { HELD_ALPHA } else { IDLE_ALPHA };

    for (bit, key) in layout.keys() {
        pen.round(key, key.h / 2, LIGHT, alpha(pressed & bit != 0));
        // A name of two letters is written larger than a word.
        let cell = if label(bit).len() <= 2 {
            key.h / 12
        } else {
            key.h / 14
        };
        pen.text(
            label(bit),
            key.x + key.w / 2,
            key.y + key.h / 2,
            cell.max(1),
        );
    }

    // The D-pad: a dark disc, and on it a cross of five pieces that do not
    // overlap, so every piece blends once.
    let pad = layout.dpad();
    pen.round(pad.bounds(), pad.r, DARK, PAD_BODY_ALPHA);
    let t = pad.r * 21 / 100;
    let reach = pad.r * 78 / 100;
    let arm = reach - t;
    let (cx, cy) = (pad.cx, pad.cy);
    let any = pressed & (BTN_UP | BTN_DOWN | BTN_LEFT | BTN_RIGHT) != 0;
    let piece = |x, y, w, h| Rect { x, y, w, h };
    pen.rect(piece(cx - t, cy - t, 2 * t, 2 * t), LIGHT, alpha(any));
    pen.rect(
        piece(cx - t, cy - reach, 2 * t, arm),
        LIGHT,
        alpha(pressed & BTN_UP != 0),
    );
    pen.rect(
        piece(cx - t, cy + t, 2 * t, arm),
        LIGHT,
        alpha(pressed & BTN_DOWN != 0),
    );
    pen.rect(
        piece(cx - reach, cy - t, arm, 2 * t),
        LIGHT,
        alpha(pressed & BTN_LEFT != 0),
    );
    pen.rect(
        piece(cx + t, cy - t, arm, 2 * t),
        LIGHT,
        alpha(pressed & BTN_RIGHT != 0),
    );

    for (bit, disc) in layout.faces() {
        pen.round(disc.bounds(), disc.r, LIGHT, alpha(pressed & bit != 0));
        pen.text(label(bit), disc.cx, disc.cy, (disc.r / 7).max(1));
    }

    let menu = layout.menu();
    pen.round(menu, menu.h / 2, LIGHT, alpha(pressed & BTN_MENU != 0));
    let bar_w = menu.w * 2 / 5;
    let bar_h = (menu.h / 11).max(1);
    let step = menu.h / 5;
    let (mx, my) = (menu.x + menu.w / 2, menu.y + menu.h / 2);
    for i in -1..=1 {
        let bar = Rect {
            x: mx - bar_w / 2,
            y: my + i * step - bar_h / 2,
            w: bar_w,
            h: bar_h,
        };
        pen.rect(bar, DARK, MARK_ALPHA);
    }
}

/// The canvas and the part of it that can be drawn to.
struct Pen<'c, 'a> {
    canvas: &'c mut Canvas<'a>,
    cw: usize,
    ch: usize,
    /// Where the console's frame is: what is drawn over it is fainter.
    picture: Rect,
}

impl Pen<'_, '_> {
    /// Blend `colour` into pixel (`x`, `y`) at `alpha` out of 255, which
    /// must be inside.
    fn blend(&mut self, x: usize, y: usize, colour: [u8; 3], alpha: u32) {
        let at = self.canvas.offset(x, y);
        let px = &mut self.canvas.data[at..at + 3];
        for (d, &s) in px.iter_mut().zip(&colour) {
            *d = div255(s as u32 * alpha + *d as u32 * (255 - alpha)) as u8;
        }
    }

    /// Row `y` from `x0` to `x1` in the pieces beside the picture and
    /// over it, each with whether it is over: the picture's edges are
    /// found once a row, not asked of every pixel.
    fn pieces(&self, y: usize, x0: usize, x1: usize) -> [(usize, usize, bool); 3] {
        let p = self.picture;
        if (y as i32) < p.y || (y as i32) >= p.bottom() {
            return [(x0, x1, false), (x1, x1, false), (x1, x1, false)];
        }
        let from = (p.x.max(0) as usize).clamp(x0, x1);
        let to = (p.right().max(0) as usize).clamp(from, x1);
        [(x0, from, false), (from, to, true), (to, x1, false)]
    }

    /// A rectangle in one colour at one opacity, fainter over the picture.
    fn rect(&mut self, r: Rect, colour: [u8; 3], alpha: u8) {
        let x0 = r.x.max(0) as usize;
        let x1 = (r.right().max(0) as usize).min(self.cw).max(x0);
        let y0 = r.y.max(0) as usize;
        let y1 = (r.bottom().max(0) as usize).min(self.ch);
        let alphas = [alpha as u32, fainter(alpha)];
        for y in y0..y1 {
            for (from, to, over) in self.pieces(y, x0, x1) {
                for x in from..to {
                    self.blend(x, y, colour, alphas[over as usize]);
                }
            }
        }
    }

    /// A rectangle with corners of radius `r`: a disc when it is square and
    /// `r` is half its side, a pill when `r` is half its height. The edge
    /// is one pixel soft, and what is over the picture is fainter.
    fn round(&mut self, bounds: Rect, r: i32, colour: [u8; 3], alpha: u8) {
        // Measured in half pixels from the rectangle's centre, so that the
        // shape is symmetrical about it: a pixel's centre is at an odd
        // number, the rectangle's edge at `w` or `h`.
        let radius = 2 * r;
        let (long_x, long_y) = ((bounds.w - radius).max(0), (bounds.h - radius).max(0));
        let inner = (radius - 1).max(0) * (radius - 1).max(0);
        let outer = (radius + 1) * (radius + 1);
        let span = outer - inner;
        let y0 = bounds.y.max(0) as usize;
        let y1 = (bounds.bottom().max(0) as usize).min(self.ch);
        let x0 = bounds.x.max(0) as usize;
        let x1 = (bounds.right().max(0) as usize).min(self.cw).max(x0);
        let alphas = [alpha as u32, fainter(alpha)];
        for y in y0..y1 {
            let dy = ((2 * y as i32 + 1 - (2 * bounds.y + bounds.h)).abs() - long_y).max(0);
            for (from, to, over) in self.pieces(y, x0, x1) {
                let alpha = alphas[over as usize];
                for x in from..to {
                    let dx = ((2 * x as i32 + 1 - (2 * bounds.x + bounds.w)).abs() - long_x).max(0);
                    let d2 = dx * dx + dy * dy;
                    if d2 >= outer {
                        continue;
                    }
                    let a = if d2 <= inner {
                        alpha
                    } else {
                        alpha * (outer - d2) as u32 / span as u32
                    };
                    self.blend(x, y, colour, a);
                }
            }
        }
    }

    /// Glyphs of 5 by 7 cells of `cell` pixels with a cell between them,
    /// centred on (`cx`, `cy`).
    fn text(&mut self, glyphs: &[Glyph], cx: i32, cy: i32, cell: i32) {
        let width = (6 * glyphs.len() as i32 - 1) * cell;
        let (x0, y0) = (cx - width / 2, cy - 7 * cell / 2);
        for (n, rows) in glyphs.iter().enumerate() {
            for (j, bits) in rows.iter().enumerate() {
                for i in 0..5 {
                    if bits & (0x10 >> i) != 0 {
                        let r = Rect {
                            x: x0 + (6 * n as i32 + i) * cell,
                            y: y0 + j as i32 * cell,
                            w: cell,
                            h: cell,
                        };
                        self.rect(r, DARK, MARK_ALPHA);
                    }
                }
            }
        }
    }
}

/// What is left of an opacity over the picture.
fn fainter(alpha: u8) -> u32 {
    div255(alpha as u32 * OVER_PICTURE as u32)
}

/// `v / 255` rounded, for `v` up to 255 * 255.
fn div255(v: u32) -> u32 {
    let v = v + 128;
    (v + (v >> 8)) >> 8
}
