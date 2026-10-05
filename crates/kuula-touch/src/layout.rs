//! Where the picture and the controls go for one window.

use kuula_core::input::{
    BASE_BUTTONS, BTN_A, BTN_B, BTN_L1, BTN_L2, BTN_R1, BTN_R2, BTN_SELECT, BTN_START, BTN_X, BTN_Y,
};

/// A rectangle in pixels: `x` and `y` are the top left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    /// The column after the last one the rectangle covers.
    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    /// The row after the last one the rectangle covers.
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// Whether the point is inside; the left and top edges count, the right
    /// and bottom edges do not.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32
            && x < self.right() as f32
            && y >= self.y as f32
            && y < self.bottom() as f32
    }

    /// The rectangle grown by `by` pixels on every side.
    pub fn grown(&self, by: i32) -> Rect {
        Rect {
            x: self.x - by,
            y: self.y - by,
            w: self.w + 2 * by,
            h: self.h + 2 * by,
        }
    }
}

/// Pixels of the window that the system covers: a display cutout, a bar.
/// Controls stay out of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Insets {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// A round control: its centre and its drawn radius, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disc {
    pub cx: i32,
    pub cy: i32,
    pub r: i32,
}

impl Disc {
    /// The square the disc fits in.
    pub fn bounds(&self) -> Rect {
        Rect {
            x: self.cx - self.r,
            y: self.cy - self.r,
            w: 2 * self.r,
            h: 2 * self.r,
        }
    }

    /// Whether the point is within `radius` of the centre.
    pub fn within(&self, x: f32, y: f32, radius: f32) -> bool {
        let (dx, dy) = (x - self.cx as f32, y - self.cy as f32);
        dx * dx + dy * dy <= radius * radius
    }
}

/// Width of the D-pad's disc, in dp.
pub const DPAD_DP: f32 = 144.0;
/// Width of the A, B, X and Y discs, in dp.
pub const BUTTON_DP: f32 = 64.0;
/// Width of the L1, R1, L2 and R2 pills, in dp.
pub const SHOULDER_W_DP: f32 = 64.0;
/// Height of the L1, R1, L2 and R2 pills, in dp.
pub const SHOULDER_H_DP: f32 = 36.0;
/// Width of the Start and Select pills, in dp. They are as high as Menu.
pub const START_W_DP: f32 = 80.0;
/// The gap between two pills, and between a pill and what it is beside,
/// in dp.
pub const GAP_DP: f32 = 8.0;
/// How far the hit rectangle of a pill other than Menu is grown on every
/// side, in dp: half the gap, so that two neighbours' areas meet.
pub const KEY_GROW_DP: f32 = 4.0;
/// Width of the Menu pill, in dp.
pub const MENU_W_DP: f32 = 56.0;
/// Height of the Menu pill, in dp.
pub const MENU_H_DP: f32 = 28.0;
/// Distance the controls keep from the edges of the window, in dp.
pub const MARGIN_DP: f32 = 16.0;
/// How far the Menu's hit rectangle is grown on every side, in dp.
pub const MENU_GROW_DP: f32 = 12.0;
/// A finger going down within this many times the D-pad's drawn radius of
/// its centre claims the D-pad.
pub const DPAD_CLAIM: f32 = 1.5;
/// Inside this fraction of the D-pad's radius it holds nothing.
pub const DPAD_DEAD_ZONE: f32 = 0.12;
/// A button's hit circle, in times its drawn radius.
pub const BUTTON_HIT: f32 = 1.4;

/// How far below the middle of the window the D-pad and the buttons are
/// centred, in dp.
const BELOW_MIDDLE_DP: f32 = 24.0;
/// A and B are centred this far from the middle of their pair, A up and to
/// the right, B down and to the left, in dp.
const PAIR_DX_DP: f32 = 30.0;
const PAIR_DY_DP: f32 = 24.0;

/// Where everything is for one window, in pixels. Landscape or portrait,
/// whichever the window's shape is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    window: (i32, i32),
    portrait: bool,
    insets: Insets,
    density: f32,
    source: (u32, u32),
    picture: Rect,
    scale: i32,
    scale2: i32,
    dpad: Disc,
    a: Disc,
    b: Disc,
    x: Disc,
    y: Disc,
    menu: Rect,
    /// L1, R1, L2, R2, Start and Select, each with its `BTN_*` bit.
    keys: [(u16, Rect); 6],
    shown: u16,
}

