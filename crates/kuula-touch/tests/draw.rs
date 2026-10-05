use kuula_core::input::{
    BTN_A, BTN_B, BTN_DOWN, BTN_LEFT, BTN_MENU, BTN_RIGHT, BTN_UP, CART_BUTTONS,
};
use kuula_touch::{draw_controls, Canvas, Insets, Layout, Rect, OVER_PICTURE};

const GREY: u8 = 100;

fn grey_canvas(w: usize, h: usize) -> Vec<u8> {
    let mut data = vec![GREY; w * h * 4];
    for px in data.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    data
}

fn drawn(layout: &Layout, w: usize, h: usize, pressed: u16) -> Vec<u8> {
    let mut data = grey_canvas(w, h);
    let mut canvas = Canvas {
        data: &mut data,
        width: w,
        height: h,
        stride: w,
    };
    draw_controls(&mut canvas, layout, pressed);
    data
}

fn px(data: &[u8], w: usize, x: i32, y: i32) -> [u8; 4] {
    let at = (y as usize * w + x as usize) * 4;
    [data[at], data[at + 1], data[at + 2], data[at + 3]]
}

fn brightness(p: [u8; 4]) -> u32 {
    p[0] as u32 + p[1] as u32 + p[2] as u32
}

const IDLE_GREY: [u8; 4] = [GREY, GREY, GREY, 255];

#[test]
fn pixels_inside_each_control_change_and_far_from_all_of_them_do_not() {
    for window in [(1280, 720), (720, 1280)] {
        let (w, h) = (window.0 as usize, window.1 as usize);
        let layout = Layout::new(window, Insets::default(), 2.0, (320, 240));
        let data = drawn(&layout, w, h, 0);
        let pad = layout.dpad();
        // The pad's cross, and its disc beside the cross.
        assert_ne!(px(&data, w, pad.cx, pad.cy), IDLE_GREY);
        assert_ne!(px(&data, w, pad.cx, pad.cy - pad.r * 6 / 10), IDLE_GREY);
        assert_ne!(
            px(&data, w, pad.cx + pad.r * 6 / 10, pad.cy + pad.r * 6 / 10),
            IDLE_GREY
        );
        // The buttons: the disc off the letter, and the letter.
        for disc in [layout.button_a(), layout.button_b()] {
            assert_ne!(px(&data, w, disc.cx + disc.r * 8 / 10, disc.cy), IDLE_GREY);
            assert_ne!(px(&data, w, disc.cx, disc.cy), IDLE_GREY);
        }
        let m = layout.menu();
        assert_ne!(px(&data, w, m.x + m.w / 2, m.y + m.h / 2), IDLE_GREY);
        assert_ne!(
            px(&data, w, m.x + m.h / 2, m.y + 2),
            IDLE_GREY,
            "the pill's end"
        );

        // Nothing else moves: not a pixel outside the controls' squares.
        let squares: Vec<Rect> = [
            layout.dpad().bounds(),
            layout.button_a().bounds(),
            layout.button_b().bounds(),
            layout.menu(),
        ]
        .to_vec();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let near = squares
                    .iter()
                    .any(|r| x >= r.x && x < r.right() && y >= r.y && y < r.bottom());
                if !near {
                    assert_eq!(px(&data, w, x, y), IDLE_GREY, "{x},{y} in {window:?}");
                }
            }
        }
        // And the corners of the squares are not touched by the discs.
        let a = layout.button_a().bounds();
        assert_eq!(px(&data, w, a.x, a.y), IDLE_GREY);
    }
}

#[test]
fn a_held_button_is_brighter_than_the_same_button_idle() {
    let (w, h) = (1280usize, 720usize);
    let layout = Layout::new((1280, 720), Insets::default(), 2.0, (320, 240));
    let idle = drawn(&layout, w, h, 0);
    let a = layout.button_a();
    let b = layout.button_b();
    let held = drawn(&layout, w, h, BTN_A);
    let at = |data: &[u8], d: kuula_touch::Disc| px(data, w, d.cx + d.r * 8 / 10, d.cy);
    assert!(brightness(at(&held, a)) > brightness(at(&idle, a)) + 90);
    assert_eq!(at(&held, b), at(&idle, b), "B did not change");
    // The letter is still darker than the disc it is on.
    let letter = px(&held, w, a.cx, a.cy);
    assert!(brightness(letter) < brightness(at(&held, a)) - 200);

    let held = drawn(&layout, w, h, BTN_B);
    assert!(brightness(at(&held, b)) > brightness(at(&idle, b)) + 90);
    assert_eq!(at(&held, a), at(&idle, a));

    let m = layout.menu();
    let held = drawn(&layout, w, h, BTN_MENU);
    let end = |data: &[u8]| px(data, w, m.x + m.h / 2, m.y + 3);
    assert!(brightness(end(&held)) > brightness(end(&idle)) + 90);
}

