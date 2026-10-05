//! The activity's input queue: touches into the on-screen pad, keys and a
//! joystick's hat and triggers into what a controller holds.

use android_activity::input::{
    Axis, InputEvent, KeyAction, KeyEvent, MotionAction, MotionEvent, Source,
};
use android_activity::{AndroidApp, InputStatus};
use kuula_host_common::chord::MenuChord;
use kuula_touch::{Layout, Pad};

use crate::controls::Controls;

/// Everything the person's hands hold down.
pub struct Input {
    /// Fingers on the on-screen controls.
    pub pad: Pad,
    /// Keys, a hat and triggers.
    pub controls: Controls,
    /// Start and Select held together are Menu.
    chord: MenuChord,
    /// Buttons that went down since the last frame took its input. A tap
    /// that is over before the next frame begins would otherwise never be
    /// seen, as on the desktop host.
    tapped: u16,
}

impl Input {
    pub fn new() -> Input {
        Input {
            pad: Pad::new(),
            controls: Controls::default(),
            chord: MenuChord::default(),
            tapped: 0,
        }
    }

    /// The buttons held, from the fingers, the keys, the hat and the
    /// triggers.
    pub fn buttons(&self) -> u16 {
        self.pad.buttons() | self.controls.buttons()
    }

    /// A frame's buttons: what is held, and what went down since the last
    /// frame even if it has been let go already. Each tap is given to one
    /// frame.
    pub fn sample(&mut self) -> u16 {
        let buttons = self.buttons() | self.tapped;
        self.tapped = 0;
        self.chord.apply(buttons)
    }

    /// Forget everything held: coordinates of fingers belonged to a window
    /// that is gone or has another shape, and no key-up will come for keys
    /// held when the focus went.
    pub fn release_all(&mut self) {
        self.pad.cancel();
        self.controls.release_all();
        self.chord = MenuChord::default();
        self.tapped = 0;
    }
}

/// Take every event that is queued. Touches that arrive while there is no
/// layout (no window) are dropped.
pub fn drain(app: &AndroidApp, input: &mut Input, layout: Option<&Layout>) {
    let mut events = match app.input_events_iter() {
        Ok(events) => events,
        Err(e) => {
            log::warn!("input: no event queue: {e:?}");
            return;
        }
    };
    while events.next(|event| {
        let status = match event {
            InputEvent::KeyEvent(key) => key_event(key, &mut input.controls),
            InputEvent::MotionEvent(motion) => motion_event(motion, input, layout),
            _ => InputStatus::Unhandled,
        };
        // Whatever this event put down counts for the next frame, even if
        // a later event in the same batch lifts it again.
        input.tapped |= input.buttons();
        status
    }) {}
}

fn key_event(key: &KeyEvent, controls: &mut Controls) -> InputStatus {
    let code = u32::from(key.key_code());
    let ours = match key.action() {
        KeyAction::Down => controls.key_down(code),
        KeyAction::Up => controls.key_up(code),
        _ => crate::keymap::button_for_key(code).is_some(),
    };
    // Handled keeps Back from leaving the app: it is Menu here.
    if ours {
        InputStatus::Handled
    } else {
        InputStatus::Unhandled
    }
}

fn is_joystick(source: Source) -> bool {
    matches!(source, Source::Gamepad | Source::Joystick) || source.is_joystick_class()
}

fn motion_event(motion: &MotionEvent, input: &mut Input, layout: Option<&Layout>) -> InputStatus {
    if is_joystick(motion.source()) {
        // A hat is two axes on the first pointer: -1, 0 or 1 each. A
        // trigger is one of two axes, by the controller; the other
        // reads 0.
        let pointer = motion.pointer_at_index(0);
        input.controls.axes(
            motion.device_id(),
            (
                pointer.axis_value(Axis::HatX),
                pointer.axis_value(Axis::HatY),
            ),
            (
                pointer
                    .axis_value(Axis::Ltrigger)
                    .max(pointer.axis_value(Axis::Brake)),
                pointer
                    .axis_value(Axis::Rtrigger)
                    .max(pointer.axis_value(Axis::Gas)),
            ),
        );
        return InputStatus::Handled;
    }
    let Some(layout) = layout else {
        return InputStatus::Handled;
    };
    match motion.action() {
        MotionAction::Down | MotionAction::PointerDown => {
            let p = motion.pointer_at_index(motion.pointer_index());
            input.controls.touched();
            input.pad.down(layout, p.pointer_id() as u64, p.x(), p.y());
        }
        MotionAction::Move => {
            for p in motion.pointers() {
                input.pad.moved(layout, p.pointer_id() as u64, p.x(), p.y());
            }
        }
        MotionAction::Up | MotionAction::PointerUp => {
            let p = motion.pointer_at_index(motion.pointer_index());
            input.pad.up(p.pointer_id() as u64);
        }
        MotionAction::Cancel => input.pad.cancel(),
        _ => return InputStatus::Unhandled,
    }
    InputStatus::Handled
}
