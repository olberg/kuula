//! What every host agrees on about a keyboard and a controller, whatever
//! library its key codes and axes come from: which key of a desktop
//! keyboard is which button, and how far a trigger is pulled before it is
//! one.

use kuula_core::input::{
    BTN_A, BTN_B, BTN_DOWN, BTN_L1, BTN_L2, BTN_LEFT, BTN_MENU, BTN_R1, BTN_R2, BTN_RIGHT,
    BTN_SELECT, BTN_START, BTN_UP, BTN_X, BTN_Y,
};

/// A key of a keyboard that is a button. A host names its own key codes
/// with these, and [`Key::button`] says what each one is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Z,
    X,
    C,
    V,
    A,
    S,
    Q,
    W,
    Enter,
    RightShift,
    Escape,
}

impl Key {
    /// Every key that is a button.
    pub const ALL: [Key; 15] = [
        Key::Up,
        Key::Down,
        Key::Left,
        Key::Right,
        Key::Z,
        Key::X,
        Key::C,
        Key::V,
        Key::A,
        Key::S,
        Key::Q,
        Key::W,
        Key::Enter,
        Key::RightShift,
        Key::Escape,
    ];

    /// The button a key of a desktop keyboard stands for, as a `BTN_*`
    /// bit: the arrows are the D-pad, the face buttons are along the
    /// bottom row and the shoulders above them. No modifier that a hotkey
    /// uses is a button.
    pub fn button(self) -> u16 {
        match self {
            Key::Up => BTN_UP,
            Key::Down => BTN_DOWN,
            Key::Left => BTN_LEFT,
            Key::Right => BTN_RIGHT,
            Key::Z => BTN_A,
            Key::X => BTN_B,
            Key::C => BTN_X,
            Key::V => BTN_Y,
            Key::A => BTN_L1,
            Key::S => BTN_R1,
            Key::Q => BTN_L2,
            Key::W => BTN_R2,
            Key::Enter => BTN_START,
            Key::RightShift => BTN_SELECT,
            Key::Escape => BTN_MENU,
        }
    }
}

/// How far a trigger must be pulled, out of 1, to hold L2 or R2.
pub const TRIGGER_THRESHOLD: f32 = 0.25;

/// Whether a trigger pulled `travel` of the way, from 0 to 1, holds its
/// button.
pub fn trigger_pulled(travel: f32) -> bool {
    travel >= TRIGGER_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuula_core::input::CART_BUTTONS;

    #[test]
    fn a_keyboard_has_a_key_for_every_button_and_for_menu() {
        let mut seen = 0;
        for key in Key::ALL {
            let bit = key.button();
            assert_eq!(bit.count_ones(), 1, "{key:?}");
            assert_eq!(seen & bit, 0, "{key:?} shares a button");
            seen |= bit;
        }
        assert_eq!(seen, CART_BUTTONS | BTN_MENU);
        assert_eq!(Key::Z.button(), BTN_A);
        assert_eq!(Key::X.button(), BTN_B);
        assert_eq!(Key::Enter.button(), BTN_START);
    }

    #[test]
    fn a_trigger_is_a_button_from_a_quarter_of_the_way() {
        assert!(!trigger_pulled(0.0));
        assert!(!trigger_pulled(0.24));
        assert!(trigger_pulled(0.25));
        assert!(trigger_pulled(1.0));
    }
}
