use kuula_core::input::{
    BASE_BUTTONS, BTN_A, BTN_B, BTN_L1, BTN_L2, BTN_R1, BTN_R2, BTN_SELECT, BTN_START, BTN_X,
    BTN_Y, CART_BUTTONS,
};
use kuula_touch::{
    Disc, Insets, Layout, Rect, GAP_DP, MARGIN_DP, MENU_H_DP, SHOULDER_H_DP, SHOULDER_W_DP,
    START_W_DP,
};

const NO_INSETS: Insets = Insets {
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
};

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

fn inside(window: (i32, i32), insets: Insets, r: Rect, what: &str) {
    assert!(
        r.x >= insets.left
            && r.y >= insets.top
            && r.right() <= window.0 - insets.right
            && r.bottom() <= window.1 - insets.bottom,
        "{what} {r:?} is outside {window:?} less {insets:?}"
    );
}

fn controls(layout: &Layout) -> [(&'static str, Rect); 4] {
    let disc = |d: Disc| d.bounds();
    [
        ("dpad", disc(layout.dpad())),
        ("a", disc(layout.button_a())),
        ("b", disc(layout.button_b())),
        ("menu", layout.menu()),
    ]
}

/// Every control is inside the window and the insets, and no two of the
/// discs cover each other.
fn check(layout: &Layout, window: (i32, i32), insets: Insets) {
    for (what, r) in controls(layout) {
        inside(window, insets, r, what);
    }
    let (a, b) = (layout.button_a(), layout.button_b());
    let apart = (((a.cx - b.cx).pow(2) + (a.cy - b.cy).pow(2)) as f32).sqrt();
    assert!(apart > 2.0 * a.r as f32, "A and B touch: {apart}");
    assert!(b.cx < a.cx && b.cy > a.cy, "B is to the lower left of A");
    assert!(layout.dpad().cx < b.cx, "the D-pad is left of A and B");
}

const WITH_CUTOUT: Insets = Insets {
    left: 100,
    top: 0,
    right: 0,
    bottom: 0,
};

#[test]
fn landscape_windows_get_the_largest_whole_scale_and_a_centred_picture() {
    // window, density, picture, twice the scale, picture rectangle. A whole
    // scale wherever it is 2 or more; one and a half where only 1 would fit
    // and one and a half does (the 640-pixel frame on a 720-pixel window).
    let cases = [
        ((2400, 1080), 2.75, (640, 480), 4, rect(560, 60, 1280, 960)),
        ((2400, 1080), 2.75, (320, 240), 8, rect(560, 60, 1280, 960)),
        ((1920, 1080), 2.625, (640, 480), 4, rect(320, 60, 1280, 960)),
        ((1920, 1080), 2.625, (320, 240), 8, rect(320, 60, 1280, 960)),
        ((1280, 720), 2.0, (640, 480), 3, rect(160, 0, 960, 720)),
        ((1280, 720), 2.0, (320, 240), 6, rect(160, 0, 960, 720)),
        ((2000, 1200), 2.0, (640, 480), 4, rect(360, 120, 1280, 960)),
        ((2000, 1200), 2.0, (320, 240), 10, rect(200, 0, 1600, 1200)),
        // Too small for one and a half: the frame as it is.
        ((900, 700), 2.0, (640, 480), 2, rect(130, 110, 640, 480)),
    ];
    for (window, density, picture, halves, expected) in cases {
        for insets in [NO_INSETS, WITH_CUTOUT] {
            let layout = Layout::new(window, insets, density, picture);
            assert!(!layout.is_portrait());
            assert_eq!(layout.scale_halves(), halves, "{window:?} {picture:?}");
            assert_eq!(layout.scale(), halves / 2, "{window:?} {picture:?}");
            assert_eq!(layout.picture(), expected, "{window:?} {picture:?}");
            assert_eq!(layout.source(), picture);
            check(&layout, window, insets);
            // The D-pad is in the left bar's side of the window, the
            // buttons and Menu in the right's.
            let middle = window.0 / 2;
            assert!(layout.dpad().cx < middle);
            assert!(layout.button_a().cx > middle && layout.button_b().cx > middle);
            assert!(layout.menu().x > middle);
            // Menu is in the top right corner, the pad a little below the
            // middle.
            assert!(layout.menu().y < window.1 / 4);
            assert!(layout.dpad().cy > window.1 / 2);
        }
    }
}

#[test]
fn landscape_controls_are_sized_in_dp_and_keep_their_margins() {
    let layout = Layout::new((2400, 1080), NO_INSETS, 2.75, (640, 480));
    // 144 dp across the D-pad, 64 the buttons, 56 by 28 the pill; 16 dp
    // is 44 pixels.
    assert_eq!(layout.dpad().r, 198);
    assert_eq!(layout.button_a().r, 88);
    assert_eq!(layout.button_b().r, 88);
    assert_eq!((layout.menu().w, layout.menu().h), (154, 77));
    assert_eq!(layout.menu().right(), 2400 - 44);
    assert_eq!(layout.menu().y, 44);
    // The hit areas: 1.5 times the pad, 1.4 times a button, Menu grown
    // by 12 dp.
    assert_eq!(layout.dpad_claim_radius(), 297.0);
    assert!((layout.button_hit_radius() - 123.2).abs() < 0.01);
    assert_eq!(layout.menu_hit(), layout.menu().grown(33));
    // A wide bar centres the pad in it.
    assert_eq!(layout.dpad().cx, 280);
}

#[test]
fn a_bar_narrower_than_its_control_keeps_the_control_by_the_edge() {
    // 1280x720 at scale 3: bars of 160 pixels, a pad 288 across.
    let layout = Layout::new((1280, 720), NO_INSETS, 2.0, (320, 240));
    let pad = layout.dpad();
    assert_eq!(pad.bounds().x, 32, "the margin, not the bar's middle");
    assert!(
        pad.bounds().right() > layout.picture().x,
        "over the picture"
    );
    let a = layout.button_a().bounds();
    assert_eq!(a.right(), 1280 - 32);
    check(&layout, (1280, 720), NO_INSETS);

    // With a cutout the pad keeps clear of it.
    let layout = Layout::new((1280, 720), WITH_CUTOUT, 2.0, (320, 240));
    assert_eq!(layout.dpad().bounds().x, 100 + 32);
    check(&layout, (1280, 720), WITH_CUTOUT);
}

#[test]
fn portrait_windows_put_the_picture_on_top_and_the_controls_under_it() {
    let tall = Insets {
        left: 0,
        top: 80,
        right: 0,
        bottom: 48,
    };
    // window, density, picture, twice the scale, picture rectangle's x and
    // width. The 640-pixel frame is one and a half times its size across a
    // 1080-pixel window, where a whole scale would leave it at 640.
    let cases = [
        ((1080, 2400), 2.75, (640, 480), 3, (60, 960)),
        ((1080, 2400), 2.75, (320, 240), 6, (60, 960)),
        ((1080, 1920), 2.625, (640, 480), 3, (60, 960)),
        ((1080, 1920), 2.625, (320, 240), 6, (60, 960)),
        ((720, 1280), 2.0, (640, 480), 2, (40, 640)),
        ((720, 1280), 2.0, (320, 240), 4, (40, 640)),
    ];
    for (window, density, picture, halves, (x, w)) in cases {
        for insets in [NO_INSETS, tall] {
            let layout = Layout::new(window, insets, density, picture);
            assert!(layout.is_portrait(), "{window:?}");
            assert_eq!(layout.scale_halves(), halves, "{window:?} {picture:?}");
            let p = layout.picture();
            assert_eq!((p.x, p.w), (x, w), "{window:?} {picture:?}");
            assert_eq!(p.h, picture.1 as i32 * halves / 2);
            assert_eq!(p.y, insets.top, "the picture's top is the top inset");
            check(&layout, window, insets);

            // The controls are in the space under the picture.
            let (pad, a, b, menu) = (
                layout.dpad(),
                layout.button_a(),
                layout.button_b(),
                layout.menu(),
            );
            for (what, r) in controls(&layout) {
                assert!(r.y >= p.bottom(), "{what} {r:?} is over the picture {p:?}");
            }
            // The pad at the lower left, the buttons at the lower right,
            // Menu between them near the bottom.
            assert!(pad.cx < window.0 / 2 && a.cx > window.0 / 2 && b.cx > window.0 / 2);
            let middle = (menu.x + menu.right()) / 2;
            assert!((middle - window.0 / 2).abs() <= 1, "Menu is centred");
            assert!(pad.cx + pad.r <= menu.x || pad.cy + pad.r <= menu.y);
            assert!(menu.right() <= b.cx.max(a.cx - a.r) || menu.bottom() <= b.cy - b.r);
            assert!(menu.bottom() > window.1 - insets.bottom - 2 * (density * 16.0) as i32);
            // The pad and the buttons are in the middle of the space.
            let space = (p.bottom() + window.1 - insets.bottom) / 2;
            assert!((pad.cy - space).abs() <= 1, "pad {} space {space}", pad.cy);
            assert!(((a.cy + b.cy) / 2 - space).abs() <= 1);
        }
    }
}

#[test]
fn a_portrait_window_with_too_little_room_keeps_the_controls_at_the_bottom() {
    // 280 pixels under the picture, and a pad 288 across.
    let window = (720, 760);
    let layout = Layout::new(window, NO_INSETS, 2.0, (640, 480));
    assert!(layout.is_portrait());
    let pad = layout.dpad();
    assert_eq!(pad.bounds().bottom(), 760 - 32, "at the bottom margin");
    assert!(
        pad.bounds().y < layout.picture().bottom(),
        "over the picture"
    );
    check(&layout, window, NO_INSETS);
}

#[test]
fn a_square_window_is_landscape_and_nonsense_does_not_panic() {
    let layout = Layout::new((900, 900), NO_INSETS, 2.0, (320, 240));
    assert!(!layout.is_portrait());
    for window in [(0, 0), (1, 1), (50, 3000), (3000, 50), (-5, 10)] {
        for density in [0.0, -1.0, f32::NAN, 1.0, 5.0] {
            let layout = Layout::new(window, NO_INSETS, density, (320, 240));
            assert!(layout.scale() >= 1);
        }
    }
}

fn centre(r: Rect) -> (i32, i32) {
    (r.x + r.w / 2, r.y + r.h / 2)
}

fn key(layout: &Layout, bit: u16) -> Rect {
    layout.key(bit).expect("a pill of its own")
}

#[test]
fn the_other_buttons_show_when_asked_for_and_nothing_moves() {
    for window in [(2400, 1080), (1080, 2400)] {
        let two = Layout::new(window, NO_INSETS, 2.75, (640, 480));
        assert_eq!(two.shown(), BASE_BUTTONS);
        assert_eq!(two.faces().count(), 2);
        assert_eq!(two.keys().count(), 0);

        let all = two.showing(CART_BUTTONS);
        assert_eq!(all.shown(), CART_BUTTONS);
        let faces: Vec<u16> = all.faces().map(|(bit, _)| bit).collect();
        assert_eq!(faces, [BTN_A, BTN_B, BTN_X, BTN_Y]);
        let keys: Vec<u16> = all.keys().map(|(bit, _)| bit).collect();
        assert_eq!(
            keys,
            [BTN_L2, BTN_L1, BTN_R1, BTN_R2, BTN_SELECT, BTN_START]
        );
        assert_eq!(all.picture(), two.picture());
        assert_eq!(all.dpad(), two.dpad());
        assert_eq!(all.button_a(), two.button_a());
        assert_eq!(all.button_b(), two.button_b());
        assert_eq!(all.menu(), two.menu());
        // A place is the same whether it is shown or not, and the two
        // buttons a cart always has cannot be taken away.
        assert_eq!(two.button_x(), all.button_x());
        assert_eq!(two.key(BTN_START), all.key(BTN_START));
        assert_eq!(all.showing(0).shown(), BASE_BUTTONS);
        assert_eq!(all.key(BTN_A), None);

        // Some of them: only those.
        let some = two.showing(BTN_X | BTN_START);
        let faces: Vec<u16> = some.faces().map(|(bit, _)| bit).collect();
        assert_eq!(faces, [BTN_A, BTN_B, BTN_X]);
        let keys: Vec<u16> = some.keys().map(|(bit, _)| bit).collect();
        assert_eq!(keys, [BTN_START]);
    }
}

#[test]
fn x_and_y_make_a_square_with_a_and_b_laid_out_as_on_a_handheld() {
    for window in [(2400, 1080), (1080, 2400), (1280, 720)] {
        let layout = Layout::new(window, NO_INSETS, 2.0, (640, 480));
        let (a, b) = (layout.button_a(), layout.button_b());
        let (x, y) = (layout.button_x(), layout.button_y());
        let apart =
            |p: Disc, q: Disc| (((p.cx - q.cx).pow(2) + (p.cy - q.cy).pow(2)) as f32).sqrt();
        let side = apart(a, b);
        for (p, q) in [(a, x), (b, y), (x, y)] {
            assert!((apart(p, q) - side).abs() < 1.0, "a side of the square");
        }
        for (p, q) in [(a, y), (b, x)] {
            assert!((apart(p, q) - side * 2f32.sqrt()).abs() < 1.5, "a diagonal");
        }
        assert!(side > 2.0 * a.r as f32, "neighbours do not touch");
        // X at the top, Y on the left, A on the right, B at the bottom.
        assert!(x.cy < y.cy.min(a.cy) && b.cy > y.cy.max(a.cy));
        assert!(y.cx < x.cx.min(b.cx) && a.cx > x.cx.max(b.cx));
        assert_eq!((x.r, y.r), (a.r, a.r));
    }
}

#[test]
fn landscape_puts_the_shoulders_under_menu_and_select_and_start_under_the_clusters() {
    let window = (2400, 1080);
    let layout = Layout::new(window, WITH_CUTOUT, 2.75, (640, 480)).showing(CART_BUTTONS);
    let dp = |v: f32| (v * 2.75).round() as i32;
    for (bit, r) in layout.keys() {
        inside(window, WITH_CUTOUT, r, &format!("pill {bit}"));
    }
    let (l1, l2) = (key(&layout, BTN_L1), key(&layout, BTN_L2));
    let (r1, r2) = (key(&layout, BTN_R1), key(&layout, BTN_R2));
    for r in [l1, l2, r1, r2] {
        assert_eq!((r.w, r.h), (dp(SHOULDER_W_DP), dp(SHOULDER_H_DP)));
        assert_eq!(r.y, l1.y, "one row");
        assert!(r.y >= layout.menu().bottom() + dp(GAP_DP), "under Menu");
        assert!(
            r.bottom() <= layout.button_x().bounds().y,
            "above X, the higher of the two clusters"
        );
    }
    // L2 and L1 from the left margin, R1 and R2 up to the right one.
    assert_eq!(l2.x, WITH_CUTOUT.left + dp(MARGIN_DP));
    assert_eq!(l1.x, l2.right() + dp(GAP_DP));
    assert_eq!(r2.right(), window.0 - dp(MARGIN_DP));
    assert_eq!(r1.right(), r2.x - dp(GAP_DP));

    let (select, start) = (key(&layout, BTN_SELECT), key(&layout, BTN_START));
    for r in [select, start] {
        assert_eq!((r.w, r.h), (dp(START_W_DP), dp(MENU_H_DP)));
        assert_eq!(r.y, select.y);
    }
    let pad = layout.dpad();
    assert_eq!(centre(select).0, pad.cx);
    assert!(select.y > pad.cy + pad.r, "under the D-pad");
    assert!(start.y > layout.button_b().bounds().bottom(), "under B");
    assert!(start.x > layout.picture().right());
}

#[test]
fn portrait_puts_select_and_start_beside_menu_and_the_shoulders_over_the_clusters() {
    let window = (1080, 2400);
    let insets = Insets {
        left: 0,
        top: 80,
        right: 0,
        bottom: 48,
    };
    let layout = Layout::new(window, insets, 2.75, (640, 480)).showing(CART_BUTTONS);
    let dp = |v: f32| (v * 2.75).round() as i32;
    for (bit, r) in layout.keys() {
        inside(window, insets, r, &format!("pill {bit}"));
        assert!(r.y >= layout.picture().bottom(), "under the picture");
    }
    let menu = layout.menu();
    let (select, start) = (key(&layout, BTN_SELECT), key(&layout, BTN_START));
    assert_eq!((select.y, start.y), (menu.y, menu.y));
    assert_eq!(select.right(), menu.x - dp(GAP_DP));
    assert_eq!(start.x, menu.right() + dp(GAP_DP));

    let (l1, l2) = (key(&layout, BTN_L1), key(&layout, BTN_L2));
    let (r1, r2) = (key(&layout, BTN_R1), key(&layout, BTN_R2));
    assert_eq!(l2.x, dp(MARGIN_DP));
    assert_eq!(r2.right(), window.0 - dp(MARGIN_DP));
    assert!(l1.right() < r1.x, "the two rows do not meet");
    let top = layout.button_x().bounds().y.min(layout.dpad().bounds().y);
    for r in [l1, l2, r1, r2] {
        assert_eq!(r.bottom(), top - dp(GAP_DP), "just over the clusters");
    }
}

#[test]
fn short_landscape_windows_keep_the_shoulder_rows_off_x_and_off_menu() {
    // Heights of 393, 360, 360 and 320 dp.
    for (window, density) in [
        ((2400, 1080), 2.75),
        ((2400, 1080), 3.0),
        ((1280, 720), 2.0),
        ((854, 480), 1.5),
    ] {
        let layout = Layout::new(window, NO_INSETS, density, (640, 480)).showing(CART_BUTTONS);
        let dp = |v: f32| (v * density).round() as i32;
        let (x, menu) = (layout.button_x().bounds(), layout.menu());
        for bit in [BTN_L1, BTN_L2, BTN_R1, BTN_R2] {
            let r = key(&layout, bit);
            assert!(r.bottom() <= x.y, "{window:?}: pill {bit} is on X");
            assert!(
                r.y >= dp(MARGIN_DP),
                "{window:?}: pill {bit} is off the top"
            );
            let on_menu = r.x < menu.right()
                && menu.x < r.right()
                && r.y < menu.bottom()
                && menu.y < r.bottom();
            assert!(!on_menu, "{window:?}: pill {bit} is on Menu");
        }
    }

    // 360 dp: there is just room under Menu, down to the top of X.
    let layout = Layout::new((1280, 720), NO_INSETS, 2.0, (640, 480)).showing(CART_BUTTONS);
    let r2 = key(&layout, BTN_R2);
    assert_eq!(r2.y, layout.menu().bottom() + 16);
    assert_eq!(r2.right(), 1280 - 32);
    // 320 dp: there is not, and the row is level with Menu, on its left.
    let layout = Layout::new((854, 480), NO_INSETS, 1.5, (640, 480)).showing(CART_BUTTONS);
    let (r1, r2) = (key(&layout, BTN_R1), key(&layout, BTN_R2));
    assert!(r2.y < layout.menu().bottom());
    assert_eq!(r2.right(), layout.menu().x - 12);
    assert_eq!(r1.right(), r2.x - 12);
    assert_eq!(key(&layout, BTN_L2).y, r2.y, "one height for both rows");
}
