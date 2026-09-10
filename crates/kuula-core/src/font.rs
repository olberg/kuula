//! The built-in system font: 3x5 glyphs in a 4x6 cell, printable ASCII
//! 32 to 126. Anything else draws a placeholder box. Drawing is in
//! `blit::print`; this module is only the glyph data, kept in the core
//! because the Rust-drawn error screen needs it
//! without a guest.

/// Pixel columns per glyph cell, including the one-column gap.
pub const GLYPH_WIDTH: i32 = 4;
/// Pixel rows per glyph cell, including the one-row gap.
pub const GLYPH_HEIGHT: i32 = 6;

const INK_WIDTH: usize = 3;
const INK_HEIGHT: usize = 5;
const FIRST: usize = 32;
const LAST: usize = 126;
const COUNT: usize = LAST - FIRST + 1;

/// One glyph: five rows, bit 2 is the leftmost pixel.
pub type Glyph = [u8; INK_HEIGHT];

/// Drawn for codepoints outside 32..=126.
pub static PLACEHOLDER: Glyph = parse_glyph("###|#.#|#.#|#.#|###");

/// Glyph for a codepoint, or the placeholder.
pub fn glyph(c: char) -> &'static Glyph {
    let code = c as usize;
    if (FIRST..=LAST).contains(&code) {
        &GLYPHS[code - FIRST]
    } else {
        &PLACEHOLDER
    }
}

const fn parse_glyph(s: &str) -> Glyph {
    let b = s.as_bytes();
    // 5 rows of 3 cells separated by '|'.
    assert!(b.len() == INK_HEIGHT * (INK_WIDTH + 1) - 1);
    let mut out = [0u8; INK_HEIGHT];
    let mut row = 0;
    while row < INK_HEIGHT {
        let mut col = 0;
        while col < INK_WIDTH {
            let ch = b[row * (INK_WIDTH + 1) + col];
            assert!(ch == b'#' || ch == b'.');
            if ch == b'#' {
                out[row] |= 0b100 >> col;
            }
            col += 1;
        }
        row += 1;
    }
    out
}

const fn parse_all(src: &[&str; COUNT]) -> [Glyph; COUNT] {
    let mut out = [[0u8; INK_HEIGHT]; COUNT];
    let mut i = 0;
    while i < COUNT {
        out[i] = parse_glyph(src[i]);
        i += 1;
    }
    out
}

static GLYPHS: [Glyph; COUNT] = parse_all(&GLYPH_ART);

