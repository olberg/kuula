use kuula_core::PALETTE_SIZE;
use kuula_touch::{present, Canvas, Insets, Layout};

/// Every index has its own colour, and index 0 is not black.
fn palette() -> [[u8; 3]; PALETTE_SIZE] {
    let mut p = [[0u8; 3]; PALETTE_SIZE];
    for (i, c) in p.iter_mut().enumerate() {
        let i = i as u8;
        *c = [10 + i, 20u8.wrapping_add(2 * i), 200 - i];
    }
    p
}

fn colour(index: u8) -> [u8; 4] {
    let c = palette()[index as usize];
    [c[0], c[1], c[2], 255]
}

const BLACK: [u8; 4] = [0, 0, 0, 255];
const PAD: u8 = 0x55;

/// A 4 by 2 frame.
const FRAME: [u8; 8] = [1, 2, 3, 4, 4, 3, 2, 1];

/// What the canvas should hold for `FRAME` at `scale` with the picture's
/// corner at (`x`, `y`) on a canvas of `w` by `h`.
fn expected(w: usize, h: usize, scale: usize, x: usize, y: usize) -> Vec<[u8; 4]> {
    let mut out = vec![BLACK; w * h];
    for py in 0..h {
        for px in 0..w {
            if px >= x && py >= y && px < x + 4 * scale && py < y + 2 * scale {
                let index = FRAME[(py - y) / scale * 4 + (px - x) / scale];
                out[py * w + px] = colour(index);
            }
        }
    }
    out
}

fn pixels(data: &[u8], w: usize, stride: usize, h: usize) -> Vec<[u8; 4]> {
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let at = (y * stride + x) * 4;
            out.push([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        }
    }
    out
}

/// Present `FRAME` on a window of `w` by `h` with rows `stride` pixels
/// long, the buffer pre-filled with a pattern that is not black.
fn render(w: usize, h: usize, stride: usize) -> (Layout, Vec<u8>) {
    let layout = Layout::new((w as i32, h as i32), Insets::default(), 1.0, (4, 2));
    let mut data = vec![PAD; stride * h * 4];
    let mut canvas = Canvas {
        data: &mut data,
        width: w,
        height: h,
        stride,
    };
    present(&mut canvas, &layout, &FRAME, &palette(), 4, 2);
    (layout, data)
}

#[test]
fn a_tiny_frame_at_scale_two_lands_exactly() {
    // 9 by 5: the picture is 8 by 4 and the rest is black.
    let (layout, data) = render(9, 5, 12);
    assert_eq!(layout.scale(), 2);
    let p = layout.picture();
    assert_eq!((p.x, p.y, p.w, p.h), (0, 0, 8, 4));
    assert_eq!(pixels(&data, 9, 12, 5), expected(9, 5, 2, 0, 0));
    // Straight from the byte values for the first row: index 1, 1, 2, 2.
    let c = palette();
    let row = &data[..9 * 4];
    assert_eq!(&row[..4], &[c[1][0], c[1][1], c[1][2], 255]);
    assert_eq!(&row[4..8], &row[..4]);
    assert_eq!(&row[8..12], &[c[2][0], c[2][1], c[2][2], 255]);
    assert_eq!(&row[32..36], &BLACK);
    // The padding of a row, past the width, is left alone.
    assert!(data[9 * 4..12 * 4].iter().all(|&b| b == PAD));
}

#[test]
fn a_tiny_frame_at_scale_three_lands_exactly() {
    let (layout, data) = render(13, 7, 20);
    assert_eq!(layout.scale(), 3);
    assert_eq!(pixels(&data, 13, 20, 7), expected(13, 7, 3, 0, 0));
    for y in 0..7 {
        assert!(data[(y * 20 + 13) * 4..(y * 20 + 20) * 4]
            .iter()
            .all(|&b| b == PAD));
    }
}

