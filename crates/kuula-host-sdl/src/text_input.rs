//! Desktop input for shell-only fields. Clipboard access is player initiated.
//! The host does not call `event` on a handheld, whose buttons are keys:
//! there a field is filled with the shell's on-screen keys alone.
//! The supplied KeyState holds only synthetic editor buttons, independently
//! of physical keyboard state, so releasing Enter cannot release a held Z.
use crate::keys::KeyState;
use kuula_core::{
    shell::{
        network::{Action, View},
        SysRequest,
    },
    Console,
};
use sdl2::{
    event::Event,
    keyboard::{Keycode, Mod},
    VideoSubsystem,
};

fn append(view: &mut View, text: &str) {
    let text = text.trim();
    if !text.is_ascii()
        || text.bytes().any(|b| b.is_ascii_control())
        || view.text.len() + text.len() > kuula_core::net::MAX_TICKET
    {
        view.detail = "Use printable text, at most 1024 characters.".into();
    } else {
        view.text.push_str(text);
    }
}
fn paste(view: &mut View, video: &VideoSubsystem) {
    match video.clipboard().clipboard_text() {
        Ok(s) => append(view, &s),
        Err(e) => view.detail = format!("Cannot read clipboard: {e}"),
    }
}
// Key-up survives the editor closing on key-down. The event is left to
// the caller, which releases the key's ordinary mapping: Start for Enter
// and Menu for Escape, held or not.
fn release_editor_key(event: &Event, keys: &mut KeyState) {
    match event {
        Event::KeyUp {
            keycode: Some(Keycode::Return),
            ..
        } => keys.release(Keycode::Z),
        Event::KeyUp {
            keycode: Some(Keycode::Escape),
            ..
        } => keys.release(Keycode::X),
        _ => {}
    }
}
pub fn event(
    event: &Event,
    console: &mut Console,
    video: &VideoSubsystem,
    keys: &mut KeyState,
) -> bool {
    release_editor_key(event, keys);
    // A question about a developer covers a field that is being edited:
    // the keys answer it, and none of them is typed into what is hidden.
    if console.dev_view().is_some_and(|d| !d.pending.is_empty()) {
        return false;
    }
    let Some(v) = console.network_view_mut().filter(|v| !v.editing.is_empty()) else {
        return false;
    };
    match event {
        Event::TextInput { text, .. } => {
            append(v, text);
            true
        }
        Event::KeyDown {
            keycode: Some(k),
            keymod,
            repeat,
            ..
        } => {
            if keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) && *k == Keycode::V {
                if !repeat {
                    paste(v, video);
                }
                return true;
            }
            match *k {
                Keycode::Backspace => {
                    v.text.pop();
                    true
                }
                Keycode::Return => {
                    if !repeat {
                        keys.press(Keycode::Z);
                    }
                    true
                }
                Keycode::Escape => {
                    if !repeat {
                        keys.press(Keycode::X);
                    }
                    true
                }
                Keycode::Up | Keycode::Down | Keycode::Left | Keycode::Right => false,
                _ => true, // Typed Z/X are text here, not A/B.
            }
        }
        // KeyUp of Return and Escape is handled by `release_editor_key`.
        _ => false,
    }
}
pub fn requests(requests: &[SysRequest], console: &mut Console, video: &VideoSubsystem) {
    let Some(v) = console.network_view_mut() else {
        return;
    };
    for r in requests {
        match r {
            SysRequest::Network(Action::Edit(field)) => {
                v.editing = field.clone();
                v.text = if field == "relay" {
                    v.relay.clone()
                } else {
                    String::new()
                };
            }
            SysRequest::Network(Action::Text(text)) => v.text = text.clone(),
            SysRequest::Network(Action::Paste) => paste(v, video),
            SysRequest::Network(Action::Copy) => {
                if let Err(e) = video.clipboard().set_clipboard_text(&v.ticket) {
                    v.detail = format!("Cannot copy ticket: {e}");
                } else {
                    v.detail = "Ticket copied.".into();
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_release_clears_editor_buttons_after_editor_has_closed() {
        let mut keys = KeyState::default();
        let mut physical_keys = KeyState::default();
        physical_keys.press(Keycode::Z);
        physical_keys.press(Keycode::X);
        for (physical, logical) in [(Keycode::Return, Keycode::Z), (Keycode::Escape, Keycode::X)] {
            keys.press(logical);
            release_editor_key(
                &Event::KeyUp {
                    timestamp: 0,
                    window_id: 0,
                    keycode: Some(physical),
                    scancode: None,
                    keymod: Mod::NOMOD,
                    repeat: false,
                },
                &mut keys,
            );
            assert_eq!(keys.input(), kuula_core::FrameInput::NONE);
            assert_eq!(
                physical_keys.input().buttons,
                kuula_core::input::BTN_A | kuula_core::input::BTN_B
            );
        }
    }
    #[test]
    fn clipboard_text_is_bounded_and_controls_rejected() {
        let mut v = View::default();
        append(&mut v, " abc ");
        assert_eq!(v.text, "abc");
        append(&mut v, "a\nb");
        assert_eq!(v.text, "abc");
        append(&mut v, &"x".repeat(1024));
        assert_eq!(v.text, "abc");
    }
}
