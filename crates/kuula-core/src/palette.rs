use std::fmt;

/// Number of palette entries. A colour index is 7 bits.
pub const PALETTE_SIZE: usize = 128;

/// Entries `0..SYSTEM_COLOURS` are the system palette. They are locked:
/// the shell's error screen and overlays rely on them.
pub const SYSTEM_COLOURS: usize = 16;

pub type Rgb = [u8; 3];

/// The 16 system colours. Sweetie-16 ordering, chosen so that 0 is black,
/// 7 is a bright white and 8 is red, matching common fantasy console
/// habits.
pub const SYSTEM_PALETTE: [Rgb; SYSTEM_COLOURS] = [
    [0x00, 0x00, 0x00], // 0 black
    [0x1d, 0x2b, 0x53], // 1 dark blue
    [0x7e, 0x25, 0x53], // 2 dark purple
    [0x00, 0x87, 0x51], // 3 dark green
    [0xab, 0x52, 0x36], // 4 brown
    [0x5f, 0x57, 0x4f], // 5 dark grey
    [0xc2, 0xc3, 0xc7], // 6 light grey
    [0xff, 0xf1, 0xe8], // 7 white
    [0xff, 0x00, 0x4d], // 8 red
    [0xff, 0xa3, 0x00], // 9 orange
    [0xff, 0xec, 0x27], // 10 yellow
    [0x00, 0xe4, 0x36], // 11 green
    [0x29, 0xad, 0xff], // 12 blue
    [0x83, 0x76, 0x9c], // 13 lavender
    [0xff, 0x77, 0xa8], // 14 pink
    [0xff, 0xcc, 0xaa], // 15 peach
];

/// The default full palette: the system colours followed by a 4x4x7 RGB
/// ramp in the remaining 112 entries. Built at compile time.
pub const DEFAULT_PALETTE: [Rgb; PALETTE_SIZE] = build_default_palette();

const fn build_default_palette() -> [Rgb; PALETTE_SIZE] {
    let mut out = [[0u8; 3]; PALETTE_SIZE];
    let mut i = 0;
    while i < SYSTEM_COLOURS {
        out[i] = SYSTEM_PALETTE[i];
        i += 1;
    }
    while i < PALETTE_SIZE {
        let n = i - SYSTEM_COLOURS;
        let r = (n % 4) as u8;
        let g = ((n / 4) % 4) as u8;
        let b = ((n / 16) % 7) as u8;
        out[i] = [r * 85, g * 85, b * 42];
        i += 1;
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteError {
    /// Index is at or above [`PALETTE_SIZE`].
    OutOfRange { index: usize },
    /// Index is one of the locked system colours.
    Locked { index: usize },
}

impl PaletteError {
    /// Stable error code.
    pub fn code(&self) -> &'static str {
        match self {
            PaletteError::OutOfRange { .. } => "palette_index_out_of_range",
            PaletteError::Locked { .. } => "palette_index_locked",
        }
    }
}

impl fmt::Display for PaletteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaletteError::OutOfRange { index } => {
                write!(f, "palette index {index} out of range 0..{PALETTE_SIZE}")
            }
            PaletteError::Locked { index } => {
                write!(f, "palette index {index} is a locked system colour")
            }
        }
    }
}

impl std::error::Error for PaletteError {}

/// 128 RGB888 entries. The first 16 are copied from the ROM at guest
/// creation and cannot be changed by a cart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    entries: [Rgb; PALETTE_SIZE],
}

impl Default for Palette {
    fn default() -> Palette {
        Palette {
            entries: DEFAULT_PALETTE,
        }
    }
}

impl Palette {
    pub fn entries(&self) -> &[Rgb; PALETTE_SIZE] {
        &self.entries
    }

    /// Colour for `index & 0x7f`.
    pub fn get(&self, index: u8) -> Rgb {
        self.entries[(index & 0x7f) as usize]
    }

    /// Set a cart-owned entry. Fails on the system colours and out of
    /// range.
    pub fn set(&mut self, index: usize, rgb: Rgb) -> Result<(), PaletteError> {
        if index >= PALETTE_SIZE {
            return Err(PaletteError::OutOfRange { index });
        }
        if index < SYSTEM_COLOURS {
            return Err(PaletteError::Locked { index });
        }
        self.entries[index] = rgb;
        Ok(())
    }

    /// Replace every cart-owned entry at once and keep the system
    /// colours as they are. For a palette that arrives from another
    /// process, whose system entries are not trusted.
    pub fn set_cart_entries(&mut self, entries: &[Rgb; PALETTE_SIZE]) {
        self.entries[SYSTEM_COLOURS..].copy_from_slice(&entries[SYSTEM_COLOURS..]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_on_system_colours_fails() {
        let mut p = Palette::default();
        for (i, sys) in SYSTEM_PALETTE.iter().enumerate() {
            assert_eq!(p.set(i, [1, 2, 3]), Err(PaletteError::Locked { index: i }));
            assert_eq!(p.entries()[i], *sys);
        }
    }

    #[test]
    fn set_on_cart_colours_succeeds() {
        let mut p = Palette::default();
        for i in SYSTEM_COLOURS..PALETTE_SIZE {
            assert_eq!(p.set(i, [i as u8, 2, 3]), Ok(()));
            assert_eq!(p.entries()[i], [i as u8, 2, 3]);
        }
    }

    #[test]
    fn set_at_128_fails() {
        let mut p = Palette::default();
        assert_eq!(
            p.set(128, [0, 0, 0]),
            Err(PaletteError::OutOfRange { index: 128 })
        );
        assert_eq!(
            p.set(usize::MAX, [0, 0, 0]).unwrap_err().code(),
            "palette_index_out_of_range"
        );
    }

    /// The brand roles of the shell (ink, midnight, ivory, ...) are drawn from
    /// the locked system colours. Moving one moves the shell's identity.
    #[test]
    fn system_colours_carry_the_brand_roles() {
        let roles = [
            ("ink", 0, [0x00, 0x00, 0x00]),
            ("midnight", 1, [0x1d, 0x2b, 0x53]),
            ("mist", 6, [0xc2, 0xc3, 0xc7]),
            ("ivory", 7, [0xff, 0xf1, 0xe8]),
            ("red", 8, [0xff, 0x00, 0x4d]),
            ("sky", 12, [0x29, 0xad, 0xff]),
            ("lavender", 13, [0x83, 0x76, 0x9c]),
            ("peach", 15, [0xff, 0xcc, 0xaa]),
        ];
        for (name, index, rgb) in roles {
            assert_eq!(
                SYSTEM_PALETTE[index], rgb,
                "{name} is system colour {index}"
            );
        }
    }

    #[test]
    fn default_palette_starts_with_system_colours() {
        let p = Palette::default();
        assert_eq!(&p.entries()[..SYSTEM_COLOURS], &SYSTEM_PALETTE[..]);
        assert_eq!(p.get(0), [0, 0, 0]);
        assert_eq!(p.get(7), [0xff, 0xf1, 0xe8]);
        assert_eq!(p.get(0x87), p.get(7), "high bit is ignored");
    }
}
