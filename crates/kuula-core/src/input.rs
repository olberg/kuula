/// One frame of logical controller state.
///
/// This is the unit of input scripting: a recorded session is a list of
/// these, and a headless host replays them. The core never sees host keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FrameInput {
    /// Held buttons, one bit each. See the `BTN_*` constants.
    pub buttons: u8,
}

pub const BTN_UP: u8 = 1 << 0;
pub const BTN_DOWN: u8 = 1 << 1;
pub const BTN_LEFT: u8 = 1 << 2;
pub const BTN_RIGHT: u8 = 1 << 3;
pub const BTN_A: u8 = 1 << 4;
pub const BTN_B: u8 = 1 << 5;
/// The Menu key. It never reaches a cart: the console strips it before
/// the cart's step and hands it to the shell.
pub const BTN_MENU: u8 = 1 << 6;
/// The bits a cart may see.
pub const CART_BUTTONS: u8 = 0x3f;

/// Number of logical buttons a cart can query with `btn(n)`.
pub const BUTTON_COUNT: u8 = 6;

impl FrameInput {
    pub const NONE: FrameInput = FrameInput { buttons: 0 };

    pub fn new(buttons: u8) -> FrameInput {
        FrameInput { buttons }
    }

    /// Whether logical button `n` (0 = up, 1 = down, 2 = left, 3 = right,
    /// 4 = A, 5 = B) is held. Out-of-range buttons read as released.
    pub fn held(&self, n: u8) -> bool {
        n < BUTTON_COUNT && self.buttons & (1 << n) != 0
    }

    /// The same input with the Menu bit removed: what a cart sees.
    pub fn for_cart(self) -> FrameInput {
        FrameInput {
            buttons: self.buttons & CART_BUTTONS,
        }
    }

    pub fn with(self, bits: u8) -> FrameInput {
        FrameInput {
            buttons: self.buttons | bits,
        }
    }
}
