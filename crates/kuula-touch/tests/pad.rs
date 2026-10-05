use kuula_core::input::{
    BTN_A, BTN_B, BTN_DOWN, BTN_L1, BTN_L2, BTN_LEFT, BTN_MENU, BTN_R1, BTN_R2, BTN_RIGHT,
    BTN_SELECT, BTN_START, BTN_UP, BTN_X, BTN_Y, CART_BUTTONS,
};
use kuula_touch::{Insets, Layout, Pad};

fn layout() -> Layout {
    Layout::new((2400, 1080), Insets::default(), 2.75, (640, 480))
}

/// The point `dist` pixels from the D-pad's centre at `degrees`, counted
/// counter-clockwise from the right, as on a screen where up is smaller y.
fn around_pad(layout: &Layout, degrees: f32, dist: f32) -> (f32, f32) {
    let pad = layout.dpad();
    let rad = degrees.to_radians();
    (
        pad.cx as f32 + dist * rad.cos(),
        pad.cy as f32 - dist * rad.sin(),
    )
}

fn centre(d: kuula_touch::Disc) -> (f32, f32) {
    (d.cx as f32, d.cy as f32)
}

fn menu_centre(layout: &Layout) -> (f32, f32) {
    let m = layout.menu();
    ((m.x + m.w / 2) as f32, (m.y + m.h / 2) as f32)
}

#[test]
fn each_of_the_eight_directions_comes_from_its_sector() {
    let layout = layout();
    let want = [
        (0.0, BTN_RIGHT),
        (45.0, BTN_RIGHT | BTN_UP),
        (90.0, BTN_UP),
        (135.0, BTN_LEFT | BTN_UP),
        (180.0, BTN_LEFT),
        (225.0, BTN_LEFT | BTN_DOWN),
        (270.0, BTN_DOWN),
        (315.0, BTN_RIGHT | BTN_DOWN),
    ];
    for (degrees, bits) in want {
        let mut pad = Pad::new();
        let (x, y) = around_pad(&layout, degrees, 120.0);
        pad.down(&layout, 1, x, y);
        assert_eq!(pad.buttons(), bits, "{degrees} degrees, on going down");
        // And by moving there from the middle of the pad's dead zone.
        let mut pad = Pad::new();
        let (cx, cy) = centre(layout.dpad());
        pad.down(&layout, 1, cx, cy);
        assert_eq!(pad.buttons(), 0);
        pad.moved(&layout, 1, x, y);
        assert_eq!(pad.buttons(), bits, "{degrees} degrees, moving");
    }
}

#[test]
fn the_sectors_are_45_degrees_wide() {
    let layout = layout();
    let mut pad = Pad::new();
    let at = |pad: &mut Pad, degrees: f32| {
        let (x, y) = around_pad(&layout, degrees, 150.0);
        pad.down(&layout, 1, x, y);
        let held = pad.buttons();
        pad.up(1);
        held
    };
    assert_eq!(at(&mut pad, 21.0), BTN_RIGHT);
    assert_eq!(at(&mut pad, 24.0), BTN_RIGHT | BTN_UP);
    assert_eq!(at(&mut pad, 66.0), BTN_RIGHT | BTN_UP);
    assert_eq!(at(&mut pad, 69.0), BTN_UP);
    assert_eq!(at(&mut pad, -21.0), BTN_RIGHT);
    assert_eq!(at(&mut pad, 204.0), BTN_LEFT | BTN_DOWN);
}

#[test]
fn nothing_is_held_inside_the_dead_zone() {
    let layout = layout();
    let r = layout.dpad().r as f32;
    let mut pad = Pad::new();
    let (x, y) = around_pad(&layout, 0.0, r * 0.11);
    pad.down(&layout, 1, x, y);
    assert_eq!(pad.buttons(), 0, "11% of the radius");
    let (x, y) = around_pad(&layout, 0.0, r * 0.13);
    pad.moved(&layout, 1, x, y);
    assert_eq!(pad.buttons(), BTN_RIGHT, "13% of the radius");
    let (x, y) = around_pad(&layout, 0.0, 0.0);
    pad.moved(&layout, 1, x, y);
    assert_eq!(pad.buttons(), 0, "back in the middle");
}