#[rustfmt::skip]
const GLYPH_ART: [&str; COUNT] = [
    "...|...|...|...|...", // 32 space
    ".#.|.#.|.#.|...|.#.", // 33 !
    "#.#|#.#|...|...|...", // 34 "
    "#.#|###|#.#|###|#.#", // 35 #
    ".##|##.|.#.|.##|##.", // 36 $
    "#.#|..#|.#.|#..|#.#", // 37 %
    ".#.|#.#|.#.|#.#|.##", // 38 &
    ".#.|.#.|...|...|...", // 39 '
    ".#.|#..|#..|#..|.#.", // 40 (
    ".#.|..#|..#|..#|.#.", // 41 )
    "#.#|.#.|###|.#.|#.#", // 42 *
    "...|.#.|###|.#.|...", // 43 +
    "...|...|...|.#.|#..", // 44 ,
    "...|...|###|...|...", // 45 -
    "...|...|...|...|.#.", // 46 .
    "..#|..#|.#.|#..|#..", // 47 /
    "###|#.#|#.#|#.#|###", // 48 0
    ".#.|##.|.#.|.#.|###", // 49 1
    "###|..#|###|#..|###", // 50 2
    "###|..#|###|..#|###", // 51 3
    "#.#|#.#|###|..#|..#", // 52 4
    "###|#..|###|..#|###", // 53 5
    "###|#..|###|#.#|###", // 54 6
    "###|..#|..#|..#|..#", // 55 7
    "###|#.#|###|#.#|###", // 56 8
    "###|#.#|###|..#|###", // 57 9
    "...|.#.|...|.#.|...", // 58 :
    "...|.#.|...|.#.|#..", // 59 ;
    "..#|.#.|#..|.#.|..#", // 60 <
    "...|###|...|###|...", // 61 =
    "#..|.#.|..#|.#.|#..", // 62 >
    "###|..#|.##|...|.#.", // 63 ?
    ".#.|#.#|###|#..|.##", // 64 @
    "###|#.#|###|#.#|#.#", // 65 A
    "##.|#.#|##.|#.#|##.", // 66 B
    "###|#..|#..|#..|###", // 67 C
    "##.|#.#|#.#|#.#|##.", // 68 D
    "###|#..|###|#..|###", // 69 E
    "###|#..|###|#..|#..", // 70 F
    "###|#..|#.#|#.#|###", // 71 G
    "#.#|#.#|###|#.#|#.#", // 72 H
    "###|.#.|.#.|.#.|###", // 73 I
    "..#|..#|..#|#.#|###", // 74 J
    "#.#|#.#|##.|#.#|#.#", // 75 K
    "#..|#..|#..|#..|###", // 76 L
    "#.#|###|###|#.#|#.#", // 77 M
    "##.|#.#|#.#|#.#|#.#", // 78 N
    "###|#.#|#.#|#.#|###", // 79 O
    "###|#.#|###|#..|#..", // 80 P
    "###|#.#|#.#|###|..#", // 81 Q
    "###|#.#|##.|#.#|#.#", // 82 R
    "###|#..|###|..#|###", // 83 S
    "###|.#.|.#.|.#.|.#.", // 84 T
    "#.#|#.#|#.#|#.#|###", // 85 U
    "#.#|#.#|#.#|#.#|.#.", // 86 V
    "#.#|#.#|###|###|#.#", // 87 W
    "#.#|#.#|.#.|#.#|#.#", // 88 X
    "#.#|#.#|###|.#.|.#.", // 89 Y
    "###|..#|.#.|#..|###", // 90 Z
    "##.|#..|#..|#..|##.", // 91 [
    "#..|#..|.#.|..#|..#", // 92 backslash
    ".##|..#|..#|..#|.##", // 93 ]
    ".#.|#.#|...|...|...", // 94 ^
    "...|...|...|...|###", // 95 _
    "#..|.#.|...|...|...", // 96 `
    "...|.##|#.#|#.#|.##", // 97 a
    "#..|#..|##.|#.#|##.", // 98 b
    "...|.##|#..|#..|.##", // 99 c
    "..#|..#|.##|#.#|.##", // 100 d
    "...|.#.|#.#|##.|.##", // 101 e
    ".##|#..|##.|#..|#..", // 102 f
    ".##|#.#|.##|..#|##.", // 103 g
    "#..|#..|##.|#.#|#.#", // 104 h
    ".#.|...|.#.|.#.|.#.", // 105 i
    "..#|...|..#|..#|##.", // 106 j
    "#..|#.#|##.|#.#|#.#", // 107 k
    ".#.|.#.|.#.|.#.|..#", // 108 l
    "...|#.#|###|#.#|#.#", // 109 m
    "...|##.|#.#|#.#|#.#", // 110 n
    "...|.#.|#.#|#.#|.#.", // 111 o
    "##.|#.#|##.|#..|#..", // 112 p
    ".##|#.#|.##|..#|..#", // 113 q
    "...|.##|#..|#..|#..", // 114 r
    "...|.##|##.|..#|##.", // 115 s
    ".#.|###|.#.|.#.|..#", // 116 t
    "...|#.#|#.#|#.#|.##", // 117 u
    "...|#.#|#.#|#.#|.#.", // 118 v
    "...|#.#|#.#|###|#.#", // 119 w
    "...|#.#|.#.|.#.|#.#", // 120 x
    "#.#|#.#|.##|..#|##.", // 121 y
    "...|###|.##|#..|###", // 122 z
    ".##|.#.|#..|.#.|.##", // 123 {
    ".#.|.#.|.#.|.#.|.#.", // 124 |
    "##.|.#.|..#|.#.|##.", // 125 }
    "...|.##|##.|...|...", // 126 ~
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_is_the_familiar_shape() {
        assert_eq!(*glyph('A'), [0b111, 0b101, 0b111, 0b101, 0b101]);
        assert_eq!(GLYPH_WIDTH, 4);
        assert_eq!(GLYPH_HEIGHT, 6);
    }

    #[test]
    fn every_printable_ascii_has_a_glyph_and_the_rest_use_the_placeholder() {
        for code in 32u8..=126 {
            let g = glyph(code as char);
            assert!(
                !std::ptr::eq(g, &PLACEHOLDER),
                "codepoint {code} has no glyph"
            );
            if code != b' ' {
                assert!(g.iter().any(|&r| r != 0), "codepoint {code} is blank");
            }
        }
        assert!(std::ptr::eq(glyph(127 as char), &PLACEHOLDER));
        assert!(std::ptr::eq(glyph('\u{e9}'), &PLACEHOLDER));
        assert!(std::ptr::eq(glyph('\u{1f600}'), &PLACEHOLDER));
        assert!(std::ptr::eq(glyph('\n'), &PLACEHOLDER));
    }

    #[test]
    fn glyphs_stay_inside_the_ink_box() {
        for g in GLYPHS.iter() {
            assert!(g.iter().all(|&r| r & !0b111 == 0));
        }
    }
}
