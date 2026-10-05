//! Keyboard to logical controller. Which key of a desktop keyboard is
//! which button is every host's to share (`kuula_host_common::controls`):
//! the arrows, Z, X, C and V for A, B, X and Y, and so on. `Ctrl+1` to
//! `Ctrl+4` are host hotkeys for the window scale and never reach the
//! cart. On the Miyoo Mini the buttons arrive as keys too, from the
//! fork's Mini video driver, and [`Layout::Handheld`] reads them: A is
//! Space, B is Left Ctrl, Menu is Escape, and the rest are in
//! [`Layout::button_bit`].

use kuula_core::input::{
    BTN_A, BTN_B, BTN_DOWN, BTN_L1, BTN_L2, BTN_LEFT, BTN_MENU, BTN_R1, BTN_R2, BTN_RIGHT,
    BTN_SELECT, BTN_START, BTN_UP, BTN_X, BTN_Y,
};
use kuula_core::FrameInput;
use kuula_host_common::controls::Key;
use sdl2::keyboard::{Keycode, Mod};

/// Which physical keys stand for the buttons.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    #[default]
    Desktop,
    Handheld,
}

impl Layout {
    pub fn for_profile(profile: crate::device::Profile) -> Layout {
        match profile {
            crate::device::Profile::Desktop => Layout::Desktop,
            crate::device::Profile::MiyooMini => Layout::Handheld,
        }
    }

    /// The `FrameInput` bit for a key, if it is mapped.
    pub fn button_bit(self, key: Keycode) -> Option<u16> {
        match self {
            Layout::Desktop => desktop_key(key).map(Key::button),
            Layout::Handheld => handheld_bit(key),
        }
    }
}

/// The key of a desktop keyboard an SDL key code is, of those that are
/// buttons.
fn desktop_key(key: Keycode) -> Option<Key> {
    Some(match key {
        Keycode::Up => Key::Up,
        Keycode::Down => Key::Down,
        Keycode::Left => Key::Left,
        Keycode::Right => Key::Right,
        Keycode::Z => Key::Z,
        Keycode::X => Key::X,
        Keycode::C => Key::C,
        Keycode::V => Key::V,
        Keycode::A => Key::A,
        Keycode::S => Key::S,
        Keycode::Q => Key::Q,
        Keycode::W => Key::W,
        Keycode::Return => Key::Enter,
        Keycode::RShift => Key::RightShift,
        Keycode::Escape => Key::Escape,
        _ => return None,
    })
}

/// The keys the Miyoo Mini's video driver sends for the device's buttons.
fn handheld_bit(key: Keycode) -> Option<u16> {
    match key {
        Keycode::Up => Some(BTN_UP),
        Keycode::Down => Some(BTN_DOWN),
        Keycode::Left => Some(BTN_LEFT),
        Keycode::Right => Some(BTN_RIGHT),
        Keycode::Escape => Some(BTN_MENU),
        Keycode::Space => Some(BTN_A),
        Keycode::LCtrl => Some(BTN_B),
        Keycode::LShift => Some(BTN_X),
        Keycode::LAlt => Some(BTN_Y),
        Keycode::E => Some(BTN_L1),
        Keycode::T => Some(BTN_R1),
        Keycode::Tab => Some(BTN_L2),
        Keycode::Backspace => Some(BTN_R2),
        Keycode::Return => Some(BTN_START),
        Keycode::RCtrl => Some(BTN_SELECT),
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
    buttons: u16,
    layout: Layout,
}

impl KeyState {
    pub fn new(layout: Layout) -> KeyState {
        KeyState { buttons: 0, layout }
    }

    pub fn press(&mut self, key: Keycode) {
        if let Some(bit) = self.layout.button_bit(key) {
            self.buttons |= bit;
        }
    }

    pub fn release(&mut self, key: Keycode) {
        if let Some(bit) = self.layout.button_bit(key) {
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
            (Keycode::C, BTN_X),
            (Keycode::V, BTN_Y),
            (Keycode::A, BTN_L1),
            (Keycode::S, BTN_R1),
            (Keycode::Q, BTN_L2),
            (Keycode::W, BTN_R2),
            (Keycode::Return, BTN_START),
            (Keycode::RShift, BTN_SELECT),
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
            Keycode::D,
            Keycode::Space,
            Keycode::Tab,
            Keycode::Num1,
            Keycode::LCtrl,
            Keycode::LShift,
        ] {
            s.press(key);
        }
        assert_eq!(s.input(), FrameInput::NONE);
        for layout in [Layout::Desktop, Layout::Handheld] {
            assert_eq!(layout.button_bit(Keycode::Escape), Some(BTN_MENU));
        }
    }

    #[test]
    fn the_handheld_layout_reads_the_face_buttons_as_keys() {
        let mut s = KeyState::new(Layout::Handheld);
        s.press(Keycode::Space);
        s.press(Keycode::LCtrl);
        s.press(Keycode::Escape);
        s.press(Keycode::Left);
        assert_eq!(s.input().buttons, BTN_A | BTN_B | BTN_MENU | BTN_LEFT);
        s.release(Keycode::Space);
        assert_eq!(s.input().buttons, BTN_B | BTN_MENU | BTN_LEFT);
    }

    #[test]
    fn the_handheld_layout_has_a_key_for_every_button() {
        let cases = [
            (Keycode::LShift, BTN_X),
            (Keycode::LAlt, BTN_Y),
            (Keycode::E, BTN_L1),
            (Keycode::T, BTN_R1),
            (Keycode::Tab, BTN_L2),
            (Keycode::Backspace, BTN_R2),
            (Keycode::Return, BTN_START),
            (Keycode::RCtrl, BTN_SELECT),
        ];
        for (key, bit) in cases {
            let mut s = KeyState::new(Layout::Handheld);
            s.press(key);
            assert_eq!(s.input(), FrameInput::new(bit), "{key:?}");
        }
        // The desktop's letters are not the device's.
        let mut s = KeyState::new(Layout::Handheld);
        for key in [Keycode::Z, Keycode::X, Keycode::C, Keycode::A, Keycode::Q] {
            s.press(key);
        }
        assert_eq!(s.input(), FrameInput::NONE);
    }

    #[test]
    fn the_desktop_layout_leaves_space_and_ctrl_alone() {
        let mut s = KeyState::new(Layout::Desktop);
        s.press(Keycode::Space);
        s.press(Keycode::LCtrl);
        assert_eq!(s.input(), FrameInput::NONE);
        assert_eq!(Layout::default(), Layout::Desktop);
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
