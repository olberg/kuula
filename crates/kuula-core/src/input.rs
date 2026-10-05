/// One frame of logical controller state.
///
/// This is the unit of input scripting: a recorded session is a list of
/// these, and a headless host replays them. The core never sees host keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FrameInput {
    /// Held buttons, one bit each. See the `BTN_*` constants.
    pub buttons: u16,
}

pub const BTN_UP: u16 = 1 << 0;
pub const BTN_DOWN: u16 = 1 << 1;
pub const BTN_LEFT: u16 = 1 << 2;
pub const BTN_RIGHT: u16 = 1 << 3;
pub const BTN_A: u16 = 1 << 4;
pub const BTN_B: u16 = 1 << 5;
pub const BTN_X: u16 = 1 << 6;
pub const BTN_Y: u16 = 1 << 7;
pub const BTN_L1: u16 = 1 << 8;
pub const BTN_R1: u16 = 1 << 9;
pub const BTN_L2: u16 = 1 << 10;
pub const BTN_R2: u16 = 1 << 11;
pub const BTN_START: u16 = 1 << 12;
pub const BTN_SELECT: u16 = 1 << 13;
/// The Menu key. It never reaches a cart: the console strips it before
/// the cart's step and hands it to the shell. It is the top bit, above
/// every button a cart can have.
pub const BTN_MENU: u16 = 1 << 15;
/// The bits a cart may see when its manifest declares every button.
pub const CART_BUTTONS: u16 = 0x3fff;
/// The bits every cart sees, and the shell uses: the D-pad, A and B.
pub const BASE_BUTTONS: u16 = 0x3f;

/// Number of logical buttons a cart can query with `btn(n)`.
pub const BUTTON_COUNT: u8 = 14;

/// The name of each cart button in an input script, in the order of its
/// number: `BUTTON_NAMES[n]` is button `n`, whose bit is `1 << n`.
pub const BUTTON_NAMES: [&str; BUTTON_COUNT as usize] = [
    "up", "down", "left", "right", "a", "b", "x", "y", "l1", "r1", "l2", "r2", "start", "select",
];

/// Start and Select held together are Menu: the console applies this to
/// every input it is given, played or scripted, so a cart never sees both
/// at once.
///
/// While both are down they are taken away and Menu is held in their
/// place. Once that has happened neither is given back until both are up,
/// so that letting go of one before the other presses nothing in the
/// cart. Buttons that have been through it pass again unchanged.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    taken: bool,
}

impl Chord {
    const BOTH: u16 = BTN_START | BTN_SELECT;

    pub fn apply(&mut self, buttons: u16) -> u16 {
        let held = buttons & Chord::BOTH;
        if held == Chord::BOTH {
            self.taken = true;
            return (buttons & !Chord::BOTH) | BTN_MENU;
        }
        if held == 0 {
            self.taken = false;
        }
        if self.taken {
            buttons & !Chord::BOTH
        } else {
            buttons
        }
    }
}

impl FrameInput {
    pub const NONE: FrameInput = FrameInput { buttons: 0 };

    pub fn new(buttons: u16) -> FrameInput {
        FrameInput { buttons }
    }

    /// Whether logical button `n` is held: 0 = up, 1 = down, 2 = left,
    /// 3 = right, 4 = A, 5 = B, 6 = X, 7 = Y, 8 = L1, 9 = R1, 10 = L2,
    /// 11 = R2, 12 = Start, 13 = Select. Out-of-range buttons read as
    /// released.
    pub fn held(&self, n: u8) -> bool {
        n < BUTTON_COUNT && self.buttons & (1 << n) != 0
    }

    /// The same input with the Menu bit removed: what a cart sees.
    pub fn for_cart(self) -> FrameInput {
        FrameInput {
            buttons: self.buttons & CART_BUTTONS,
        }
    }

    pub fn with(self, bits: u16) -> FrameInput {
        FrameInput {
            buttons: self.buttons | bits,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cart_button_has_its_number_its_bit_and_its_name() {
        let bits = [
            BTN_UP, BTN_DOWN, BTN_LEFT, BTN_RIGHT, BTN_A, BTN_B, BTN_X, BTN_Y, BTN_L1, BTN_R1,
            BTN_L2, BTN_R2, BTN_START, BTN_SELECT,
        ];
        assert_eq!(bits.len(), BUTTON_COUNT as usize);
        assert_eq!(BUTTON_NAMES.len(), bits.len());
        for (n, bit) in bits.into_iter().enumerate() {
            assert_eq!(bit, 1 << n, "button {n}");
            let input = FrameInput::new(bit);
            for m in 0..=u8::MAX {
                assert_eq!(input.held(m), m as usize == n, "bit {n}, asked {m}");
            }
        }
        assert_eq!(bits.iter().fold(0, |all, b| all | b), CART_BUTTONS);
        assert_eq!(bits[..6].iter().fold(0, |all, b| all | b), BASE_BUTTONS);
    }

    #[test]
    fn start_and_select_together_are_menu_and_nothing_else() {
        let both = BTN_START | BTN_SELECT;
        let mut c = Chord::default();
        assert_eq!(c.apply(BTN_START), BTN_START);
        assert_eq!(c.apply(BTN_SELECT | BTN_A), BTN_SELECT | BTN_A);
        assert_eq!(c.apply(both), BTN_MENU);
        assert_eq!(c.apply(both | BTN_LEFT), BTN_MENU | BTN_LEFT);
        assert_eq!(c.apply(0), 0);
        // A Menu key of its own passes through.
        assert_eq!(c.apply(BTN_MENU | BTN_START), BTN_MENU | BTN_START);
    }

    #[test]
    fn after_the_chord_neither_comes_back_until_both_are_up() {
        let both = BTN_START | BTN_SELECT;
        let mut c = Chord::default();
        assert_eq!(c.apply(BTN_START), BTN_START);
        assert_eq!(c.apply(both), BTN_MENU);
        // Select is let go first: Start is still down and stays taken.
        assert_eq!(c.apply(BTN_START | BTN_A), BTN_A);
        // Select again while Start never came up: Menu again.
        assert_eq!(c.apply(both), BTN_MENU);
        assert_eq!(c.apply(BTN_SELECT), 0);
        assert_eq!(c.apply(0), 0);
        assert_eq!(c.apply(BTN_SELECT), BTN_SELECT);
    }

    #[test]
    fn what_the_chord_gave_passes_through_it_again_unchanged() {
        // A recorded run is replayed through a console of its own.
        let both = BTN_START | BTN_SELECT;
        let held = [
            BTN_START,
            both,
            both | BTN_A,
            BTN_START,
            0,
            BTN_SELECT,
            both,
            0,
        ];
        let (mut first, mut second) = (Chord::default(), Chord::default());
        for buttons in held {
            let once = first.apply(buttons);
            assert_eq!(second.apply(once & CART_BUTTONS), once & CART_BUTTONS);
        }
    }

    #[test]
    fn menu_is_no_cart_button() {
        assert_eq!(BTN_MENU & CART_BUTTONS, 0);
        let input = FrameInput::new(BTN_MENU | BTN_SELECT);
        assert_eq!(input.for_cart(), FrameInput::new(BTN_SELECT));
        for n in 0..=u8::MAX {
            assert!(!FrameInput::new(BTN_MENU).held(n), "button {n}");
        }
    }
}
