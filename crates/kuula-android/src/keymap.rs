//! Android key codes and joystick axes to the logical controller. The
//! codes are `android.view.KeyEvent`'s own integers, so nothing here names
//! an Android API and the tables are tested on the desktop.

use kuula_core::input::{
    BTN_A, BTN_B, BTN_DOWN, BTN_L1, BTN_L2, BTN_LEFT, BTN_MENU, BTN_R1, BTN_R2, BTN_RIGHT,
    BTN_SELECT, BTN_START, BTN_UP, BTN_X, BTN_Y,
};
use kuula_host_common::controls::{trigger_pulled, Key};

pub const KEYCODE_BACK: u32 = 4;
pub const KEYCODE_DPAD_UP: u32 = 19;
pub const KEYCODE_DPAD_DOWN: u32 = 20;
pub const KEYCODE_DPAD_LEFT: u32 = 21;
pub const KEYCODE_DPAD_RIGHT: u32 = 22;
pub const KEYCODE_DPAD_CENTER: u32 = 23;
pub const KEYCODE_A: u32 = 29;
pub const KEYCODE_C: u32 = 31;
pub const KEYCODE_Q: u32 = 45;
pub const KEYCODE_S: u32 = 47;
pub const KEYCODE_V: u32 = 50;
pub const KEYCODE_W: u32 = 51;
pub const KEYCODE_X: u32 = 52;
pub const KEYCODE_Z: u32 = 54;
pub const KEYCODE_SHIFT_RIGHT: u32 = 60;
pub const KEYCODE_ENTER: u32 = 66;
pub const KEYCODE_BUTTON_A: u32 = 96;
pub const KEYCODE_BUTTON_B: u32 = 97;
pub const KEYCODE_BUTTON_X: u32 = 99;
pub const KEYCODE_BUTTON_Y: u32 = 100;
pub const KEYCODE_BUTTON_L1: u32 = 102;
pub const KEYCODE_BUTTON_R1: u32 = 103;
pub const KEYCODE_BUTTON_L2: u32 = 104;
pub const KEYCODE_BUTTON_R2: u32 = 105;
pub const KEYCODE_BUTTON_START: u32 = 108;
pub const KEYCODE_BUTTON_SELECT: u32 = 109;
pub const KEYCODE_BUTTON_MODE: u32 = 110;
pub const KEYCODE_ESCAPE: u32 = 111;

/// How far a hat axis must lean, out of 1, to press its direction.
pub const HAT_THRESHOLD: f32 = 0.5;

/// The button a key stands for. The D-pad keys are the D-pad and a
/// controller's buttons are the buttons of the same names; Back and Mode
/// are Menu. A hardware keyboard has the desktop's keys, by the table
/// every host shares.
pub fn button_for_key(code: u32) -> Option<u16> {
    match code {
        KEYCODE_DPAD_UP => Some(BTN_UP),
        KEYCODE_DPAD_DOWN => Some(BTN_DOWN),
        KEYCODE_DPAD_LEFT => Some(BTN_LEFT),
        KEYCODE_DPAD_RIGHT => Some(BTN_RIGHT),
        KEYCODE_BUTTON_A | KEYCODE_DPAD_CENTER => Some(BTN_A),
        KEYCODE_BUTTON_B => Some(BTN_B),
        KEYCODE_BUTTON_X => Some(BTN_X),
        KEYCODE_BUTTON_Y => Some(BTN_Y),
        KEYCODE_BUTTON_L1 => Some(BTN_L1),
        KEYCODE_BUTTON_R1 => Some(BTN_R1),
        KEYCODE_BUTTON_L2 => Some(BTN_L2),
        KEYCODE_BUTTON_R2 => Some(BTN_R2),
        KEYCODE_BUTTON_START => Some(BTN_START),
        KEYCODE_BUTTON_SELECT => Some(BTN_SELECT),
        KEYCODE_BACK | KEYCODE_BUTTON_MODE => Some(BTN_MENU),
        _ => keyboard_key(code).map(Key::button),
    }
}

/// The key of a keyboard an Android key code is, of those that are
/// buttons. A keyboard's arrows are the D-pad's own codes, above.
fn keyboard_key(code: u32) -> Option<Key> {
    Some(match code {
        KEYCODE_Z => Key::Z,
        KEYCODE_X => Key::X,
        KEYCODE_C => Key::C,
        KEYCODE_V => Key::V,
        KEYCODE_A => Key::A,
        KEYCODE_S => Key::S,
        KEYCODE_Q => Key::Q,
        KEYCODE_W => Key::W,
        KEYCODE_ENTER => Key::Enter,
        KEYCODE_SHIFT_RIGHT => Key::RightShift,
        KEYCODE_ESCAPE => Key::Escape,
        _ => return None,
    })
}