#[test]
fn a_held_direction_lights_its_arm_only() {
    let (w, h) = (1280usize, 720usize);
    let layout = Layout::new((1280, 720), Insets::default(), 2.0, (320, 240));
    let idle = drawn(&layout, w, h, 0);
    let pad = layout.dpad();
    let r = pad.r;
    let arm = |data: &[u8], dx: i32, dy: i32| {
        px(data, w, pad.cx + dx * r * 6 / 10, pad.cy + dy * r * 6 / 10)
    };
    let arms = [
        (BTN_UP, 0, -1),
        (BTN_DOWN, 0, 1),
        (BTN_LEFT, -1, 0),
        (BTN_RIGHT, 1, 0),
    ];
    for (bit, _, _) in arms {
        let held = drawn(&layout, w, h, bit);
        for (other, ox, oy) in arms {
            let (a, b) = (arm(&held, ox, oy), arm(&idle, ox, oy));
            if other == bit {
                assert!(brightness(a) > brightness(b) + 90, "{bit} lit");
            } else {
                assert_eq!(a, b, "{other} stays while {bit} is held");
            }
        }
    }
    // Two directions at once, a diagonal.
    let held = drawn(&layout, w, h, BTN_UP | BTN_RIGHT);
    assert!(brightness(arm(&held, 0, -1)) > brightness(arm(&idle, 0, -1)));
    assert!(brightness(arm(&held, 1, 0)) > brightness(arm(&idle, 1, 0)));
    assert_eq!(arm(&held, -1, 0), arm(&idle, -1, 0));
}

#[test]
fn the_alpha_is_about_a_half_when_idle() {
    let layout = Layout::new((1280, 720), Insets::default(), 2.0, (320, 240));
    let data = drawn(&layout, 1280, 720, 0);
    // The light disc of A over grey 100: 236 at 115/255 is about 160, and
    // the unused byte stays 255.
    let d = layout.button_a();
    let p = px(&data, 1280, d.cx + d.r * 8 / 10, d.cy);
    assert!((158..=162).contains(&p[0]), "{p:?}");
    assert_eq!(p[3], 255);
}

#[test]
fn a_canvas_smaller_than_the_layout_does_not_panic() {
    let layout = Layout::new((1280, 720), Insets::default(), 2.0, (320, 240));
    for (w, h) in [
        (0, 0),
        (1, 1),
        (100, 50),
        (300, 700),
        (700, 100),
        (1279, 719),
    ] {
        let data = drawn(&layout, w, h, 0xff);
        assert_eq!(data.len(), w * h * 4);
    }
    // A cut-off canvas is a crop of the whole drawing.
    let full = drawn(&layout, 1280, 720, BTN_A);
    let (w, h) = (400usize, 500usize);
    let part = drawn(&layout, w, h, BTN_A);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            assert_eq!(px(&part, w, x, y), px(&full, 1280, x, y), "{x},{y}");
        }
    }
    // A buffer shorter than it says, and strides of every kind.
    for len in [0, 3, 100, 5000, 1280 * 720 * 4 - 1] {
        let mut data = vec![GREY; len];
        let mut canvas = Canvas {
            data: &mut data,
            width: 1280,
            height: 720,
            stride: 1280,
        };
        draw_controls(&mut canvas, &layout, 0xff);
    }
    for stride in [0, 5, 2000] {
        let mut data = vec![GREY; 1280 * 720 * 4];
        let mut canvas = Canvas {
            data: &mut data,
            width: 1280,
            height: 720,
            stride,
        };
        draw_controls(&mut canvas, &layout, 0xff);
    }
}

#[test]
fn a_stride_wider_than_the_width_leaves_the_padding_alone() {
    let layout = Layout::new((1280, 720), Insets::default(), 2.0, (320, 240));
    let (w, stride, h) = (1280usize, 1300usize, 720usize);
    let mut data = vec![7u8; stride * h * 4];
    let mut canvas = Canvas {
        data: &mut data,
        width: w,
        height: h,
        stride,
    };
    draw_controls(&mut canvas, &layout, 0);
    for y in 0..h {
        assert!(data[(y * stride + w) * 4..(y + 1) * stride * 4]
            .iter()
            .all(|&b| b == 7));
    }
}