#[test]
fn a_frame_at_one_and_a_half_is_three_pixels_for_every_two() {
    // A window of 6 by 3 holds the 4 by 2 frame at one and a half and not
    // at 2: of each pair of source pixels the first is two wide and the
    // second one, and the same down the rows.
    let layout = Layout::new((6, 3), Insets::default(), 1.0, (4, 2));
    assert_eq!(layout.scale_halves(), 3);
    let p = layout.picture();
    assert_eq!((p.x, p.y, p.w, p.h), (0, 0, 6, 3));
    let stride = 8;
    let mut data = vec![PAD; stride * 3 * 4];
    let mut canvas = Canvas {
        data: &mut data,
        width: 6,
        height: 3,
        stride,
    };
    present(&mut canvas, &layout, &FRAME, &palette(), 4, 2);
    let top = [1u8, 1, 2, 3, 3, 4].map(colour);
    let bottom = [4u8, 4, 3, 2, 2, 1].map(colour);
    let want: Vec<[u8; 4]> = [top, top, bottom].concat();
    assert_eq!(pixels(&data, 6, stride, 3), want);

    // A canvas that cuts the picture anywhere is a crop of the same.
    for (cw, ch) in [(5, 3), (4, 2), (2, 1), (1, 3)] {
        let mut small = vec![PAD; cw * ch * 4];
        let mut canvas = Canvas {
            data: &mut small,
            width: cw,
            height: ch,
            stride: cw,
        };
        present(&mut canvas, &layout, &FRAME, &palette(), 4, 2);
        let got = pixels(&small, cw, cw, ch);
        for y in 0..ch {
            assert_eq!(
                got[y * cw..(y + 1) * cw],
                want[y * 6..y * 6 + cw],
                "{cw}x{ch} row {y}"
            );
        }
    }
}

#[test]
fn the_picture_is_centred_and_the_rest_is_black() {
    // 23 by 12: 5 across and 6 down, so scale 5, 20 by 10.
    let (layout, data) = render(23, 12, 23);
    let p = layout.picture();
    assert_eq!(layout.scale(), 5);
    assert_eq!((p.x, p.y), (1, 1));
    assert_eq!(
        pixels(&data, 23, 23, 12),
        expected(23, 12, 5, p.x as usize, p.y as usize)
    );
}

#[test]
fn a_picture_with_a_margin_each_side_is_surrounded_by_black() {
    let layout = Layout::new((30, 13), Insets::default(), 1.0, (4, 2));
    let mut data = vec![PAD; 30 * 13 * 4];
    let mut canvas = Canvas {
        data: &mut data,
        width: 30,
        height: 13,
        stride: 30,
    };
    present(&mut canvas, &layout, &FRAME, &palette(), 4, 2);
    let p = layout.picture();
    assert_eq!(layout.scale(), 6);
    assert_eq!((p.x, p.y, p.w, p.h), (3, 0, 24, 12));
    let got = pixels(&data, 30, 30, 13);
    assert_eq!(got, expected(30, 13, 6, 3, 0));
    assert!(got[..3].iter().all(|&px| px == BLACK));
    assert!(got[30 * 12..].iter().all(|&px| px == BLACK));
}

#[test]
fn a_frame_that_does_not_match_the_layout_draws_what_fits() {
    let layout = Layout::new((9, 5), Insets::default(), 1.0, (4, 2));
    let c = palette();
    let draw = |pixels: &[u8], w: u32, h: u32| {
        let mut data = vec![PAD; 9 * 5 * 4];
        let mut canvas = Canvas {
            data: &mut data,
            width: 9,
            height: 5,
            stride: 9,
        };
        present(&mut canvas, &layout, pixels, &c, w, h);
        pixels_of(&data)
    };
    fn pixels_of(data: &[u8]) -> Vec<[u8; 4]> {
        pixels(data, 9, 9, 5)
    }
    let all = expected(9, 5, 2, 0, 0);

    // Narrower and shorter than the layout: what there is, and black.
    let got = draw(&[1, 2, 4, 3], 2, 2);
    assert_eq!(got[0], colour(1));
    assert_eq!(got[2], colour(2));
    assert_eq!(got[4], BLACK, "past the frame's width");
    assert_eq!(got[2 * 9], colour(4), "the second row");
    assert_eq!(got[4 * 9], BLACK, "past the frame's height");

    // Wider and taller than the layout: the layout's picture only.
    let got = draw(&[1; 6 * 4], 6, 4);
    assert!(got[..8].iter().all(|&px| px == colour(1)));
    assert_eq!(got[8], BLACK, "the picture is 8 wide");
    assert!(got.chunks(9).take(4).all(|r| r[..8] == [colour(1); 8]));
    assert!(got[4 * 9..].iter().all(|&px| px == BLACK));

    // Too few pixels for the size claimed.
    let got = draw(&FRAME[..5], 4, 2);
    assert_eq!(got[..2], all[..2]);
    assert_eq!(got[2 * 9..2 * 9 + 2], [colour(4), colour(4)], "the 5th");
    assert_eq!(got[2 * 9 + 2], BLACK, "past the 5 pixels");
    assert_eq!(got[4 * 9], BLACK);
    // No pixels at all, and a zero size.
    assert!(draw(&[], 4, 2).iter().all(|&px| px == BLACK));
    assert!(draw(&FRAME, 0, 0).iter().all(|&px| px == BLACK));
    assert!(draw(&FRAME, 4, 2) == all);
}

