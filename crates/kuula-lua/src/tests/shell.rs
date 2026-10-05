//! The shell guest from `rom/main.lua` driven headless: boot, list, run,
//! pause, error screen, restart and quit.

use std::rc::Rc;

use kuula_core::input::{BTN_A, BTN_B, BTN_MENU};
use kuula_core::shell::{CartEntry, Settings, SysRequest};
use kuula_core::{Console, FrameInput, MemoryStore};

use super::cart;
use crate::LuaGuest;

const ROM: &str = include_str!("../../../../rom/main.lua");

const HELLO: &str = r#"
n = 0
function _update(dt) n = n + 1 end
function _draw() cls(2) end
"#;

const DIES: &str = r#"
function _update(dt)
  if stat("frame") == 3 then error("bang") end
end
function _draw() cls(2) end
"#;

fn shell_console(carts: &[(&str, &str)]) -> Console {
    let sources: Vec<(String, Rc<kuula_core::Snapshot>)> = carts
        .iter()
        .map(|(name, src)| (name.to_string(), cart(&[("main.lua", src.as_bytes())])))
        .collect();
    let entries = sources
        .iter()
        .map(|(n, _)| CartEntry {
            name: n.clone(),
            title: n.to_uppercase(),
            ..CartEntry::default()
        })
        .collect();
    let opener: kuula_core::console::CartOpener = Rc::new(move |name: &str| {
        let (_, snap) = sources
            .iter()
            .find(|(n, _)| n == name)
            .ok_or_else(|| kuula_core::Fault::new("cart_read_error", name, None, "no such cart"))?;
        Ok((
            snap.clone() as Rc<dyn kuula_core::CartSource>,
            Box::new(MemoryStore::new()) as Box<dyn kuula_core::SaveStore>,
        ))
    });
    let shell = LuaGuest::new_shell(ROM, "rom/main.lua").expect("shell compiles");
    Console::with_shell(
        Box::new(shell),
        entries,
        Settings::default(),
        opener,
        Rc::new(LuaGuest::factory),
        kuula_core::Preload::Decode,
    )
}

fn press(c: &mut Console, bits: u16) {
    c.step(FrameInput::new(bits));
    c.step(FrameInput::NONE);
}

#[test]
fn first_startup_a_never_launches_a_cart_even_after_waiting() {
    for delay in [2, 90, 92, 180] {
        let mut c = shell_console(&[("hello", HELLO)]);
        steps(&mut c, delay);
        c.step(FrameInput::new(BTN_A));
        assert_eq!(
            c.frame(),
            0,
            "first A launched a cart after waiting {delay} frames"
        );
        steps_with(&mut c, BTN_A, 20);
        assert_eq!(
            c.frame(),
            0,
            "held first A launched the cart after startup delay {delay}"
        );
        c.step(FrameInput::NONE);
        press(&mut c, BTN_A);
        assert!(
            c.frame() > 0,
            "a fresh second A should run the selected cart"
        );
    }
}

fn steps(c: &mut Console, n: usize) {
    steps_with(c, 0, n);
}

fn steps_with(c: &mut Console, bits: u16, n: usize) {
    for _ in 0..n {
        c.step(FrameInput::new(bits));
    }
}

/// Boot with A, pick the first cart with A.
fn boot_and_run(c: &mut Console) {
    steps(c, 2);
    press(c, BTN_A);
    assert!(c.shell_fault().is_none(), "{:?}", c.shell_fault());
    press(c, BTN_A);
    assert!(c.shell_fault().is_none(), "{:?}", c.shell_fault());
}

#[test]
fn boot_screen_draws_and_the_list_runs_a_cart() {
    let mut c = shell_console(&[("hello", HELLO)]);
    // The shell's first step runs its chunk and `_init`; the second draws.
    c.step(FrameInput::NONE);
    let out = c.step(FrameInput::NONE);
    assert_eq!(out.frame, 0, "no cart yet");
    assert!(out.screen.contains(&7), "boot text is drawn");
    boot_and_run(&mut c);
    assert!(c.frame() >= 1, "the cart runs");
    let before = c.frame();
    steps(&mut c, 5);
    assert_eq!(c.frame(), before + 5);
    let out = c.output();
    assert!(
        out.screen.iter().all(|&p| p == 2),
        "the cart shows through a transparent overlay"
    );
}

