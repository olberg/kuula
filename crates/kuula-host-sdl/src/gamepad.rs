//! Controller navigation uses the same logical buttons as the keyboard.
use kuula_core::input::{
    BTN_A, BTN_B, BTN_DOWN, BTN_L1, BTN_L2, BTN_LEFT, BTN_MENU, BTN_R1, BTN_R2, BTN_RIGHT,
    BTN_SELECT, BTN_START, BTN_UP, BTN_X, BTN_Y,
};
use kuula_host_common::controls::trigger_pulled;
use sdl2::controller::{Axis, Button, GameController};
use sdl2::{event::Event, GameControllerSubsystem};

pub struct Gamepads {
    subsystem: GameControllerSubsystem,
    pads: Vec<GameController>,
    buttons: u16,
}
/// A controller's buttons by the names SDL gives them. Guide is Menu where
/// it reaches the program at all; Start and Back held together are Menu
/// everywhere, which the console sees to.
const BUTTONS: [(Button, u16); 13] = [
    (Button::A, BTN_A),
    (Button::B, BTN_B),
    (Button::X, BTN_X),
    (Button::Y, BTN_Y),
    (Button::DPadUp, BTN_UP),
    (Button::DPadDown, BTN_DOWN),
    (Button::DPadLeft, BTN_LEFT),
    (Button::DPadRight, BTN_RIGHT),
    (Button::LeftShoulder, BTN_L1),
    (Button::RightShoulder, BTN_R1),
    (Button::Start, BTN_START),
    (Button::Back, BTN_SELECT),
    (Button::Guide, BTN_MENU),
];
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
    pub fn buttons(&mut self) -> u16 {
        self.buttons = 0;
        for pad in &self.pads {
            for (b, bit) in BUTTONS {
                if pad.button(b) {
                    self.buttons |= bit;
                }
            }
            // L2 and R2 are axes to SDL, from 0 to `i16::MAX`, whether
            // the controller has triggers that travel or two more buttons.
            for (axis, bit) in [(Axis::TriggerLeft, BTN_L2), (Axis::TriggerRight, BTN_R2)] {
                if trigger_pulled(pad.axis(axis) as f32 / i16::MAX as f32) {
                    self.buttons |= bit;
                }
            }
        }
        self.buttons
    }
}
