//! Indexed screen to RGB24 or ARGB8888 through the palette.

use kuula_core::PALETTE_SIZE;

/// Convert one run of palette indices into packed RGB bytes. `out` must be
/// exactly three times `indexed`.
pub fn row_to_rgb(indexed: &[u8], palette: &[[u8; 3]; PALETTE_SIZE], out: &mut [u8]) {
    debug_assert_eq!(out.len(), indexed.len() * 3);
    for (px, dst) in indexed.iter().zip(out.as_chunks_mut::<3>().0) {
        dst.copy_from_slice(&palette[(px & 0x7f) as usize]);
    }
}

/// Convert a whole screen into a texture buffer with a row `pitch` in
/// bytes, which may be wider than `width * 3`.
pub fn blit_rows(
    indexed: &[u8],
    palette: &[[u8; 3]; PALETTE_SIZE],
    width: usize,
    out: &mut [u8],
    pitch: usize,
) {
    for (src, dst) in indexed.chunks_exact(width).zip(out.chunks_exact_mut(pitch)) {
        row_to_rgb(src, palette, &mut dst[..width * 3]);
    }
}

/// Like [`blit_rows`] into ARGB8888 texels (`0xAARRGGBB` in native byte
/// order, alpha opaque), the format the handheld renderer takes; `pitch`
/// is in bytes.
pub fn blit_rows_argb(
    indexed: &[u8],
    palette: &[[u8; 3]; PALETTE_SIZE],
    width: usize,
    out: &mut [u8],
    pitch: usize,
) {
    for (src, dst) in indexed.chunks_exact(width).zip(out.chunks_exact_mut(pitch)) {
        for (px, texel) in src.iter().zip(dst[..width * 4].as_chunks_mut::<4>().0) {
            let [r, g, b] = palette[(px & 0x7f) as usize];
            let argb = 0xff00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32;
            *texel = argb.to_ne_bytes();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> [[u8; 3]; PALETTE_SIZE] {
        let mut p = [[0u8; 3]; PALETTE_SIZE];
        for (i, e) in p.iter_mut().enumerate() {
            *e = [i as u8, 200, 255 - i as u8];
        }
        p
    }

    #[test]
    fn known_row_produces_expected_rgb_bytes() {
        let mut out = [0u8; 12];
        row_to_rgb(&[0, 1, 127, 0x81], &palette(), &mut out);
        assert_eq!(out, [0, 200, 255, 1, 200, 254, 127, 200, 128, 1, 200, 254]);
    }

    #[test]
    fn argb_texels_are_opaque_and_respect_pitch() {
        let mut out = [0xEEu8; 2 * 12];
        blit_rows_argb(&[1, 2, 3, 0x84], &palette(), 2, &mut out, 12);
        let texel = |i: usize| u32::from_ne_bytes(out[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!(texel(0), 0xff01_c8fe);
        assert_eq!(texel(1), 0xff02_c8fd);
        assert_eq!(&out[8..12], &[0xEE; 4], "padding untouched");
        assert_eq!(texel(3), 0xff03_c8fc);
        assert_eq!(texel(4), 0xff04_c8fb);
    }

    #[test]
    fn blit_respects_pitch() {
        let mut out = [0xEEu8; 2 * 8];
        blit_rows(&[1, 2, 3, 4], &palette(), 2, &mut out, 8);
        assert_eq!(&out[..6], &[1, 200, 254, 2, 200, 253]);
        assert_eq!(&out[6..8], &[0xEE, 0xEE], "padding untouched");
        assert_eq!(&out[8..14], &[3, 200, 252, 4, 200, 251]);
    }
}