#[test]
fn menu_pauses_and_the_cart_never_sees_it() {
    let mut c = shell_console(&[("hello", HELLO)]);
    boot_and_run(&mut c);
    press(&mut c, BTN_MENU);
    assert!(c.is_paused());
    let frozen = c.frame();
    steps(&mut c, 3);
    assert_eq!(c.frame(), frozen, "a paused cart does not step");
    let out = c.output();
    assert!(out.screen.contains(&1), "the pause panel is drawn");
    assert!(out.audio.iter().all(|&s| s == 0), "paused audio is silence");
    // Resume is the first item.
    // Resume applies after the shell's step, so the cart runs again from
    // the frame after the press.
    press(&mut c, BTN_A);
    assert!(!c.is_paused());
    assert_eq!(c.frame(), frozen + 1);
    steps(&mut c, 2);
    assert_eq!(c.frame(), frozen + 3);
    let dump = c.state_dump(&["n".to_string()]).unwrap();
    assert!(dump.contains("n = "), "{dump}");
}

#[test]
fn settings_change_scale_through_a_host_request() {
    let mut c = shell_console(&[("hello", HELLO)]);
    boot_and_run(&mut c);
    press(&mut c, BTN_MENU);
    press(&mut c, kuula_core::input::BTN_DOWN);
    press(&mut c, kuula_core::input::BTN_DOWN);
    press(&mut c, BTN_A); // settings
    press(&mut c, kuula_core::input::BTN_RIGHT); // scale 2 -> 3
    assert_eq!(c.settings().scale, 3);
    assert_eq!(c.take_host_requests(), vec![SysRequest::SetScale(3)]);
    press(&mut c, kuula_core::input::BTN_DOWN);
    press(&mut c, kuula_core::input::BTN_LEFT); // volume 100 -> 90
    assert_eq!(c.settings().volume, 90);
    assert_eq!(c.draw_state().audio.master(), 90);
}

#[test]
fn a_faulting_cart_gets_the_shells_error_screen_and_restarts() {
    let mut c = shell_console(&[("dies", DIES)]);
    boot_and_run(&mut c);
    steps(&mut c, 4);
    let fault = c.state().fault().cloned().expect("cart faulted");
    assert_eq!(fault.code, "runtime_error");
    let out = c.output();
    assert!(out.screen.contains(&15), "the code is drawn in peach");
    assert!(c.shell_fault().is_none());
    press(&mut c, BTN_A);
    assert!(c.state().fault().is_none(), "A restarts the cart");
    assert!(c.frame() <= 2);
    steps(&mut c, 4);
    assert!(c.state().fault().is_some(), "and it dies again");
    press(&mut c, BTN_B);
    assert!(c.state().fault().is_none(), "B quits to the list");
    assert_eq!(c.frame(), 0);
}

/// The host's reload path (a deployed cart): `load_cart` on a console that
/// is already running a cart tears that guest down and starts the new one
/// from frame 0, over a running cart, over a faulted one, and with a new
/// cart that cannot start.
#[test]
fn load_cart_replaces_a_running_cart_and_starts_the_new_one() {
    const NEXT: &str = "n = 0\nfunction _update(dt) n = n + 1 end\nfunction _draw() cls(5) end\n";
    let load = |c: &mut Console, src: &str| {
        c.load_cart(
            cart(&[("main.lua", src.as_bytes())]) as Rc<dyn kuula_core::CartSource>,
            Box::new(MemoryStore::new()),
        );
    };
    let mut c = shell_console(&[("hello", HELLO)]);
    boot_and_run(&mut c);
    steps(&mut c, 5);
    assert!(c.frame() >= 5);
    assert!(
        c.output().screen.iter().all(|&p| p == 2),
        "the old cart draws"
    );

    load(&mut c, NEXT);
    assert!(c.state().fault().is_none(), "{:?}", c.state());
    assert_eq!(c.frame(), 0, "the new cart starts from its first frame");
    steps(&mut c, 3);
    assert_eq!(c.frame(), 3);
    assert!(
        c.output().screen.iter().all(|&p| p == 5),
        "the new cart draws"
    );
    assert!(c.shell_fault().is_none());
    assert!(!c.is_paused());

    // Over a faulted cart.
    load(&mut c, "function _update(dt) error('bang') end");
    steps(&mut c, 2);
    assert!(c.state().fault().is_some(), "the cart faulted");
    load(&mut c, NEXT);
    assert!(
        c.state().fault().is_none(),
        "a fresh cart replaces the faulted one"
    );
    steps(&mut c, 2);
    assert_eq!(c.frame(), 2);

    // A new cart that cannot start is a fault the host can report.
    load(&mut c, "this is not lua");
    let fault = c.state().fault().cloned().expect("the new cart faulted");
    assert!(!fault.code.is_empty());
    assert!(c.shell_fault().is_none(), "the shell itself is fine");
}

