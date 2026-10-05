//! The pixel buffer the controls and the picture are drawn into.

/// A window buffer: four bytes a pixel, R, G, B, X.
pub struct Canvas<'a> {
    /// The pixels, 4 bytes each: R, G, B, X.
    pub data: &'a mut [u8],
    /// Pixels drawn to in a row.
    pub width: usize,
    /// Rows drawn to.
    pub height: usize,
    /// Pixels a row takes in `data`; at least `width` in a sound buffer.
    pub stride: usize,
}

/// Black, with the unused byte as 255.
pub(crate) const BLACK: [u8; 4] = [0, 0, 0, 255];

impl Canvas<'_> {
    /// The width and height that `data` really holds, whatever `width`,
    /// `height` and `stride` claim: nothing is drawn outside it, so a
    /// buffer that is too small for its own size is drawn to as far as it
    /// goes.
    pub(crate) fn visible(&self) -> (usize, usize) {
        let w = self.width.min(self.stride);
        let pixels = self.data.len() / 4;
        if w == 0 || self.stride == 0 || pixels < w {
            return (0, 0);
        }
        let rows = (pixels - w) / self.stride + 1;
        (w, self.height.min(rows))
    }

    /// The bytes of pixel `x` in row `y`, which must be visible.
    pub(crate) fn offset(&self, x: usize, y: usize) -> usize {
        (y * self.stride + x) * 4
    }
}

/// Write `pixel` into every pixel of `span`.
pub(crate) fn fill(span: &mut [u8], pixel: [u8; 4]) {
    for p in span.as_chunks_mut::<4>().0 {
        *p = pixel;
    }
}