impl Layout {
    /// The layout for a window of `window` pixels (width, height) whose
    /// system-covered edges are `insets`. `density` is pixels per dp and
    /// `picture` is the console's frame size. The window's shape picks the
    /// orientation: landscape when it is at least as wide as it is high.
    ///
    /// Landscape: the picture is centred at the largest whole scale (1 at
    /// least) at which it fits the window's width and its height less the
    /// top and bottom insets. The D-pad is centred in the left bar and A and
    /// B in the right bar, a little below the middle, each kept
    /// [`MARGIN_DP`] from the window's edges and inside the left and right
    /// insets. A and B are further right than the middle of their bar
    /// where X and Y would otherwise lie over the picture. Where a bar is
    /// narrower than its control, the control keeps its place by the edge
    /// and lies over the picture's edge. Menu is in the top right corner.
    ///
    /// Portrait: the picture is at the top, just below the top inset,
    /// centred between the left and right insets at the largest whole scale
    /// (1 at least) whose width fits between them. The controls are in the
    /// space below it: the D-pad at the lower left, A and B at the lower
    /// right, each vertically in the middle of that space, and Menu centred
    /// between the insets near the bottom, all [`MARGIN_DP`] from the edges
    /// and clear of the bottom inset. Where the space is shorter than a
    /// control, the control keeps its place at the bottom and lies over the
    /// picture.
    ///
    /// Those are the controls of a cart with two buttons, and the ones
    /// shown until [`Layout::showing`] asks for more. The rest have their
    /// places around them, and none of the above moves when they appear.
    /// X and Y complete the square A and B are a side of: X is over A and
    /// Y over B, so the four lie as on a handheld, X at the top and Y on
    /// the left. L2 and L1 are a row from the left edge and R1 and R2 a
    /// row up to the right edge, just above whichever of the D-pad and X
    /// is higher. In landscape, where Menu has the corner, the rows are
    /// under Menu if they then come no lower than the top of X, and
    /// otherwise level with it, R1 and R2 ending on Menu's left. Select and
    /// Start are on Menu's left and right in portrait; in landscape Select
    /// is under the D-pad and Start under the buttons.
    pub fn new(window: (i32, i32), insets: Insets, density: f32, picture: (u32, u32)) -> Layout {
        let density = if density.is_finite() && density > 0.0 {
            density
        } else {
            1.0
        };
        let dp = |v: f32| (v * density).round() as i32;
        let (w, h) = (window.0.max(0), window.1.max(0));
        let (pw, ph) = (picture.0.max(1) as i32, picture.1.max(1) as i32);
        let portrait = h > w;
        let free_h = (h - insets.top - insets.bottom).max(0);
        let free_w = (w - insets.left - insets.right).max(0);
        // Twice the scale, so that one and a half is a whole number. A
        // whole scale is taken wherever it is 2 or more. Where only 1 fits,
        // as for a 640-pixel frame across a 1080-pixel phone held upright,
        // the picture would be small, and one and a half is taken if it
        // fits: three pixels for every two of the frame.
        let halves = |fit: i32, of: i32| -> i32 {
            let whole = (fit / of).max(1);
            let even = pw % 2 == 0 && ph % 2 == 0;
            if whole == 1 && even && of * 3 / 2 <= fit {
                3
            } else {
                whole * 2
            }
        };
        let (scale2, picture_rect) = if portrait {
            let scale2 = halves(free_w, pw);
            let rect = Rect {
                x: insets.left + (free_w - pw * scale2 / 2) / 2,
                y: insets.top,
                w: pw * scale2 / 2,
                h: ph * scale2 / 2,
            };
            (scale2, rect)
        } else {
            let scale2 = halves(w, pw).min(halves(free_h, ph));
            let rect = Rect {
                x: (w - pw * scale2 / 2) / 2,
                y: insets.top + (free_h - ph * scale2 / 2) / 2,
                w: pw * scale2 / 2,
                h: ph * scale2 / 2,
            };
            (scale2, rect)
        };
        let scale = scale2 / 2;

        let margin = dp(MARGIN_DP);
        let left = insets.left + margin;
        let right = w - insets.right - margin;
        let top = insets.top + margin;
        let bottom = h - insets.bottom - margin;

        let r = dp(DPAD_DP / 2.0);
        let rb = dp(BUTTON_DP / 2.0);
        let (dx, dy) = (dp(PAIR_DX_DP), dp(PAIR_DY_DP));
        let half = dx + rb;
        let reach = dy + rb;
        let (mw, mh) = (dp(MENU_W_DP), dp(MENU_H_DP));
        let (dpad, pair_x, pair_y, menu_x, menu_y);
        if portrait {
            // The space under the picture: a control is in the middle of
            // it, or at the bottom when the space is shorter than it.
            let space_top = picture_rect.bottom();
            let space = h - insets.bottom - space_top;
            let place = |half_height: i32| {
                let middle = space_top + space / 2;
                if space >= 2 * half_height {
                    middle.min(bottom - half_height)
                } else {
                    bottom - half_height
                }
            };
            dpad = Disc {
                cx: left + r,
                cy: place(r),
                r,
            };
            pair_x = right - half;
            pair_y = place(reach);
            menu_x = insets.left + (free_w - mw) / 2;
            menu_y = bottom - mh;
        } else {
            let middle = insets.top + free_h / 2;
            let below = dp(BELOW_MIDDLE_DP);
            // The D-pad: centred in the left bar, no nearer the edge than
            // the margin, which is where it stays when the bar is too
            // narrow.
            let bar_middle = (insets.left + picture_rect.x) / 2;
            dpad = Disc {
                cx: bar_middle.max(left + r),
                cy: (middle + below).min(bottom - r).max(top + r),
                r,
            };
            // A and B: a pair centred in the right bar, or as far to the
            // right of that as keeps Y, the leftmost of the four, a gap
            // clear of the picture. They are there whether X and Y are
            // shown or not, so that they never move.
            let bar_middle = (picture_rect.right() + w - insets.right) / 2;
            let clear_of_picture = picture_rect.right() + dp(GAP_DP) + dx + 2 * dy + rb;
            pair_x = bar_middle
                .max(clear_of_picture)
                .min(right - half)
                .max(left + half);
            pair_y = (middle + below).min(bottom - reach).max(top + reach);
            menu_x = right - mw;
            menu_y = top;
        }
        let a = Disc {
            cx: pair_x + dx,
            cy: pair_y - dy,
            r: rb,
        };
        let b = Disc {
            cx: pair_x - dx,
            cy: pair_y + dy,
            r: rb,
        };
        let menu = Rect {
            x: menu_x,
            y: menu_y,
            w: mw,
            h: mh,
        };

        // From A to X and from B to Y is the step from B to A turned a
        // quarter of the way round.
        let x = Disc {
            cx: a.cx - 2 * dy,
            cy: a.cy - 2 * dx,
            r: rb,
        };
        let y = Disc {
            cx: b.cx - 2 * dy,
            cy: b.cy - 2 * dx,
            r: rb,
        };
        let (sw, sh) = (dp(SHOULDER_W_DP), dp(SHOULDER_H_DP));
        let start_w = dp(START_W_DP);
        let gap = dp(GAP_DP);
        let cluster_top = (dpad.cy - r).min(x.cy - rb);
        let above = cluster_top - gap - sh;
        let under_menu = top + mh + gap;
        let (row_y, row_right) = if portrait || above >= under_menu {
            (above.max(top), right)
        } else if under_menu + sh <= cluster_top {
            (under_menu, right)
        } else {
            (above.max(top), menu_x - gap)
        };
        let shoulder = |x: i32| Rect {
            x,
            y: row_y,
            w: sw,
            h: sh,
        };
        let (select_x, start_x, start_y);
        if portrait {
            select_x = menu_x - gap - start_w;
            start_x = menu_x + mw + gap;
            start_y = menu_y;
        } else {
            select_x = (dpad.cx - start_w / 2).max(left);
            start_x = (pair_x - start_w / 2).min(right - start_w);
            start_y = ((dpad.cy + r).max(b.cy + rb) + 2 * gap).min(bottom - mh);
        }
        let small = |x: i32| Rect {
            x,
            y: start_y,
            w: start_w,
            h: mh,
        };
        let keys = [
            (BTN_L2, shoulder(left)),
            (BTN_L1, shoulder(left + sw + gap)),
            (BTN_R1, shoulder(row_right - 2 * sw - gap)),
            (BTN_R2, shoulder(row_right - sw)),
            (BTN_SELECT, small(select_x)),
            (BTN_START, small(start_x)),
        ];

        Layout {
            window: (w, h),
            portrait,
            insets,
            density,
            source: (pw as u32, ph as u32),
            picture: picture_rect,
            scale,
            scale2,
            dpad,
            a,
            b,
            x,
            y,
            menu,
            keys,
            shown: BASE_BUTTONS,
        }
    }