#[test]
fn the_cart_has_no_sys_and_the_shell_has_no_cart_authority_leak() {
    let mut c = shell_console(&[(
        "probe",
        "function _draw() if sys then error('leak') end cls(3) end",
    )]);
    boot_and_run(&mut c);
    steps(&mut c, 2);
    assert!(c.state().fault().is_none(), "{:?}", c.state());
}

#[test]
fn quit_from_the_pause_menu_returns_to_the_list() {
    let mut c = shell_console(&[("hello", HELLO)]);
    boot_and_run(&mut c);
    press(&mut c, BTN_MENU);
    for _ in 0..3 {
        press(&mut c, kuula_core::input::BTN_DOWN);
    }
    press(&mut c, BTN_A);
    assert_eq!(c.frame(), 0, "no cart");
    assert!(!c.is_paused());
    let out = c.output();
    assert!(out.screen.contains(&12), "the list highlight is drawn");
}

#[test]
fn buttons_held_through_boot_do_not_move_the_list() {
    let mut c = shell_console(&[
        ("one", "function _draw() cls(2) end"),
        ("two", "function _draw() cls(3) end"),
    ]);
    steps(&mut c, 2);
    // Up is held from the boot screen into the list; A dismisses boot.
    c.step(FrameInput::new(kuula_core::input::BTN_UP | BTN_A));
    steps_with(&mut c, kuula_core::input::BTN_UP, 3);
    c.step(FrameInput::NONE);
    press(&mut c, BTN_A);
    assert!(c.shell_fault().is_none(), "{:?}", c.shell_fault());
    steps(&mut c, 2);
    let out = c.output();
    assert!(
        out.screen.iter().all(|&p| p == 2),
        "the first cart runs; a held Up is not a fresh press"
    );
}

#[test]
fn a_huge_fault_message_does_not_kill_the_shell() {
    let mut c = shell_console(&[(
        "loud",
        "function _update() error(('word '):rep(40000)) end\nfunction _draw() end",
    )]);
    boot_and_run(&mut c);
    steps(&mut c, 4);
    assert!(c.state().fault().is_some(), "the cart died");
    assert!(c.shell_fault().is_none(), "{:?}", c.shell_fault());
    let out = c.output();
    assert!(out.screen.contains(&15), "the error screen is drawn");
    press(&mut c, BTN_B);
    assert!(c.state().fault().is_none(), "B still quits to the list");
}

fn network_console() -> Console {
    let source = cart(&[
        (
            "main.lua",
            br#"
n = 0; presses = 0; invitation = ''
function _init() invitation = net.invite() or 'host' end
function _update()
 n = n + 1
 if n == 1 then if net.invite() then net.join(net.invite()) else net.host() end end
 if btn(4) then presses = presses + 1 end
 while net.recv() do end
end
function _draw() cls(2) end
"#,
        ),
        ("cart.toml", b"[cart]\nservices = [\"net\"]\n"),
    ]);
    let opener: kuula_core::console::CartOpener =
        Rc::new(move |_| Ok((source.clone(), Box::new(MemoryStore::new()))));
    Console::with_shell(
        Box::new(LuaGuest::new_shell(ROM, "rom/main.lua").unwrap()),
        vec![CartEntry {
            name: "net".into(),
            title: "Network cart".into(),
            network: true,
            ..Default::default()
        }],
        Settings::default(),
        opener,
        Rc::new(LuaGuest::factory),
        kuula_core::Preload::Decode,
    )
}

fn network_menu(c: &mut Console) {
    steps(c, 2);
    press(c, BTN_A);
    press(c, BTN_A);
    assert!(c.network_view().unwrap().overlay);
    assert_eq!(c.frame(), 0);
}

#[test]
fn a_host_without_networking_says_so_and_offers_only_back() {
    let mut c = network_console();
    network_menu(&mut c);
    c.network_view_mut().unwrap().unavailable = "no networking here".into();
    // "host game" would have asked for permission or started the cart.
    press(&mut c, BTN_A);
    assert!(!c.network_view().unwrap().overlay, "A goes back");
    assert_eq!(c.frame(), 0, "no cart was started");
}

#[test]
fn the_host_can_correct_the_scale_the_settings_show() {
    let mut c = network_console();
    steps(&mut c, 2);
    assert_eq!(c.settings().scale, Settings::default().scale);
    c.set_effective_scale(1);
    assert_eq!(c.settings().scale, 1);
}