#[test]
fn a_finger_on_the_pad_steers_it_wherever_it_goes() {
    let layout = layout();
    let mut pad = Pad::new();
    // Down at the edge of the claim area, 1.5 times the drawn radius.
    let (x, y) = around_pad(&layout, 90.0, layout.dpad().r as f32 * 1.49);
    pad.down(&layout, 7, x, y);
    assert_eq!(pad.buttons(), BTN_UP);
    // Slid far away, over the picture and off the screen.
    pad.moved(&layout, 7, 1200.0, 100.0);
    assert_eq!(pad.buttons(), BTN_RIGHT | BTN_UP);
    pad.moved(&layout, 7, -2000.0, 2500.0);
    assert_eq!(pad.buttons(), BTN_LEFT | BTN_DOWN);
    // Over button A it is still the pad's finger, not A's.
    let (ax, ay) = centre(layout.button_a());
    pad.moved(&layout, 7, ax, ay);
    assert_eq!(pad.buttons() & (BTN_A | BTN_B), 0);
    pad.up(7);
    assert_eq!(pad.buttons(), 0);
}

#[test]
fn a_finger_just_outside_the_claim_area_is_a_button_finger() {
    let layout = layout();
    let mut pad = Pad::new();
    let (x, y) = around_pad(&layout, 90.0, layout.dpad().r as f32 * 1.51);
    pad.down(&layout, 1, x, y);
    assert_eq!(pad.buttons(), 0);
    // It does not steer the pad when it slides onto it.
    let (x, y) = around_pad(&layout, 0.0, 120.0);
    pad.moved(&layout, 1, x, y);
    assert_eq!(pad.buttons(), 0);
}

#[test]
fn a_second_finger_holds_a_button_while_the_first_steers() {
    let layout = layout();
    let mut pad = Pad::new();
    let (x, y) = around_pad(&layout, 0.0, 120.0);
    pad.down(&layout, 1, x, y);
    let (ax, ay) = centre(layout.button_a());
    pad.down(&layout, 2, ax, ay);
    assert_eq!(pad.buttons(), BTN_RIGHT | BTN_A);
    let (bx, by) = centre(layout.button_b());
    pad.down(&layout, 3, bx, by);
    assert_eq!(pad.buttons(), BTN_RIGHT | BTN_A | BTN_B);
    // Lifting the pad's finger leaves the others.
    pad.up(1);
    assert_eq!(pad.buttons(), BTN_A | BTN_B);
    // Two fingers on one button, and one lifts: it is still held.
    pad.down(&layout, 4, ax, ay);
    pad.up(2);
    assert_eq!(pad.buttons(), BTN_A | BTN_B);
    pad.up(4);
    assert_eq!(pad.buttons(), BTN_B);
}

#[test]
fn a_button_finger_slides_between_the_buttons_and_holds_both_in_the_gap() {
    let layout = layout();
    let mut pad = Pad::new();
    let (a, b) = (centre(layout.button_a()), centre(layout.button_b()));
    pad.down(&layout, 1, a.0, a.1);
    assert_eq!(pad.buttons(), BTN_A);
    let gap = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    pad.moved(&layout, 1, gap.0, gap.1);
    assert_eq!(pad.buttons(), BTN_A | BTN_B, "one thumb in the gap");
    pad.moved(&layout, 1, b.0, b.1);
    assert_eq!(pad.buttons(), BTN_B);
    // Outside both it holds nothing, but it is still tracked.
    pad.moved(&layout, 1, (a.0 + b.0) / 2.0, a.1 - 400.0);
    assert_eq!(pad.buttons(), 0);
    pad.moved(&layout, 1, a.0, a.1);
    assert_eq!(pad.buttons(), BTN_A);
    // The hit circle is 1.4 times the drawn radius.
    let r = layout.button_a().r as f32;
    pad.moved(&layout, 1, a.0 + r * 1.39, a.1);
    assert_eq!(pad.buttons() & BTN_A, BTN_A);
    pad.moved(&layout, 1, a.0 + r * 1.41, a.1 - 0.0);
    assert_eq!(pad.buttons() & BTN_A, 0);
}

