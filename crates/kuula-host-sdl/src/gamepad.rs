//! Controller navigation uses the same logical buttons as the keyboard.
use kuula_core::input::{BTN_A, BTN_B, BTN_DOWN, BTN_LEFT, BTN_MENU, BTN_RIGHT, BTN_UP};
use sdl2::controller::{Button, GameController};
use sdl2::{event::Event, GameControllerSubsystem};

pub struct Gamepads {
    subsystem: GameControllerSubsystem,
    pads: Vec<GameController>,
    buttons: u8,
}
fn bit(b: Button) -> u8 {
    match b {
        Button::A => BTN_A,
        Button::B => BTN_B,
        Button::DPadUp => BTN_UP,
        Button::DPadDown => BTN_DOWN,
        Button::DPadLeft => BTN_LEFT,
        Button::DPadRight => BTN_RIGHT,
        Button::Start | Button::Guide => BTN_MENU,
        _ => 0,
    }
}
impl Gamepads {
    pub fn new(sdl: &sdl2::Sdl) -> Result<Self, String> {
        let subsystem = sdl.game_controller()?;
        let mut pads = Vec::new();
        for id in 0..subsystem.num_joysticks()? {
            if pads.len() < 4 {
                if let Ok(pad) = subsystem.open(id) {
                    pads.push(pad);
                }
            }
        }
        Ok(Self {
            subsystem,
            pads,
            buttons: 0,
        })
    }
    pub fn event(&mut self, event: &Event) {
        match event {
            // SDL queues an Added event for every pad already open from
            // `new`, so a pad is admitted once by instance id.
            Event::ControllerDeviceAdded { which, .. } if self.pads.len() < 4 => {
                if let Ok(pad) = self.subsystem.open(*which) {
                    if !self
                        .pads
                        .iter()
                        .any(|p| p.instance_id() == pad.instance_id())
                    {
                        self.pads.push(pad);
                    }
                }
            }
            Event::ControllerDeviceRemoved { which, .. } => {
                self.pads.retain(|p| p.instance_id() != *which);
                self.buttons = 0;
            }
            _ => {}
        }
    }
    pub fn buttons(&mut self) -> u8 {
        self.buttons = 0;
        for pad in &self.pads {
            for b in [
                Button::A,
                Button::B,
                Button::DPadUp,
                Button::DPadDown,
                Button::DPadLeft,
                Button::DPadRight,
                Button::Start,
                Button::Guide,
            ] {
                if pad.button(b) {
                    self.buttons |= bit(b);
                }
            }
        }
        self.buttons
    }
}