/// The D-pad directions a joystick's hat axes (each from -1 to 1, down and
/// right positive) hold.
pub fn hat_buttons(x: f32, y: f32) -> u16 {
    let mut bits = 0;
    if x <= -HAT_THRESHOLD {
        bits |= BTN_LEFT;
    } else if x >= HAT_THRESHOLD {
        bits |= BTN_RIGHT;
    }
    if y <= -HAT_THRESHOLD {
        bits |= BTN_UP;
    } else if y >= HAT_THRESHOLD {
        bits |= BTN_DOWN;
    }
    bits
}

/// L2 and R2 as a controller's trigger axes (each from 0 to 1) hold them.
/// A controller reports its triggers as keys, as axes or as both.
pub fn trigger_buttons(left: f32, right: f32) -> u16 {
    let mut bits = 0;
    if trigger_pulled(left) {
        bits |= BTN_L2;
    }
    if trigger_pulled(right) {
        bits |= BTN_R2;
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_of_a_controller_and_of_a_handheld() {
        let cases = [
            (KEYCODE_DPAD_UP, BTN_UP),
            (KEYCODE_DPAD_RIGHT, BTN_RIGHT),
            (KEYCODE_BUTTON_A, BTN_A),
            (KEYCODE_BUTTON_B, BTN_B),
            (KEYCODE_BUTTON_X, BTN_X),
            (KEYCODE_BUTTON_Y, BTN_Y),
            (KEYCODE_BUTTON_L1, BTN_L1),
            (KEYCODE_BUTTON_R1, BTN_R1),
            (KEYCODE_BUTTON_L2, BTN_L2),
            (KEYCODE_BUTTON_R2, BTN_R2),
            (KEYCODE_BUTTON_START, BTN_START),
            (KEYCODE_BUTTON_SELECT, BTN_SELECT),
        ];
        for (code, bit) in cases {
            assert_eq!(button_for_key(code), Some(bit), "key {code}");
        }
        for menu in [KEYCODE_BACK, KEYCODE_BUTTON_MODE, KEYCODE_ESCAPE] {
            assert_eq!(button_for_key(menu), Some(BTN_MENU));
        }
        // The volume keys, the stick clicks, a letter and the like are
        // not ours.
        for other in [24, 25, 30, 98, 101, 106, 107] {
            assert_eq!(button_for_key(other), None, "key {other}");
        }
    }

    #[test]
    fn a_keyboard_has_the_desktops_keys() {
        let cases = [
            (KEYCODE_Z, BTN_A),
            (KEYCODE_X, BTN_B),
            (KEYCODE_C, BTN_X),
            (KEYCODE_V, BTN_Y),
            (KEYCODE_A, BTN_L1),
            (KEYCODE_S, BTN_R1),
            (KEYCODE_Q, BTN_L2),
            (KEYCODE_W, BTN_R2),
            (KEYCODE_ENTER, BTN_START),
            (KEYCODE_SHIFT_RIGHT, BTN_SELECT),
        ];
        for (code, bit) in cases {
            assert_eq!(button_for_key(code), Some(bit), "key {code}");
        }
    }

    #[test]
    fn a_hat_leans_to_a_direction_past_the_threshold_and_diagonals_are_two() {
        assert_eq!(hat_buttons(0.0, 0.0), 0);
        assert_eq!(hat_buttons(-1.0, 0.0), BTN_LEFT);
        assert_eq!(hat_buttons(1.0, 0.0), BTN_RIGHT);
        assert_eq!(hat_buttons(0.0, -1.0), BTN_UP);
        assert_eq!(hat_buttons(0.0, 1.0), BTN_DOWN);
        assert_eq!(hat_buttons(1.0, 1.0), BTN_RIGHT | BTN_DOWN);
        assert_eq!(hat_buttons(-0.49, 0.49), 0);
    }

    #[test]
    fn a_trigger_pulled_a_quarter_of_the_way_holds_its_button() {
        assert_eq!(trigger_buttons(0.0, 0.0), 0);
        assert_eq!(trigger_buttons(0.24, 0.0), 0);
        assert_eq!(trigger_buttons(0.25, 0.0), BTN_L2);
        assert_eq!(trigger_buttons(0.0, 1.0), BTN_R2);
        assert_eq!(trigger_buttons(1.0, 1.0), BTN_L2 | BTN_R2);
    }
}
