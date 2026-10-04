//! The system fonts: Unscii 8x8 and 8x16, embedded from
//! `fonts/*.hex` and parsed once on first use. Drawing is in
//! `blit::print`; this module is only the glyph data, kept in the core
//! because the Rust-drawn error screen needs it without a guest.
//!
//! A codepoint neither face carries draws a hollow box.

use std::sync::OnceLock;

/// Pixel columns of an ordinary glyph in either face. A few glyphs of the
/// large face are twice as wide; [`Glyph::width`] says so.
pub const CELL_WIDTH: i32 = 8;

/// Which system face `print` draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontId {
    /// unscii-8: 8x8, the default at 320x240.
    Small,
    /// unscii-16: 8x16, the default at 640x480.
    Large,
}

impl FontId {
    /// The face a screen of this height starts with: the small one below
    /// 480 rows, the large one from there, so text keeps the same 30
    /// rows in both screen modes.
    pub fn for_screen_height(height: u32) -> FontId {
        if height >= 480 {
            FontId::Large
        } else {
            FontId::Small
        }
    }

    /// The face whose glyphs are `height` pixels tall, if there is one.
    pub fn from_height(height: i64) -> Option<FontId> {
        match height {
            8 => Some(FontId::Small),
            16 => Some(FontId::Large),
            _ => None,
        }
    }

    pub fn height(self) -> i32 {
        match self {
            FontId::Small => 8,
            FontId::Large => 16,
        }
    }

    /// The parsed face, shared by every console in the process.
    pub fn font(self) -> &'static Font {
        static SMALL: OnceLock<Font> = OnceLock::new();
        static LARGE: OnceLock<Font> = OnceLock::new();
        match self {
            FontId::Small => SMALL.get_or_init(|| Font::parse(SMALL_HEX, 8)),
            FontId::Large => LARGE.get_or_init(|| Font::parse(LARGE_HEX, 16)),
        }
    }
}

const SMALL_HEX: &str = include_str!("../fonts/unscii-8.hex");
const LARGE_HEX: &str = include_str!("../fonts/unscii-16.hex");

/// One glyph: `height` rows of `width / 8` bytes each, the most
/// significant bit leftmost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph<'a> {
    pub width: i32,
    pub height: i32,
    rows: &'a [u8],
}

impl Glyph<'_> {
    /// Whether the pixel at `(x, y)` inside the glyph is ink.
    #[inline]
    pub fn pixel(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return false;
        }
        let stride = (self.width / 8) as usize;
        let byte = self.rows[y as usize * stride + (x / 8) as usize];
        byte & (0x80 >> (x % 8)) != 0
    }

    /// The row bytes, `height * width / 8` of them.
    pub fn rows(&self) -> &[u8] {
        self.rows
    }
}

/// One face: every glyph's rows in one slab, indexed by a sorted
/// codepoint table.
#[derive(Debug)]
pub struct Font {
    height: i32,
    /// Sorted, parallel to `entries`.
    codes: Vec<u32>,
    /// Offset into `bits` and the glyph's width.
    entries: Vec<(u32, u8)>,
    bits: Vec<u8>,
    /// The hollow box for codepoints the face lacks.
    placeholder: Vec<u8>,
}

impl Font {
    /// Parse a Unifont-style hex file: `CODE:ROWS` per line, the rows a
    /// run of hex digits, two per byte, `height` rows with as many bytes
    /// per row as the glyph's width needs. Lines that do not fit that
    /// shape are skipped, so a hand-edited file degrades to missing
    /// glyphs rather than a panic; the tests check the bundled files
    /// lose nothing.
    pub fn parse(src: &str, height: i32) -> Font {
        let mut glyphs: Vec<(u32, u8, Vec<u8>)> = Vec::new();
        for line in src.lines() {
            let Some((code, rows)) = line.trim_end().split_once(':') else {
                continue;
            };
            let Ok(code) = u32::from_str_radix(code, 16) else {
                continue;
            };
            if code > char::MAX as u32 || rows.len() % 2 != 0 {
                continue;
            }
            let bytes = rows.len() / 2;
            if bytes % height as usize != 0 {
                continue;
            }
            let width = bytes / height as usize * 8;
            if width != 8 && width != 16 {
                continue;
            }
            let Ok(bits) = (0..bytes)
                .map(|i| u8::from_str_radix(&rows[2 * i..2 * i + 2], 16))
                .collect::<Result<Vec<u8>, _>>()
            else {
                continue;
            };
            glyphs.push((code, width as u8, bits));
        }
        glyphs.sort_by_key(|g| g.0);
        glyphs.dedup_by_key(|g| g.0);
        let mut codes = Vec::with_capacity(glyphs.len());
        let mut entries = Vec::with_capacity(glyphs.len());
        let mut bits = Vec::new();
        for (code, width, rows) in glyphs {
            codes.push(code);
            entries.push((bits.len() as u32, width));
            bits.extend_from_slice(&rows);
        }
        let mut placeholder = vec![0x7e; height as usize];
        for row in placeholder.iter_mut().take(height as usize - 1).skip(1) {
            *row = 0x42;
        }
        Font {
            height,
            codes,
            entries,
            bits,
            placeholder,
        }
    }

    /// Glyph height in pixels; the line advance.
    pub fn height(&self) -> i32 {
        self.height
    }

    /// Codepoints the face carries.
    pub fn len(&self) -> usize {
        self.codes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.codes.is_empty()
    }

    pub fn has(&self, c: char) -> bool {
        self.codes.binary_search(&(c as u32)).is_ok()
    }

