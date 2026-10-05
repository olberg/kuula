//! Which button each finger holds.

use kuula_core::input::{BTN_DOWN, BTN_LEFT, BTN_MENU, BTN_RIGHT, BTN_UP};

use crate::layout::{Layout, DPAD_DEAD_ZONE};

/// Fingers tracked at once. A touch screen reports ten at most; a finger
/// beyond these is ignored.
pub const MAX_FINGERS: usize = 10;

/// tan(22.5 degrees): the edge between a straight direction and a diagonal
/// one, in the eight 45-degree sectors the D-pad is divided into.
const TAN_22_5: f32 = 0.414_213_57;

/// What a finger went down on, which it keeps until it lifts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Claim {
    /// The D-pad: it steers wherever it moves.
    Dpad,
    /// Anywhere that is not the D-pad or a pill: it holds each round
    /// button that it is inside the hit circle of.
    Buttons,
    /// A pill: Menu, a shoulder button, Start or Select.
    Key,
}

#[derive(Debug, Clone, Copy)]
struct Finger {
    id: u64,
    claim: Claim,
    /// The `BTN_*` bits this finger holds now.
    held: u16,
}

/// The fingers and what they hold down. A fixed number of fingers is
/// tracked, so nothing here allocates.
#[derive(Debug, Clone, Default)]
pub struct Pad {
    fingers: [Option<Finger>; MAX_FINGERS],
}

impl Pad {
    pub fn new() -> Pad {
        Pad::default()
    }

    /// A finger touched the screen at (`x`, `y`) in window pixels.
    ///
    /// What it is on is decided in the order the controls are drawn over
    /// each other, so that in a window too small to keep them apart a
    /// finger gets what it sees. On Menu's pill it holds Menu. On the
    /// picture of a round button it is a button finger, and on the
    /// picture of the D-pad it claims the D-pad. Then the wider areas: on
    /// the hit area of a pill that is shown it holds that pill, a shoulder
    /// button, Start or Select before Menu, whose area is the widest;
    /// within [`Layout::dpad_claim_radius`] of the D-pad's centre it
    /// claims the D-pad; and anywhere else it is a button finger. A finger
    /// that is already tracked under `id` is replaced.
    pub fn down(&mut self, layout: &Layout, id: u64, x: f32, y: f32) {
        self.up(id);
        let pad = layout.dpad();
        let (claim, held) = if layout.menu().contains(x, y) {
            (Claim::Key, BTN_MENU)
        } else if layout.faces().any(|(_, d)| d.within(x, y, d.r as f32)) {
            (Claim::Buttons, buttons_at(layout, x, y))
        } else if pad.within(x, y, pad.r as f32) {
            (Claim::Dpad, direction(layout, x, y))
        } else if let Some(bit) = key_at(layout, x, y) {
            (Claim::Key, bit)
        } else if pad.within(x, y, layout.dpad_claim_radius()) {
            (Claim::Dpad, direction(layout, x, y))
        } else {
            (Claim::Buttons, buttons_at(layout, x, y))
        };
        let Some(slot) = self.fingers.iter_mut().find(|f| f.is_none()) else {
            return;
        };
        *slot = Some(Finger { id, claim, held });
    }

    /// A tracked finger moved to (`x`, `y`). A finger that never went down
    /// is ignored.
    pub fn moved(&mut self, layout: &Layout, id: u64, x: f32, y: f32) {
        let Some(finger) = self.fingers.iter_mut().flatten().find(|f| f.id == id) else {
            return;
        };
        match finger.claim {
            Claim::Dpad => finger.held = direction(layout, x, y),
            Claim::Buttons => finger.held = buttons_at(layout, x, y),
            // A pill is held until the finger lifts, wherever it goes.
            Claim::Key => {}
        }
    }

    /// A finger lifted; what only it held is released.
    pub fn up(&mut self, id: u64) {
        for slot in self.fingers.iter_mut() {
            if slot.is_some_and(|f| f.id == id) {
                *slot = None;
            }
        }
    }

    /// Every finger is gone (the app lost focus, the surface went away).
    pub fn cancel(&mut self) {
        self.fingers = [None; MAX_FINGERS];
    }

    /// The buttons held by all the fingers together, as `BTN_*` bits.
    pub fn buttons(&self) -> u16 {
        self.fingers
            .iter()
            .flatten()
            .fold(0, |bits, f| bits | f.held)
    }
}

/// The direction bits for a finger at (`x`, `y`) steering the D-pad: the
/// 45-degree sector the finger's offset from the centre is in, and nothing
/// within the dead zone.
fn direction(layout: &Layout, x: f32, y: f32) -> u16 {
    let pad = layout.dpad();
    let (dx, dy) = (x - pad.cx as f32, y - pad.cy as f32);
    let dead = pad.r as f32 * DPAD_DEAD_ZONE;
    if dx * dx + dy * dy < dead * dead {
        return 0;
    }
    let (ax, ay) = (dx.abs(), dy.abs());
    let horizontal = if dx < 0.0 { BTN_LEFT } else { BTN_RIGHT };
    let vertical = if dy < 0.0 { BTN_UP } else { BTN_DOWN };
    if ay <= ax * TAN_22_5 {
        horizontal
    } else if ax <= ay * TAN_22_5 {
        vertical
    } else {
        horizontal | vertical
    }
}

/// The round buttons whose hit circles contain (`x`, `y`).
fn buttons_at(layout: &Layout, x: f32, y: f32) -> u16 {
    let reach = layout.button_hit_radius();
    layout
        .faces()
        .filter(|(_, d)| d.within(x, y, reach))
        .fold(0, |held, (bit, _)| held | bit)
}

/// The pill whose hit area contains (`x`, `y`).
fn key_at(layout: &Layout, x: f32, y: f32) -> Option<u16> {
    let grow = layout.key_grow();
    layout
        .keys()
        .find(|(_, r)| r.grown(grow).contains(x, y))
        .map(|(bit, _)| bit)
        .or_else(|| layout.menu_hit().contains(x, y).then_some(BTN_MENU))
}