#[test]
fn network_permission_host_cancel_retry_and_gameplay_input_isolation() {
    use kuula_core::net::{Event, Status};
    let mut c = network_console();
    network_menu(&mut c);
    press(&mut c, BTN_A); // host -> permission
    assert!(!c.settings().net);
    assert_eq!(c.frame(), 0);
    press(&mut c, BTN_A); // approve -> menu
    assert!(c.settings().net);
    press(&mut c, BTN_A); // host
    steps(&mut c, 3);
    assert_eq!(c.net_state().unwrap().status, Status::Hosting);
    let frame = c.frame();
    steps(&mut c, 3);
    assert!(
        c.frame() > frame,
        "connection overlay must keep the cart stepping"
    );
    c.step_with(FrameInput::NONE, vec![Event::Connected { peer: 1 }]);
    c.step(FrameInput::new(BTN_A)); // enter game
    steps_with(&mut c, BTN_A, 3);
    assert!(!c.network_view().unwrap().overlay);
    assert!(c
        .state_dump(&["presses".into()])
        .unwrap()
        .contains("presses = 0"));
    c.step(FrameInput::NONE);
    press(&mut c, BTN_A);
    assert!(c
        .state_dump(&["presses".into()])
        .unwrap()
        .contains("presses = 1"));
    c.step_with(
        FrameInput::NONE,
        vec![Event::Disconnected {
            reason: kuula_core::net::Reason::Lost,
        }],
    );
    steps(&mut c, 2);
    assert!(c.network_view().unwrap().overlay);
    press(&mut c, BTN_B);
    assert_eq!(c.frame(), 0);
    press(&mut c, BTN_A);
    steps(&mut c, 3);
    assert_eq!(c.net_state().unwrap().status, Status::Hosting);
    assert!(c.network_view().unwrap().detail.is_empty());
    assert!(c.shell_fault().is_none(), "{:?}", c.shell_fault());
}

/// A cart the host starts while the shell is on a network screen gets
/// its buttons: the overlay that screen held goes with it.
#[test]
fn a_host_started_cart_gets_input_from_a_network_screen() {
    const COUNTS: &str = r#"
presses = 0
function _update(dt) if btn(4) then presses = presses + 1 end end
function _draw() cls(5) end
"#;
    let mut c = network_console();
    network_menu(&mut c);
    c.host_load_cart(
        cart(&[("main.lua", COUNTS.as_bytes())]) as Rc<dyn kuula_core::CartSource>,
        Box::new(MemoryStore::new()),
    );
    steps(&mut c, 2);
    assert!(
        !c.network_view().unwrap().overlay,
        "the network screen's overlay is gone"
    );
    assert!(c.output().screen.iter().all(|&p| p == 5), "the cart shows");
    press(&mut c, BTN_A);
    assert!(c
        .state_dump(&["presses".into()])
        .unwrap()
        .contains("presses = 1"));
    assert!(c.shell_fault().is_none(), "{:?}", c.shell_fault());
}

#[test]
fn ticket_entry_and_relay_menu_draw_without_shell_faults() {
    use kuula_core::input::BTN_DOWN;
    let mut c = network_console();
    network_menu(&mut c);
    press(&mut c, BTN_A);
    press(&mut c, BTN_A); // permission
    press(&mut c, BTN_DOWN);
    press(&mut c, BTN_A); // ticket entry
    c.network_view_mut().unwrap().text = "example-ticket".into();
    steps(&mut c, 2);
    press(&mut c, BTN_A); // done, launches with invite
    steps(&mut c, 3);
    assert_eq!(
        c.net_state().unwrap().invite.as_deref(),
        Some("example-ticket")
    );
    assert!(c
        .state_dump(&["invitation".into()])
        .unwrap()
        .contains("example-ticket"));
    press(&mut c, BTN_B);
    assert_eq!(c.frame(), 0);
    press(&mut c, BTN_DOWN);
    press(&mut c, BTN_DOWN);
    press(&mut c, BTN_A); // relay entry
    steps(&mut c, 2);
    assert!(c.shell_fault().is_none(), "{:?}", c.shell_fault());
}

