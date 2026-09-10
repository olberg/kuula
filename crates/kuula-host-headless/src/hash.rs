//! The canonical conformance hash: FNV-1a 64
//! over each frame's indexed pixels, then its 128 palette entries, then
//! its audio samples as little-endian bytes, in frame order. Never over
//! encoded PNG or WAV bytes.

use crate::OwnedFrame;

const OFFSET: u64 = 0xcbf29ce484222325;
const PRIME: u64 = 0x100000001b3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hasher {
    state: u64,
}

impl Default for Hasher {
    fn default() -> Hasher {
        Hasher::new()
    }
}

impl Hasher {
    pub fn new() -> Hasher {
        Hasher { state: OFFSET }
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.state ^= b as u64;
            self.state = self.state.wrapping_mul(PRIME);
        }
    }

    /// Fold one frame in: pixels, then palette, then audio.
    pub fn frame(&mut self, out: &OwnedFrame) {
        self.parts(&out.pixels, &out.palette, &out.audio);
    }

    /// The same, from the raw parts.
    pub fn parts(&mut self, pixels: &[u8], palette: &[[u8; 3]], audio: &[i16]) {
        self.bytes(pixels);
        for rgb in palette {
            self.bytes(rgb);
        }
        for s in audio {
            self.bytes(&s.to_le_bytes());
        }
    }

    pub fn value(&self) -> u64 {
        self.state
    }

    /// `0x` plus 16 hex digits.
    pub fn hex(&self) -> String {
        format!("{:#018x}", self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_reference_fnv1a() {
        let mut h = Hasher::new();
        h.bytes(b"");
        assert_eq!(h.value(), 0xcbf29ce484222325);
        let mut h = Hasher::new();
        h.bytes(b"a");
        assert_eq!(h.value(), 0xaf63dc4c8601ec8c);
        let mut h = Hasher::new();
        h.bytes(b"foobar");
        assert_eq!(h.value(), 0x85944171f73967e8);
        assert_eq!(h.hex(), "0x85944171f73967e8");
    }

    #[test]
    fn audio_is_hashed_after_the_palette_little_endian() {
        let mut a = Hasher::new();
        a.parts(&[1], &[[2, 3, 4]], &[0x0102, -1]);
        let mut b = Hasher::new();
        b.bytes(&[1, 2, 3, 4, 0x02, 0x01, 0xff, 0xff]);
        assert_eq!(a.value(), b.value());
        let mut c = Hasher::new();
        c.parts(&[1], &[[2, 3, 4]], &[]);
        assert_ne!(a.value(), c.value());
    }
}
