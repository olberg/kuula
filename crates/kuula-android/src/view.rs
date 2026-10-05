//! What the window looks like to the layout: the content rectangle as
//! insets, the density as a scale, and the key that says when the layout
//! must be built again.

use kuula_touch::Insets;

/// Pixels per dp at the baseline density of 160 dpi.
const BASELINE_DPI: f32 = 160.0;

/// The density as pixels per dp. A device that reports none is taken as
/// the baseline; absurd values are held to a sane range.
pub fn density_scale(dpi: Option<u32>) -> f32 {
    let dpi = dpi.unwrap_or(BASELINE_DPI as u32) as f32;
    (dpi / BASELINE_DPI).clamp(0.5, 8.0)
}

/// The insets a window of `window` pixels has when its content rectangle is
/// `content` (left, top, right, bottom, in window pixels): the bars, the
/// cutout, the gesture area. A rectangle that has not been set (empty, as
/// the activity reports before its first layout) means no insets, and no
/// inset is allowed to be more than half the window, so a bad report cannot
/// leave nothing to draw in.
pub fn insets_from_content(window: (i32, i32), content: (i32, i32, i32, i32)) -> Insets {
    let (w, h) = window;
    let (left, top, right, bottom) = content;
    if right <= left || bottom <= top || w <= 0 || h <= 0 {
        return Insets::default();
    }
    Insets {
        left: left.clamp(0, w / 2),
        top: top.clamp(0, h / 2),
        right: (w - right).clamp(0, w / 2),
        bottom: (h - bottom).clamp(0, h / 2),
    }
}

/// The larger of two sets of insets on each side: what the content
/// rectangle says and what the display's cutout leaves clear.
pub fn widest(a: Insets, cutout: [i32; 4], window: (i32, i32)) -> Insets {
    let (w, h) = window;
    Insets {
        left: a.left.max(cutout[0]).clamp(0, (w / 2).max(0)),
        top: a.top.max(cutout[1]).clamp(0, (h / 2).max(0)),
        right: a.right.max(cutout[2]).clamp(0, (w / 2).max(0)),
        bottom: a.bottom.max(cutout[3]).clamp(0, (h / 2).max(0)),
    }
}

/// What a layout was built from. When any of it changes the layout is built
/// again, and the fingers that were held belonged to the old one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutKey {
    pub window: (i32, i32),
    pub insets: Insets,
    pub density: f32,
    /// The console's frame size.
    pub picture: (u32, u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cutout_widens_an_inset_and_never_takes_half_the_window() {
        let none = Insets::default();
        let got = widest(none, [0, 120, 0, 0], (1080, 2392));
        assert_eq!((got.left, got.top, got.right, got.bottom), (0, 120, 0, 0));
        let bars = Insets {
            left: 10,
            top: 200,
            right: 0,
            bottom: 48,
        };
        let got = widest(bars, [90, 120, 0, 0], (2392, 1080));
        assert_eq!((got.left, got.top, got.right, got.bottom), (90, 200, 0, 48));
        let got = widest(none, [5000, 5000, 0, 0], (1000, 600));
        assert_eq!((got.left, got.top), (500, 300));
    }

    #[test]
    fn density_is_dpi_over_160_and_sane() {
        assert_eq!(density_scale(Some(160)), 1.0);
        assert_eq!(density_scale(Some(420)), 2.625);
        assert_eq!(density_scale(None), 1.0);
        assert_eq!(density_scale(Some(0)), 0.5);
        assert_eq!(density_scale(Some(100_000)), 8.0);
    }

    #[test]
    fn insets_are_what_the_content_rectangle_leaves_out() {
        let i = insets_from_content((2400, 1080), (96, 0, 2304, 1080));
        assert_eq!(
            i,
            Insets {
                left: 96,
                top: 0,
                right: 96,
                bottom: 0
            }
        );
        // A portrait window with a status bar above and a gesture bar below.
        let i = insets_from_content((1080, 2400), (0, 80, 1080, 2300));
        assert_eq!((i.top, i.bottom, i.left, i.right), (80, 100, 0, 0));
    }

    #[test]
    fn an_unset_or_absurd_rectangle_leaves_the_window_whole_or_half() {
        assert_eq!(
            insets_from_content((800, 480), (0, 0, 0, 0)),
            Insets::default()
        );
        assert_eq!(
            insets_from_content((0, 0), (0, 0, 10, 10)),
            Insets::default()
        );
        let i = insets_from_content((800, 480), (700, 400, 790, 470));
        assert_eq!((i.left, i.top), (400, 240));
    }
}
