//! Desktop input for shell-only fields. Clipboard access is player initiated.
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
// Key-up survives the editor closing on key-down. Escape also releases
// the ordinary Menu mapping in the caller when no editor is open.
fn release_editor_key(event: &Event, keys: &mut KeyState) -> bool {
    match event {
        Event::KeyUp {
            keycode: Some(Keycode::Return),
            ..
        } => {
            keys.release(Keycode::Z);
            true
        }
        Event::KeyUp {
            keycode: Some(Keycode::Escape),
            ..
        } => {
            keys.release(Keycode::X);
            false
        }
        _ => false,
    }
}
pub fn event(
    event: &Event,
    console: &mut Console,
    video: &VideoSubsystem,
    keys: &mut KeyState,
) -> bool {
    if release_editor_key(event, keys) {
        return true;
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