#[test]
fn the_other_buttons_are_drawn_only_where_the_layout_shows_them() {
    for window in [(1280, 720), (720, 1280)] {
        let (w, h) = (window.0 as usize, window.1 as usize);
        let two = Layout::new(window, Insets::default(), 2.0, (320, 240));
        let all = two.showing(CART_BUTTONS);
        // A point on each: off the letter of a disc, at the round end of
        // a pill.
        let mut points = Vec::new();
        for disc in [all.button_x(), all.button_y()] {
            points.push((disc.cx + disc.r * 8 / 10, disc.cy));
        }
        for (_, r) in all.keys() {
            points.push((r.x + r.h / 2, r.y + 3));
        }
        assert_eq!(points.len(), 8);

        let hidden = drawn(&two, w, h, 0);
        let idle = drawn(&all, w, h, 0);
        let held = drawn(&all, w, h, CART_BUTTONS);
        for (x, y) in points {
            assert_eq!(px(&hidden, w, x, y), IDLE_GREY, "nothing at {x},{y}");
            assert_ne!(px(&idle, w, x, y), IDLE_GREY, "a control at {x},{y}");
            // Less so over the picture, where everything is fainter.
            let more = if all.picture().contains(x as f32, y as f32) {
                45
            } else {
                90
            };
            assert!(
                brightness(px(&held, w, x, y)) > brightness(px(&idle, w, x, y)) + more,
                "held is brighter at {x},{y}"
            );
        }
        // Each has something written on it, darker than the control.
        for (_, r) in all.keys() {
            let row = (r.x..r.right()).map(|x| brightness(px(&held, w, x, r.y + r.h / 2)));
            let (dark, light) = row.fold((u32::MAX, 0), |(lo, hi), b| (lo.min(b), hi.max(b)));
            assert!(dark + 120 < light, "a name on the pill at {r:?}");
        }
    }
}

#[test]
fn what_lies_over_the_picture_is_fainter_than_what_is_beside_it() {
    // 1280x720 at scale 3: bars of 160 pixels and a pad 288 across, so
    // the pad's disc is partly in the bar and partly over the picture.
    let (w, h) = (1280usize, 720usize);
    let layout = Layout::new((1280, 720), Insets::default(), 2.0, (320, 240));
    let pad = layout.dpad();
    let edge = layout.picture().x;
    assert!(pad.bounds().x < edge - 8 && pad.bounds().right() > edge + 8);

    // The left and the right arm of the cross, the same distance from
    // the centre: one beside the picture and one over it.
    let (beside, over) = (pad.cx - pad.r * 6 / 10, pad.cx + pad.r * 6 / 10);
    assert!(beside < edge && over >= edge);
    let lift = |data: &[u8], x: i32| brightness(px(data, w, x, pad.cy)) - brightness(IDLE_GREY);
    for pressed in [0, BTN_LEFT | BTN_RIGHT] {
        let data = drawn(&layout, w, h, pressed);
        let (full, faint) = (lift(&data, beside), lift(&data, over));
        // Each of the two layers there, the disc and the arm on it, keeps
        // OVER_PICTURE of its opacity, so what is left is near that share.
        let want = full * OVER_PICTURE as u32 / 255;
        assert!(faint < full * 3 / 4, "{faint} of {full} is fainter");
        assert!(faint.abs_diff(want) <= want / 6, "{faint} is near {want}");
    }
    // Held, it is still plainly brighter than idle.
    let idle = lift(&drawn(&layout, w, h, 0), over);
    let held = lift(&drawn(&layout, w, h, BTN_RIGHT), over);
    assert!(held > idle + 45);
}

#[test]
fn a_phone_with_wide_bars_has_nothing_over_the_picture() {
    // 2400x1080 with a 640x480 frame at twice its size: bars of 560.
    for shown in [0, CART_BUTTONS] {
        let (w, h) = (2400usize, 1080usize);
        let layout = Layout::new((2400, 1080), Insets::default(), 2.75, (640, 480)).showing(shown);
        let data = drawn(&layout, w, h, CART_BUTTONS | BTN_MENU);
        let p = layout.picture();
        for y in (p.y..p.bottom()).step_by(3) {
            for x in (p.x..p.right()).step_by(3) {
                assert_eq!(px(&data, w, x, y), IDLE_GREY, "a control at {x},{y}");
            }
        }
    }
}