#[test]
fn indexes_beyond_the_palette_are_black() {
    let layout = Layout::new((4, 2), Insets::default(), 1.0, (4, 2));
    let mut data = vec![PAD; 4 * 2 * 4];
    let mut canvas = Canvas {
        data: &mut data,
        width: 4,
        height: 2,
        stride: 4,
    };
    present(
        &mut canvas,
        &layout,
        &[1, 127, 128, 255, 0, 0, 0, 0],
        &palette(),
        4,
        2,
    );
    let got = pixels(&data, 4, 4, 2);
    assert_eq!(got[..4], [colour(1), colour(127), BLACK, BLACK]);
}

/// A canvas smaller than the layout, or one that starts left of the
/// picture, is the same picture cut off.
#[test]
fn a_smaller_canvas_is_a_crop_of_the_full_picture() {
    let (w, h) = (23, 12);
    let layout = Layout::new((w as i32, h as i32), Insets::default(), 1.0, (4, 2));
    let (_, full) = render(w, h, w);
    let full = pixels(&full, w, w, h);
    for cw in 1..=w {
        for ch in 1..=h {
            let mut data = vec![PAD; cw * ch * 4];
            let mut canvas = Canvas {
                data: &mut data,
                width: cw,
                height: ch,
                stride: cw,
            };
            present(&mut canvas, &layout, &FRAME, &palette(), 4, 2);
            let got = pixels(&data, cw, cw, ch);
            for y in 0..ch {
                for x in 0..cw {
                    assert_eq!(got[y * cw + x], full[y * w + x], "{cw}x{ch} at {x},{y}");
                }
            }
        }
    }
}

#[test]
fn a_picture_hanging_over_the_canvas_edge_is_cut_there() {
    // A window of 2 by 1 under a 4 by 2 picture: scale 1, the picture's
    // left edge at -1.
    let layout = Layout::new((2, 1), Insets::default(), 1.0, (4, 2));
    assert_eq!((layout.picture().x, layout.picture().y), (-1, 0));
    let mut data = vec![PAD; 2 * 4];
    let mut canvas = Canvas {
        data: &mut data,
        width: 2,
        height: 1,
        stride: 2,
    };
    present(&mut canvas, &layout, &FRAME, &palette(), 4, 2);
    assert_eq!(pixels(&data, 2, 2, 1), [colour(2), colour(3)]);
}

#[test]
fn a_canvas_whose_buffer_is_too_short_does_not_panic() {
    let layout = Layout::new((9, 5), Insets::default(), 1.0, (4, 2));
    for len in [0, 1, 3, 4, 35, 36, 37, 100, 9 * 5 * 4 - 1] {
        let mut data = vec![PAD; len];
        let mut canvas = Canvas {
            data: &mut data,
            width: 9,
            height: 5,
            stride: 9,
        };
        present(&mut canvas, &layout, &FRAME, &palette(), 4, 2);
    }
    // A stride shorter than the width, and a zero one.
    for stride in [0, 1, 5] {
        let mut data = vec![PAD; 9 * 5 * 4];
        let mut canvas = Canvas {
            data: &mut data,
            width: 9,
            height: 5,
            stride,
        };
        present(&mut canvas, &layout, &FRAME, &palette(), 4, 2);
    }
}