    /// The glyph for a codepoint, or the placeholder box.
    pub fn glyph(&self, c: char) -> Glyph<'_> {
        match self.codes.binary_search(&(c as u32)) {
            Ok(i) => {
                let (offset, width) = self.entries[i];
                let len = self.height as usize * (width as usize / 8);
                Glyph {
                    width: width as i32,
                    height: self.height,
                    rows: &self.bits[offset as usize..offset as usize + len],
                }
            }
            Err(_) => Glyph {
                width: CELL_WIDTH,
                height: self.height,
                rows: &self.placeholder,
            },
        }
    }

    /// Pixels a string advances the pen by.
    pub fn width_of(&self, text: &str) -> i32 {
        text.chars()
            .fold(0i32, |w, c| w.saturating_add(self.glyph(c).width))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ascii(g: &Glyph<'_>) -> Vec<String> {
        (0..g.height)
            .map(|y| {
                (0..g.width)
                    .map(|x| if g.pixel(x, y) { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_bundled_files_parse_whole() {
        let small = FontId::Small.font();
        let large = FontId::Large.font();
        assert_eq!(small.len(), SMALL_HEX.lines().count());
        assert_eq!(large.len(), LARGE_HEX.lines().count());
        assert_eq!(small.len(), 3191);
        assert_eq!(large.len(), 3240);
        assert_eq!(small.height(), 8);
        assert_eq!(large.height(), 16);
    }

    #[test]
    fn a_is_the_familiar_shape_in_both_faces() {
        let a = FontId::Small.font().glyph('A');
        assert_eq!(
            ascii(&a),
            [
                "...##...", "..####..", ".##..##.", ".##..##.", ".######.", ".##..##.", ".##..##.",
                "........",
            ]
        );
        assert_eq!(a.rows(), [0x18, 0x3c, 0x66, 0x66, 0x7e, 0x66, 0x66, 0x00]);
        let a = FontId::Large.font().glyph('A');
        assert_eq!((a.width, a.height), (8, 16));
        assert_eq!(ascii(&a)[4], ".##..##.");
        assert!(ascii(&a)[15].chars().all(|c| c == '.'));
    }

    #[test]
    fn every_printable_ascii_and_latin1_has_a_glyph() {
        for id in [FontId::Small, FontId::Large] {
            let f = id.font();
            for code in 32u32..=255 {
                let c = char::from_u32(code).unwrap();
                assert!(f.has(c), "{id:?} lacks U+{code:04X}");
                if c != ' ' && c != '\u{a0}' && c != '\u{ad}' {
                    let g = f.glyph(c);
                    assert!(
                        g.rows().iter().any(|&r| r != 0),
                        "{id:?} U+{code:04X} is blank"
                    );
                }
            }
            assert!(f.has('ä') && f.has('ö') && f.has('å'));
            assert!(f.has('█') && f.has('┌'), "box drawing");
            assert!(f.has('\u{1fb00}'), "legacy computing block");
        }
    }

    #[test]
    fn missing_codepoints_draw_the_box() {
        let f = FontId::Small.font();
        assert!(!f.has('\u{1f600}'));
        let g = f.glyph('\u{1f600}');
        assert_eq!(
            ascii(&g),
            [
                ".######.", ".#....#.", ".#....#.", ".#....#.", ".#....#.", ".#....#.", ".#....#.",
                ".######.",
            ]
        );
        // Unscii gives the C0 controls their own pictures, not boxes.
        assert!(f.has('\n'));
        let g = FontId::Large.font().glyph('\u{1f600}');
        assert_eq!(ascii(&g)[0], ".######.");
        assert_eq!(ascii(&g)[8], ".#....#.");
        assert_eq!(ascii(&g)[15], ".######.");
    }

    #[test]
    fn the_large_face_has_double_width_glyphs_and_widths_add_up() {
        let large = FontId::Large.font();
        let wide = large.glyph('\u{23e9}');
        assert_eq!((wide.width, wide.height), (16, 16));
        assert_eq!(wide.rows().len(), 32);
        assert!(wide.pixel(9, 8) || wide.pixel(8, 8) || wide.pixel(10, 8));
        assert_eq!(large.width_of("A\u{23e9}"), 24);
        assert_eq!(FontId::Small.font().width_of("hello"), 40);
        assert_eq!(FontId::Small.font().width_of(""), 0);
        // Out-of-range pixels are not ink.
        assert!(!wide.pixel(-1, 0) && !wide.pixel(16, 0) && !wide.pixel(0, 16));
    }

    #[test]
    fn parse_skips_malformed_lines() {
        let f = Font::parse(
            "0041:183C66667E666600\nnot a line\n0042:12\n0043:ZZ3C66667E666600\n",
            8,
        );
        assert_eq!(f.len(), 1);
        assert!(f.has('A') && !f.has('B') && !f.has('C'));
        let empty = Font::parse("", 16);
        assert!(empty.is_empty());
        assert_eq!(empty.glyph('A').height, 16);
    }

    #[test]
    fn face_defaults_follow_the_screen_mode() {
        assert_eq!(FontId::for_screen_height(240), FontId::Small);
        assert_eq!(FontId::for_screen_height(480), FontId::Large);
        assert_eq!(FontId::for_screen_height(0), FontId::Small);
        assert_eq!(FontId::from_height(8), Some(FontId::Small));
        assert_eq!(FontId::from_height(16), Some(FontId::Large));
        assert_eq!(FontId::from_height(12), None);
        assert_eq!(FontId::Large.height(), 16);
    }
}