    /// The same layout showing the controls of `buttons`, as `BTN_*`
    /// bits. The D-pad, A, B and Menu are always shown.
    pub fn showing(mut self, buttons: u16) -> Layout {
        self.shown = buttons | BASE_BUTTONS;
        self
    }

    /// The `BTN_*` bits of the controls that are shown, Menu apart.
    pub fn shown(&self) -> u16 {
        self.shown
    }

    /// Where the console's frame goes: centred, at a whole scale.
    pub fn picture(&self) -> Rect {
        self.picture
    }

    /// Twice the scale the picture is drawn at: an even number for a whole
    /// scale, 3 for one and a half.
    pub fn scale_halves(&self) -> i32 {
        self.scale2
    }

    /// The whole part of the scale the picture is drawn at.
    pub fn scale(&self) -> i32 {
        self.scale
    }

    /// The console's frame size the layout was made for.
    pub fn source(&self) -> (u32, u32) {
        self.source
    }

    /// Whether the window is higher than it is wide, and the controls are
    /// below the picture.
    pub fn is_portrait(&self) -> bool {
        self.portrait
    }

    /// The window's size in pixels.
    pub fn window(&self) -> (i32, i32) {
        self.window
    }

    /// The insets the layout keeps the controls inside.
    pub fn insets(&self) -> Insets {
        self.insets
    }