#[test]
fn a_finger_that_goes_down_on_nothing_holds_nothing_until_it_slides_on_a_button() {
    let layout = layout();
    let mut pad = Pad::new();
    pad.down(&layout, 1, 1200.0, 500.0);
    assert_eq!(pad.buttons(), 0);
    let (bx, by) = centre(layout.button_b());
    pad.moved(&layout, 1, bx, by);
    assert_eq!(pad.buttons(), BTN_B);
}

#[test]
fn menu_is_held_until_its_finger_lifts() {
    let layout = layout();
    let mut pad = Pad::new();
    let (x, y) = menu_centre(&layout);
    pad.down(&layout, 1, x, y);
    assert_eq!(pad.buttons(), BTN_MENU);
    // The hit area is the pill grown by 12 dp (33 pixels).
    pad.moved(&layout, 1, 100.0, 900.0);
    assert_eq!(pad.buttons(), BTN_MENU, "it stays held wherever it goes");
    pad.up(1);
    assert_eq!(pad.buttons(), 0);
    let m = layout.menu();
    pad.down(&layout, 2, (m.x - 30) as f32, (m.y - 30) as f32);
    assert_eq!(pad.buttons(), BTN_MENU, "just outside the pill");
    pad.up(2);
    pad.down(&layout, 3, (m.x - 40) as f32, (m.y - 40) as f32);
    assert_eq!(pad.buttons(), 0, "outside the grown area");
}

#[test]
fn a_finger_lifting_releases_only_what_it_held() {
    let layout = layout();
    let mut pad = Pad::new();
    let (a, b) = (centre(layout.button_a()), centre(layout.button_b()));
    let (mx, my) = menu_centre(&layout);
    pad.down(&layout, 10, a.0, a.1);
    pad.down(&layout, 11, b.0, b.1);
    pad.down(&layout, 12, mx, my);
    assert_eq!(pad.buttons(), BTN_A | BTN_B | BTN_MENU);
    pad.up(11);
    assert_eq!(pad.buttons(), BTN_A | BTN_MENU);
    pad.up(10);
    assert_eq!(pad.buttons(), BTN_MENU);
    pad.up(12);
    assert_eq!(pad.buttons(), 0);
}

#[test]
fn cancel_forgets_every_finger() {
    let layout = layout();
    let mut pad = Pad::new();
    let (x, y) = around_pad(&layout, 180.0, 120.0);
    pad.down(&layout, 1, x, y);
    let (ax, ay) = centre(layout.button_a());
    pad.down(&layout, 2, ax, ay);
    let (mx, my) = menu_centre(&layout);
    pad.down(&layout, 3, mx, my);
    assert_eq!(pad.buttons(), BTN_LEFT | BTN_A | BTN_MENU);
    pad.cancel();
    assert_eq!(pad.buttons(), 0);
    // Their moves and lifts are for fingers that are not there.
    pad.moved(&layout, 2, ax, ay);
    pad.up(1);
    assert_eq!(pad.buttons(), 0);
}

#[test]
fn an_id_that_never_went_down_is_ignored() {
    let layout = layout();
    let mut pad = Pad::new();
    let (ax, ay) = centre(layout.button_a());
    pad.moved(&layout, 5, ax, ay);
    assert_eq!(pad.buttons(), 0);
    pad.up(5);
    pad.down(&layout, 1, ax, ay);
    pad.moved(&layout, 6, 0.0, 0.0);
    pad.up(6);
    assert_eq!(pad.buttons(), BTN_A);
}

#[test]
fn a_finger_that_goes_down_again_replaces_itself_and_extra_fingers_are_ignored() {
    let layout = layout();
    let mut pad = Pad::new();
    let (ax, ay) = centre(layout.button_a());
    let (bx, by) = centre(layout.button_b());
    pad.down(&layout, 1, ax, ay);
    pad.down(&layout, 1, bx, by);
    assert_eq!(pad.buttons(), BTN_B);
    pad.cancel();
    for id in 0..kuula_touch::MAX_FINGERS as u64 {
        pad.down(&layout, id, 1200.0, 500.0);
    }
    pad.down(&layout, 99, ax, ay);
    assert_eq!(pad.buttons(), 0, "no room for an eleventh");
    pad.up(0);
    pad.down(&layout, 99, ax, ay);
    assert_eq!(pad.buttons(), BTN_A);
}

