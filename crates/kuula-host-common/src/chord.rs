//! Start and Select pressed together, by a person. The console makes Menu
//! of the two held at once ([`kuula_core::input::Chord`]); two thumbs do
//! not land in the same frame, and without this the one that landed first
//! would be the cart's Start or Select for a frame or two before the menu
//! opened over it.

use kuula_core::input::{Chord, BTN_SELECT, BTN_START};

const BOTH: u16 = BTN_START | BTN_SELECT;

/// How many frames Start and Select are held back: a tenth of a second.
pub const GRACE_FRAMES: usize = 6;

/// Turns the buttons a person holds into what the console is given.
///
/// Start and Select reach the console [`GRACE_FRAMES`] late, every other
/// button at once. When the two are down together within that time they
/// are Menu from that frame and the cart is given neither, not even the
/// one that was first; a press that is over sooner than the delay still
/// arrives, as long as it was. Longer apart than that, the first is the
/// cart's until the second joins it.
#[derive(Debug, Default, Clone, Copy)]
pub struct MenuChord {
    chord: Chord,
    /// Start and Select as they were in each of the last frames.
    line: [u16; GRACE_FRAMES],
    at: usize,
}

impl MenuChord {
    pub fn apply(&mut self, buttons: u16) -> u16 {
        let ruled = self.chord.apply(buttons);
        if buttons & BOTH == BOTH {
            // Menu already; what was on its way to the cart is dropped.
            self.line = [0; GRACE_FRAMES];
            return ruled;
        }
        let late = std::mem::replace(&mut self.line[self.at], ruled & BOTH);
        self.at = (self.at + 1) % GRACE_FRAMES;
        (ruled & !BOTH) | late
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuula_core::input::{BTN_A, BTN_LEFT, BTN_MENU};

    /// What comes out for each frame of `held`.
    fn run(chord: &mut MenuChord, held: &[u16]) -> Vec<u16> {
        held.iter().map(|&b| chord.apply(b)).collect()
    }

    #[test]
    fn start_alone_arrives_late_and_as_long_as_it_was_held() {
        let mut c = MenuChord::default();
        let mut held = vec![BTN_START | BTN_A; 2];
        held.extend([BTN_LEFT; GRACE_FRAMES + 1]);
        let out = run(&mut c, &held);
        // A and the D-pad at once; Start for two frames, six frames on.
        assert_eq!(out[..2], [BTN_A, BTN_A]);
        assert_eq!(out[2..GRACE_FRAMES], vec![BTN_LEFT; GRACE_FRAMES - 2]);
        assert_eq!(
            out[GRACE_FRAMES..GRACE_FRAMES + 2],
            [BTN_START | BTN_LEFT; 2]
        );
        assert_eq!(out[GRACE_FRAMES + 2], BTN_LEFT);
    }

    #[test]
    fn the_two_a_few_frames_apart_are_menu_and_the_cart_gets_neither() {
        let mut c = MenuChord::default();
        // Start, Select two frames later, Select let go first, then Start.
        let mut held = vec![BTN_START; 2];
        held.extend([BOTH; 4]);
        held.extend([BTN_START; 3]);
        held.extend([0; GRACE_FRAMES + 2]);
        let out = run(&mut c, &held);
        assert_eq!(out[..2], [0, 0]);
        assert_eq!(out[2..6], [BTN_MENU; 4]);
        assert!(out[6..].iter().all(|&b| b == 0), "{out:?}");
    }

    #[test]
    fn longer_apart_than_the_delay_the_first_is_the_carts_until_the_second_joins() {
        let mut c = MenuChord::default();
        let mut held = vec![BTN_SELECT; GRACE_FRAMES + 2];
        held.extend([BOTH; 2]);
        held.extend([0; GRACE_FRAMES + 1]);
        let out = run(&mut c, &held);
        assert_eq!(out[..GRACE_FRAMES], [0; GRACE_FRAMES]);
        assert_eq!(out[GRACE_FRAMES..GRACE_FRAMES + 2], [BTN_SELECT; 2]);
        assert_eq!(out[GRACE_FRAMES + 2..GRACE_FRAMES + 4], [BTN_MENU; 2]);
        assert!(out[GRACE_FRAMES + 4..].iter().all(|&b| b == 0), "{out:?}");
    }

    #[test]
    fn a_menu_key_of_its_own_is_not_held_back() {
        let mut c = MenuChord::default();
        assert_eq!(c.apply(BTN_MENU | BTN_A), BTN_MENU | BTN_A);
    }
}