#[test]
fn a_shell_on_one_cart_opens_it_at_once_and_quitting_ends_the_host() {
    use kuula_core::input::BTN_DOWN;
    let mut c = shell_console(&[("hello", HELLO)]);
    c.set_single_cart(true);
    steps(&mut c, 3);
    assert!(c.shell_fault().is_none(), "{:?}", c.shell_fault());
    assert!(c.frame() > 0, "the cart runs with no button pressed");
    assert!(!c.take_host_requests().contains(&SysRequest::Exit));
    // Menu, three down, A: the item that is "quit to shell" with a list.
    press(&mut c, BTN_MENU);
    assert!(c.is_paused());
    for _ in 0..3 {
        press(&mut c, BTN_DOWN);
    }
    c.step(FrameInput::new(BTN_A));
    assert!(c.take_host_requests().contains(&SysRequest::Exit));
    assert!(c.shell_fault().is_none(), "{:?}", c.shell_fault());

    // B on the error screen of a cart that died is the way out too.
    let mut c = shell_console(&[("dies", DIES)]);
    c.set_single_cart(true);
    steps(&mut c, 8);
    assert!(c.state().fault().is_some());
    assert!(!c.take_host_requests().contains(&SysRequest::Exit));
    c.step(FrameInput::new(BTN_B));
    assert!(c.take_host_requests().contains(&SysRequest::Exit));

    // A shell with a list never asks to end.
    let mut c = shell_console(&[("hello", HELLO)]);
    boot_and_run(&mut c);
    press(&mut c, BTN_MENU);
    for _ in 0..3 {
        press(&mut c, BTN_DOWN);
    }
    press(&mut c, BTN_A);
    assert_eq!(c.frame(), 0, "back on the list");
    assert!(!c.take_host_requests().contains(&SysRequest::Exit));
}

#[test]
fn a_developer_waiting_for_approval_is_put_to_the_person() {
    use kuula_core::shell::DevAction;
    let id = "0123456789abcdef".repeat(4);
    // What the host does when its receiver has refused a developer.
    let ask = |c: &mut Console| {
        let view = c.dev_view_mut().unwrap();
        view.active = true;
        view.pending = id.clone();
        view.cart = "hello".into();
        view.bytes = 736;
        view.from = "192.0.2.5".into();
    };
    let answers = |c: &mut Console| -> Vec<SysRequest> {
        c.take_host_requests()
            .into_iter()
            .filter(|r| matches!(r, SysRequest::Dev(_)))
            .collect()
    };

    // Over a running cart: the cart waits while the question is up.
    let mut c = shell_console(&[("hello", HELLO)]);
    boot_and_run(&mut c);
    assert!(!c.is_paused());
    ask(&mut c);
    steps(&mut c, 2);
    assert!(c.is_paused(), "the cart waits while the question is up");
    assert!(c.output().screen.contains(&15), "the id is drawn");
    // A tapped as in a game answers nothing, at once or later.
    for _ in 0..80 {
        press(&mut c, BTN_A);
    }
    assert_eq!(answers(&mut c), []);
    // A pressed after the first second and a half and held for a second
    // approves, and not a frame before.
    steps_with(&mut c, BTN_A, 59);
    assert_eq!(answers(&mut c), []);
    c.step(FrameInput::new(BTN_A));
    assert_eq!(answers(&mut c), [SysRequest::Dev(DevAction::Approve)]);
    // The host takes the question away with the answer.
    c.dev_view_mut().unwrap().pending.clear();
    steps(&mut c, 2);
    assert!(!c.is_paused(), "the cart goes on");
    assert!(c.shell_fault().is_none(), "{:?}", c.shell_fault());

    // A that was already held when the question came up never approves.
    let mut c = shell_console(&[("hello", HELLO)]);
    boot_and_run(&mut c);
    steps_with(&mut c, BTN_A, 5);
    ask(&mut c);
    steps_with(&mut c, BTN_A, 400);
    assert_eq!(answers(&mut c), []);

    // From the list: B refuses, once the first second and a half is over,
    // and the list is back.
    let mut c = shell_console(&[("hello", HELLO)]);
    steps(&mut c, 2);
    press(&mut c, BTN_A);
    ask(&mut c);
    steps(&mut c, 2);
    press(&mut c, BTN_B);
    assert_eq!(answers(&mut c), [], "too soon to be meant");
    steps(&mut c, 95);
    c.step(FrameInput::new(BTN_B));
    assert_eq!(answers(&mut c), [SysRequest::Dev(DevAction::Refuse)]);
    c.dev_view_mut().unwrap().pending.clear();
    steps(&mut c, 2);
    press(&mut c, BTN_A);
    assert!(c.frame() > 0, "A on the list runs the cart again");
    assert!(c.shell_fault().is_none(), "{:?}", c.shell_fault());

    // A shell with no receiver is never asked, whatever the view holds.
    let mut c = shell_console(&[("hello", HELLO)]);
    boot_and_run(&mut c);
    c.dev_view_mut().unwrap().pending = id.clone();
    steps(&mut c, 3);
    assert!(!c.is_paused());
}
