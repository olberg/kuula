//! What is held down by keys, a joystick's hat and its triggers, and
//! whether the on-screen controls are showing. Touch is the pad's own
//! (`kuula_touch::Pad`); this is the rest, which is what a controller or a
//! handheld's own buttons give.
//!
//! The controls are drawn while touch is the way the person plays: a
//! controller's key, a leaning hat or a pulled trigger hides them, the next
//! touch shows them again. Back is not a controller's key for this: a phone with nothing
//! but its touch screen sends it for the system's back gesture, and
//! opening the menu that way must not take the controls away.

use crate::keymap;

/// Most keys held at once that are remembered; more are ignored.
const MAX_KEYS: usize = 16;

/// Most controllers whose hat and triggers are remembered at once; one
/// more is ignored until another has let everything go.
const MAX_PADS: usize = 4;

/// What one controller's axes hold: its hat and its triggers.
#[derive(Debug, Clone, Copy)]
struct PadAxes {
    device: i32,
    hat: u16,
    triggers: u16,
}

#[derive(Debug, Clone)]
pub struct Controls {
    keys: Vec<u32>,
    /// The controllers whose axes hold something. An event says how one
    /// device's axes are, and nothing about another's.
    pads: Vec<PadAxes>,
    controls_visible: bool,
}

impl Default for Controls {
    fn default() -> Self {
        Controls {
            keys: Vec::new(),
            pads: Vec::new(),
            controls_visible: true,
        }
    }
}

impl Controls {
    /// A key went down. True when it is one of ours, so the event is
    /// handled and Back does not leave the app.
    pub fn key_down(&mut self, code: u32) -> bool {
        if keymap::button_for_key(code).is_none() {
            return false;
        }
        if !self.keys.contains(&code) && self.keys.len() < MAX_KEYS {
            self.keys.push(code);
        }
        if code != keymap::KEYCODE_BACK {
            self.controls_visible = false;
        }
        true
    }

    /// A key came up; true when it is one of ours.
    pub fn key_up(&mut self, code: u32) -> bool {
        self.keys.retain(|&k| k != code);
        keymap::button_for_key(code).is_some()
    }

    /// The axes of the controller `device` as they are now: its hat (each
    /// axis from -1 to 1) and its two triggers (each from 0 to 1).
    pub fn axes(&mut self, device: i32, hat: (f32, f32), triggers: (f32, f32)) {
        let hat = keymap::hat_buttons(hat.0, hat.1);
        let triggers = keymap::trigger_buttons(triggers.0, triggers.1);
        self.pads.retain(|p| p.device != device);
        if hat | triggers == 0 {
            return;
        }
        if self.pads.len() < MAX_PADS {
            self.pads.push(PadAxes {
                device,
                hat,
                triggers,
            });
        }
        self.controls_visible = false;
    }

    /// A finger touched the screen.
    pub fn touched(&mut self) {
        self.controls_visible = true;
    }

    /// Everything let go: the window lost focus or the app paused, so no
    /// key-up will come for what was held.
    pub fn release_all(&mut self) {
        self.keys.clear();
        self.pads.clear();
    }

    /// The buttons keys, the hats and the triggers hold.
    pub fn buttons(&self) -> u16 {
        let axes = self
            .pads
            .iter()
            .fold(0, |bits, p| bits | p.hat | p.triggers);
        self.keys
            .iter()
            .filter_map(|&k| keymap::button_for_key(k))
            .fold(axes, |bits, b| bits | b)
    }

    /// Whether the on-screen controls are drawn.
    pub fn controls_visible(&self) -> bool {
        self.controls_visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuula_core::input::{BTN_A, BTN_L2, BTN_LEFT, BTN_MENU, BTN_R2};

    #[test]
    fn two_keys_for_one_button_hold_it_until_both_are_up() {
        let mut c = Controls::default();
        assert!(c.key_down(keymap::KEYCODE_BUTTON_A));
        assert!(c.key_down(keymap::KEYCODE_DPAD_CENTER));
        assert_eq!(c.buttons(), BTN_A);
        c.key_up(keymap::KEYCODE_BUTTON_A);
        assert_eq!(c.buttons(), BTN_A);
        c.key_up(keymap::KEYCODE_DPAD_CENTER);
        assert_eq!(c.buttons(), 0);
    }

    #[test]
    fn a_trigger_is_held_by_its_key_by_its_axis_or_by_both() {
        let mut c = Controls::default();
        c.axes(7, (0.0, 0.0), (1.0, 0.0));
        assert_eq!(c.buttons(), BTN_L2);
        assert!(!c.controls_visible());
        assert!(c.key_down(keymap::KEYCODE_BUTTON_L2));
        assert!(c.key_down(keymap::KEYCODE_BUTTON_R2));
        assert_eq!(c.buttons(), BTN_L2 | BTN_R2);
        c.axes(7, (0.0, 0.0), (0.0, 0.0));
        assert_eq!(c.buttons(), BTN_L2 | BTN_R2);
        c.key_up(keymap::KEYCODE_BUTTON_L2);
        assert_eq!(c.buttons(), BTN_R2);
    }

    #[test]
    fn one_controllers_axes_say_nothing_about_anothers() {
        let mut c = Controls::default();
        // The first holds L2 and leans its hat left, and then is still.
        c.axes(1, (-1.0, 0.0), (1.0, 0.0));
        // The second moves: its own axes are at rest, then R2.
        c.axes(2, (0.0, 0.0), (0.0, 0.0));
        assert_eq!(c.buttons(), BTN_L2 | BTN_LEFT);
        c.axes(2, (0.0, 0.0), (0.0, 1.0));
        assert_eq!(c.buttons(), BTN_L2 | BTN_LEFT | BTN_R2);
        c.axes(1, (0.0, 0.0), (0.0, 0.0));
        assert_eq!(c.buttons(), BTN_R2);
        // No more than four are remembered, and one that lets go makes
        // room.
        for device in 3..=6 {
            c.axes(device, (0.0, 1.0), (0.0, 0.0));
        }
        c.axes(2, (0.0, 0.0), (0.0, 0.0));
        c.axes(9, (0.0, 0.0), (1.0, 0.0));
        assert_eq!(c.buttons(), kuula_core::input::BTN_DOWN | BTN_L2);
    }

    #[test]
    fn a_key_hides_the_controls_and_a_touch_brings_them_back() {
        let mut c = Controls::default();
        assert!(c.controls_visible());
        // Not ours: the controls stay, and the event is not handled.
        assert!(!c.key_down(24));
        assert!(c.controls_visible());
        // Back is Menu, and is what a phone's own back gesture sends: the
        // controls stay.
        assert!(c.key_down(keymap::KEYCODE_BACK));
        assert!(c.controls_visible());
        assert_eq!(c.buttons(), BTN_MENU);
        // A controller's button hides them, and a touch brings them back.
        assert!(c.key_down(keymap::KEYCODE_BUTTON_A));
        assert!(!c.controls_visible());
        c.key_up(keymap::KEYCODE_BUTTON_A);
        c.touched();
        assert!(c.controls_visible());
        c.axes(1, (-1.0, 0.0), (0.0, 0.0));
        assert!(!c.controls_visible());
        assert_eq!(c.buttons(), BTN_MENU | BTN_LEFT);
    }

    #[test]
    fn letting_everything_go_clears_keys_and_hat() {
        let mut c = Controls::default();
        c.key_down(keymap::KEYCODE_DPAD_UP);
        c.axes(1, (1.0, 1.0), (1.0, 1.0));
        c.release_all();
        assert_eq!(c.buttons(), 0);
    }
}