#[test]
fn portrait_layouts_work_the_same_way() {
    let layout = Layout::new((1080, 2400), Insets::default(), 2.75, (320, 240));
    let mut pad = Pad::new();
    let (x, y) = around_pad(&layout, 270.0, 100.0);
    pad.down(&layout, 1, x, y);
    let (ax, ay) = centre(layout.button_a());
    pad.down(&layout, 2, ax, ay);
    let (mx, my) = menu_centre(&layout);
    pad.down(&layout, 3, mx, my);
    assert_eq!(pad.buttons(), BTN_DOWN | BTN_A | BTN_MENU);
}

fn all_buttons() -> Layout {
    layout().showing(CART_BUTTONS)
}

fn rect_centre(r: kuula_touch::Rect) -> (f32, f32) {
    ((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32)
}

#[test]
fn a_cart_with_two_buttons_has_no_other_controls() {
    let layout = layout();
    let mut pad = Pad::new();
    let places = [centre(layout.button_x()), centre(layout.button_y())]
        .into_iter()
        .chain([BTN_L1, BTN_R2, BTN_START].map(|b| rect_centre(layout.key(b).unwrap())));
    for (id, (x, y)) in places.enumerate() {
        pad.down(&layout, id as u64, x, y);
    }
    assert_eq!(pad.buttons(), 0);
    // Where Select would be, under the D-pad, a finger steers down.
    let (x, y) = rect_centre(layout.key(BTN_SELECT).unwrap());
    pad.down(&layout, 9, x, y);
    assert_eq!(pad.buttons(), BTN_DOWN);
}

#[test]
fn every_pill_is_held_by_a_finger_on_it_until_the_finger_lifts() {
    let layout = all_buttons();
    for bit in [BTN_L1, BTN_R1, BTN_L2, BTN_R2, BTN_START, BTN_SELECT] {
        let mut pad = Pad::new();
        let (x, y) = rect_centre(layout.key(bit).unwrap());
        pad.down(&layout, 1, x, y);
        assert_eq!(pad.buttons(), bit);
        // It slides onto A and is still on its pill.
        let (ax, ay) = centre(layout.button_a());
        pad.moved(&layout, 1, ax, ay);
        assert_eq!(pad.buttons(), bit);
        pad.up(1);
        assert_eq!(pad.buttons(), 0);
    }
    // A shoulder button beside Menu is its own, not Menu's wider area.
    let mut pad = Pad::new();
    let r2 = layout.key(BTN_R2).unwrap();
    pad.down(&layout, 1, (r2.x + r2.w / 2) as f32, r2.y as f32);
    assert_eq!(pad.buttons(), BTN_R2);
    let (mx, my) = menu_centre(&layout);
    pad.down(&layout, 2, mx, my);
    assert_eq!(pad.buttons(), BTN_R2 | BTN_MENU);
}

#[test]
fn x_and_y_are_round_buttons_like_a_and_b() {
    let layout = all_buttons();
    let (a, b) = (centre(layout.button_a()), centre(layout.button_b()));
    let (x, y) = (centre(layout.button_x()), centre(layout.button_y()));
    let between = |p: (f32, f32), q: (f32, f32)| ((p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0);

    let mut pad = Pad::new();
    pad.down(&layout, 1, x.0, x.1);
    assert_eq!(pad.buttons(), BTN_X);
    // Sliding to A: both in the gap, then A alone.
    let gap = between(x, a);
    pad.moved(&layout, 1, gap.0, gap.1);
    assert_eq!(pad.buttons(), BTN_X | BTN_A);
    pad.moved(&layout, 1, a.0, a.1);
    assert_eq!(pad.buttons(), BTN_A);
    // The middle of the four is nothing: no thumb holds all of them.
    let middle = between(a, y);
    pad.moved(&layout, 1, middle.0, middle.1);
    assert_eq!(pad.buttons(), 0);
    let gap = between(y, b);
    pad.moved(&layout, 1, gap.0, gap.1);
    assert_eq!(pad.buttons(), BTN_Y | BTN_B);
    pad.down(&layout, 2, x.0, x.1);
    assert_eq!(pad.buttons(), BTN_Y | BTN_B | BTN_X);
}

#[test]
fn a_finger_on_the_picture_of_a_button_is_on_it_however_near_the_pad_is() {
    // An upright phone of 360 dp: the left of Y is nearer the D-pad than
    // the edge of the area in which a finger claims it.
    let two = Layout::new((720, 1280), Insets::default(), 2.0, (320, 240));
    let all = two.showing(CART_BUTTONS);
    let y = all.button_y();
    let (px, py) = ((y.cx - y.r + 6) as f32, y.cy as f32);
    assert!(all.dpad().within(px, py, all.dpad_claim_radius()));

    let mut pad = Pad::new();
    pad.down(&all, 1, px, py);
    assert_eq!(pad.buttons(), BTN_Y);
    // Where Y is not shown the same finger steers.
    let mut pad = Pad::new();
    pad.down(&two, 1, px, py);
    assert_eq!(pad.buttons() & (BTN_RIGHT | BTN_UP), pad.buttons());
    assert_ne!(pad.buttons(), 0);
}

#[test]
fn where_a_pill_lies_on_a_round_button_the_finger_gets_the_button() {
    // A landscape window 280 dp high: too short for the shoulder row to
    // clear X, so R2 lies over the top of it.
    let layout = Layout::new((840, 420), Insets::default(), 1.5, (640, 480)).showing(CART_BUTTONS);
    let x = layout.button_x();
    let r2 = layout.key(BTN_R2).unwrap().grown(layout.key_grow());
    let (px, py) = (x.cx as f32, (x.cy - x.r + 3) as f32);
    assert!(
        r2.contains(px, py) && x.within(px, py, x.r as f32),
        "the two overlap at {px},{py}"
    );
    let mut pad = Pad::new();
    pad.down(&layout, 1, px, py);
    assert_eq!(pad.buttons(), BTN_X);
    // The rest of the pill is still the pill.
    pad.down(&layout, 2, (r2.x + 8) as f32, (r2.y + 8) as f32);
    assert_eq!(pad.buttons(), BTN_X | BTN_R2);
}

#[test]
fn in_a_short_upright_window_the_pad_keeps_its_picture_from_select() {
    // 360 dp wide with 200 dp under the picture: Select, beside Menu at
    // the bottom, lies across the lower part of the D-pad.
    let layout = Layout::new((720, 880), Insets::default(), 2.0, (640, 480)).showing(CART_BUTTONS);
    let dpad = layout.dpad();
    let select = layout.key(BTN_SELECT).unwrap();
    let (px, py) = (dpad.cx as f32, (select.y + 8) as f32);
    assert!(
        select.contains(px, py) && dpad.within(px, py, dpad.r as f32),
        "the two overlap at {px},{py}"
    );
    let mut pad = Pad::new();
    pad.down(&layout, 1, px, py);
    assert_eq!(pad.buttons(), BTN_DOWN);
    // Off the D-pad's picture, Select is Select, though the D-pad would
    // claim a finger this near it.
    let (sx, sy) = ((select.right() - 8) as f32, (select.bottom() - 8) as f32);
    assert!(!dpad.within(sx, sy, dpad.r as f32));
    assert!(dpad.within(sx, sy, layout.dpad_claim_radius()));
    pad.down(&layout, 2, sx, sy);
    assert_eq!(pad.buttons(), BTN_DOWN | BTN_SELECT);
}

#[test]
fn menu_is_menu_on_its_own_pill_whatever_lies_under_it() {
    // The same short window: R2 is level with Menu and beside it, and
    // Menu's wider area reaches over the gap between them.
    let layout = Layout::new((840, 420), Insets::default(), 1.5, (640, 480)).showing(CART_BUTTONS);
    let (mx, my) = menu_centre(&layout);
    let mut pad = Pad::new();
    pad.down(&layout, 1, mx, my);
    assert_eq!(pad.buttons(), BTN_MENU);
    // In the gap, the pill's own area comes before Menu's.
    let r2 = layout.key(BTN_R2).unwrap();
    pad.down(
        &layout,
        2,
        (r2.right() + 3) as f32,
        (r2.y + r2.h / 2) as f32,
    );
    assert_eq!(pad.buttons(), BTN_MENU | BTN_R2);
}