    /// Pixels per dp.
    pub fn density(&self) -> f32 {
        self.density
    }

    /// The D-pad's disc.
    pub fn dpad(&self) -> Disc {
        self.dpad
    }

    /// Button A's disc.
    pub fn button_a(&self) -> Disc {
        self.a
    }

    /// Button B's disc.
    pub fn button_b(&self) -> Disc {
        self.b
    }

    /// Button X's disc, shown or not.
    pub fn button_x(&self) -> Disc {
        self.x
    }

    /// Button Y's disc, shown or not.
    pub fn button_y(&self) -> Disc {
        self.y
    }

    /// The round buttons that are shown, each with its `BTN_*` bit: A and
    /// B, and X and Y for a cart that has them.
    pub fn faces(&self) -> impl Iterator<Item = (u16, Disc)> + '_ {
        [
            (BTN_A, self.a),
            (BTN_B, self.b),
            (BTN_X, self.x),
            (BTN_Y, self.y),
        ]
        .into_iter()
        .filter(|(bit, _)| self.shown & bit != 0)
    }

    /// The Menu pill.
    pub fn menu(&self) -> Rect {
        self.menu
    }

    /// The pill of L1, R1, L2, R2, Start or Select by its `BTN_*` bit,
    /// shown or not; `None` for any other bit.
    pub fn key(&self, bit: u16) -> Option<Rect> {
        self.keys.iter().find(|(b, _)| *b == bit).map(|(_, r)| *r)
    }

    /// The pills that are shown besides Menu, each with its `BTN_*` bit.
    pub fn keys(&self) -> impl Iterator<Item = (u16, Rect)> + '_ {
        self.keys
            .into_iter()
            .filter(|(bit, _)| self.shown & bit != 0)
    }

    /// How far the hit rectangle of each of [`Layout::keys`] is grown on
    /// every side, in pixels.
    pub fn key_grow(&self) -> i32 {
        (KEY_GROW_DP * self.density).round() as i32
    }

    /// The distance from the D-pad's centre within which a finger going
    /// down claims it.
    pub fn dpad_claim_radius(&self) -> f32 {
        self.dpad.r as f32 * DPAD_CLAIM
    }

    /// The distance from a button's centre within which a finger holds it.
    pub fn button_hit_radius(&self) -> f32 {
        self.a.r as f32 * BUTTON_HIT
    }

    /// The area in which a finger going down holds Menu.
    pub fn menu_hit(&self) -> Rect {
        self.menu
            .grown((MENU_GROW_DP * self.density).round() as i32)
    }
}
