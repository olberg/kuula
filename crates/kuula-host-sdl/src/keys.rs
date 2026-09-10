//! Keyboard to logical controller. Arrows are the D-pad, Z is A, X is B.
//! `Ctrl+1` to `Ctrl+4` are host hotkeys for the window scale and never
//! reach the cart.

use kuula_core::input::{BTN_A, BTN_B, BTN_DOWN, BTN_LEFT, BTN_MENU, BTN_RIGHT, BTN_UP};
use kuula_core::FrameInput;
use sdl2::keyboard::{Keycode, Mod};

/// The `FrameInput` bit for a key, if it is mapped.
pub fn button_bit(key: Keycode) -> Option<u8> {
    match key {
        Keycode::Up => Some(BTN_UP),
        Keycode::Down => Some(BTN_DOWN),
        Keycode::Left => Some(BTN_LEFT),
        Keycode::Right => Some(BTN_RIGHT),
        Keycode::Z => Some(BTN_A),
        Keycode::X => Some(BTN_B),
        Keycode::Escape => Some(BTN_MENU),
        _ => None,
    }
}

/// `Ctrl+1` to `Ctrl+4` select a window scale.
pub fn scale_hotkey(key: Keycode, keymod: Mod) -> Option<u32> {
    if !keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) {
        return None;
    }
    match key {
        Keycode::Num1 => Some(1),
        Keycode::Num2 => Some(2),
        Keycode::Num3 => Some(3),
        Keycode::Num4 => Some(4),
        _ => None,
    }
}

/// Held buttons, updated from key events, sampled once per frame.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyState {
    buttons: u8,
}

impl KeyState {
    pub fn press(&mut self, key: Keycode) {
        if let Some(bit) = button_bit(key) {
            self.buttons |= bit;
        }
    }

    pub fn release(&mut self, key: Keycode) {
        if let Some(bit) = button_bit(key) {
            self.buttons &= !bit;
        }
    }

    pub fn input(&self) -> FrameInput {
        FrameInput::new(self.buttons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_keys_produce_their_bits() {
        let cases = [
            (Keycode::Up, BTN_UP),
            (Keycode::Down, BTN_DOWN),
            (Keycode::Left, BTN_LEFT),
            (Keycode::Right, BTN_RIGHT),
            (Keycode::Z, BTN_A),
            (Keycode::X, BTN_B),
        ];
        for (key, bit) in cases {
            let mut s = KeyState::default();
            s.press(key);
            assert_eq!(s.input(), FrameInput::new(bit), "{key:?}");
            s.release(key);
            assert_eq!(s.input(), FrameInput::NONE);
        }
    }

    #[test]
    fn unmapped_keys_are_ignored() {
        let mut s = KeyState::default();
        for key in [
            Keycode::A,
            Keycode::Space,
            Keycode::Return,
            Keycode::Num1,
            Keycode::LCtrl,
        ] {
            s.press(key);
        }
        assert_eq!(s.input(), FrameInput::NONE);
        assert_eq!(button_bit(Keycode::Escape), Some(BTN_MENU));
    }

    #[test]
    fn several_keys_combine() {
        let mut s = KeyState::default();
        s.press(Keycode::Up);
        s.press(Keycode::Right);
        s.press(Keycode::Z);
        assert_eq!(s.input().buttons, BTN_UP | BTN_RIGHT | BTN_A);
        s.release(Keycode::Right);
        assert_eq!(s.input().buttons, BTN_UP | BTN_A);
    }

    #[test]
    fn ctrl_digits_select_a_scale() {
        assert_eq!(scale_hotkey(Keycode::Num1, Mod::LCTRLMOD), Some(1));
        assert_eq!(scale_hotkey(Keycode::Num4, Mod::RCTRLMOD), Some(4));
        assert_eq!(
            scale_hotkey(Keycode::Num2, Mod::LCTRLMOD | Mod::LSHIFTMOD),
            Some(2)
        );
        assert_eq!(scale_hotkey(Keycode::Num2, Mod::NOMOD), None);
        assert_eq!(scale_hotkey(Keycode::Num5, Mod::LCTRLMOD), None);
        assert_eq!(scale_hotkey(Keycode::Z, Mod::LCTRLMOD), None);
    }
}
