//! The real ROM and multiplayer cart driven through two shell instances.
use kuula_core::input::{BTN_A, BTN_B, BTN_DOWN, BTN_RIGHT};
use kuula_core::net::{Link, MemoryTransport, NetEnv, Status, MEMORY_TICKET};
use kuula_core::shell::{CartEntry, Settings};
use kuula_core::{Console, FrameInput, MemoryStore, Preload, Snapshot, SnapshotLimits};
use kuula_lua::LuaGuest;
use std::{path::PathBuf, rc::Rc};

fn console() -> Console {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let snap =
        Snapshot::from_dir(&root.join("examples/marbles"), SnapshotLimits::default()).unwrap();
    console_with(snap)
}
/// A shell over one network cart, `snap`, listed as "marbles".
fn console_with(snap: Snapshot) -> Console {
    let snap = Rc::new(snap);
    let opener: kuula_core::console::CartOpener =
        Rc::new(move |_| Ok((snap.clone(), Box::new(MemoryStore::new()))));
    let shell = LuaGuest::new_shell(include_str!("../../../rom/main.lua"), "rom/main.lua").unwrap();
    let mut c = Console::with_shell(
        Box::new(shell),
        vec![CartEntry {
            name: "marbles".into(),
            title: "Marble Duel".into(),
            network: true,
            ..Default::default()
        }],
        Settings {
            net: true,
            ..Settings::default()
        },
        opener,
        Rc::new(LuaGuest::factory),
        Preload::Decode,
    );
    c.set_net_env(NetEnv {
        permitted: true,
        invite: None,
    });
    c
}
fn capture(c: &Console, name: &str) {
    if let Some(dir) = std::env::var_os("KUULA_SHELL_CAPTURE_DIR") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{name}.png")),
            kuula_host_headless::frame_png(&kuula_host_headless::owned_frame(c)),
        )
        .unwrap();
    }
}
fn press(c: &mut Console, key: u8) {
    c.step(FrameInput::new(key));
    c.step(FrameInput::NONE);
}
fn menu(c: &mut Console) {
    c.step(FrameInput::NONE);
    press(c, BTN_A);
    press(c, BTN_A);
}
fn tick(a: &mut Console, al: &mut Link, b: &mut Console, bl: &mut Link, ai: u8, bi: u8) {
    a.step_linked(al, FrameInput::new(ai));
    b.step_linked(bl, FrameInput::new(bi));
    assert!(a.state().fault().is_none(), "{:?}", a.state());
    assert!(b.state().fault().is_none(), "{:?}", b.state());
    assert!(a.shell_fault().is_none(), "{:?}", a.shell_fault());
    assert!(b.shell_fault().is_none(), "{:?}", b.shell_fault());
}
#[test]
fn two_shells_host_join_play_a_round_and_cancel() {
    let mut a = console();
    let mut b = console();
    menu(&mut a);
    menu(&mut b);
    capture(&a, "network-menu");
    press(&mut b, BTN_DOWN);
    press(&mut b, BTN_A);
    capture(&b, "ticket-entry");
    b.network_view_mut().unwrap().text = MEMORY_TICKET.into();
    // Launch on the A down frame; the first cart frame then runs through its link.
    a.step(FrameInput::new(BTN_A));
    b.step(FrameInput::new(BTN_A));
    let (at, bt) = MemoryTransport::pair();
    let mut al = Link::over(Box::new(at));
    let mut bl = Link::over(Box::new(bt));
    for _ in 0..12 {
        tick(&mut a, &mut al, &mut b, &mut bl, 0, 0);
    }
    assert_eq!(a.net_state().unwrap().status, Status::Connected);
    assert_eq!(b.net_state().unwrap().status, Status::Connected);
    capture(&a, "connected");
    tick(&mut a, &mut al, &mut b, &mut bl, BTN_A, BTN_A);
    tick(&mut a, &mut al, &mut b, &mut bl, 0, 0);
    for _ in 0..2 {
        tick(&mut a, &mut al, &mut b, &mut bl, BTN_RIGHT, BTN_RIGHT);
        tick(&mut a, &mut al, &mut b, &mut bl, 0, 0);
    }
    for turn in 0..5 {
        let (ai, bi) = if turn % 2 == 0 {
            (BTN_A, 0)
        } else {
            (0, BTN_A)
        };
        tick(&mut a, &mut al, &mut b, &mut bl, ai, bi);
        for _ in 0..6 {
            tick(&mut a, &mut al, &mut b, &mut bl, 0, 0);
        }
    }
    let state = a.state_dump(&["game".into()]).unwrap();
    assert!(state.contains("winner = 1"), "{state}");
    capture(&a, "won");
    bl.set_permitted(false);
    for _ in 0..5 {
        tick(&mut a, &mut al, &mut b, &mut bl, 0, 0);
    }
    assert!(a.network_view().unwrap().overlay);
    capture(&a, "disconnected");
    tick(&mut a, &mut al, &mut b, &mut bl, BTN_B, 0);
    assert_eq!(a.frame(), 0);
}

/// The 640x480 network overlay draws in the 8x8 face on a 320x240
/// canvas over a running cart, without a shell fault and without
/// touching the cart's own pen: a cart on `font(8)` keeps it.
#[test]
fn network_overlay_draws_at_640_over_a_running_cart() {
    let source = br#"
function _init()
  font(8)
  if net.invite() == nil then net.host() else net.join(net.invite()) end
end
function _update() end
function _draw()
  seen = stat("font")
  cls(0)
  print("x", 0, 0)
end
"#;
    let make = || {
        Snapshot::from_entries(
            [
                ("main.lua", source.to_vec()),
                (
                    "cart.toml",
                    b"[cart]\nservices=['net']\nscreen_mode='640x480'".to_vec(),
                ),
            ],
            SnapshotLimits::default(),
        )
        .unwrap()
    };
    let mut a = console_with(make());
    let mut b = console_with(make());
    menu(&mut a);
    menu(&mut b);
    press(&mut b, BTN_DOWN);
    press(&mut b, BTN_A);
    b.network_view_mut().unwrap().text = MEMORY_TICKET.into();
    a.step(FrameInput::new(BTN_A));
    b.step(FrameInput::new(BTN_A));
    let (at, bt) = MemoryTransport::pair();
    let mut al = Link::over(Box::new(at));
    let mut bl = Link::over(Box::new(bt));
    for _ in 0..12 {
        tick(&mut a, &mut al, &mut b, &mut bl, 0, 0);
    }
    assert_eq!(a.net_state().unwrap().status, Status::Connected);
    // Still on the connecting overlay, over a running 640x480 cart.
    assert!(a.network_view().unwrap().overlay);
    assert_eq!(a.output().width, 640);
    let state = a.state_dump(&["seen".into()]).unwrap();
    assert!(state.contains("seen = 8"), "{state}");
    capture(&a, "connecting-640");
}

/// Every shell screen draws without a shell fault: at 640x480 in the
/// 8x16 face, and over a 320x240 cart in the 8x8 one. Set
/// `KUULA_SHELL_CAPTURE_DIR` to look at them.
#[test]
fn every_screen_draws_in_both_faces() {
    use kuula_core::input::BTN_MENU;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let load = |name: &str| {
        Rc::new(
            Snapshot::from_dir(&root.join("examples").join(name), SnapshotLimits::default())
                .unwrap(),
        )
    };
    let snaps = [
        ("hello", load("hello")),
        ("broken", load("broken")),
        ("spritetest", load("spritetest")),
    ];
    let opener: kuula_core::console::CartOpener = Rc::new(move |name| {
        let snap = snaps
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, s)| s.clone())
            .unwrap();
        Ok((snap, Box::new(MemoryStore::new())))
    });
    let shell = LuaGuest::new_shell(include_str!("../../../rom/main.lua"), "rom/main.lua").unwrap();
    let entry = |name: &str, title: &str| CartEntry {
        name: name.into(),
        title: title.into(),
        author: "Kuula".into(),
        ..Default::default()
    };
    let mut c = Console::with_shell(
        Box::new(shell),
        vec![
            entry("hello", "Hello"),
            entry("broken", "Breaks on frame 60"),
            entry("spritetest", "Alien sprite test"),
        ],
        Settings::default(),
        opener,
        Rc::new(LuaGuest::factory),
        Preload::Decode,
    );
    let check = |c: &mut Console, name: &str, width: u32| {
        assert!(c.shell_fault().is_none(), "{name}: {:?}", c.shell_fault());
        assert_eq!(c.output().width, width, "{name}");
        capture(c, name);
    };
    c.step(FrameInput::NONE);
    c.step(FrameInput::NONE);
    check(&mut c, "boot", 640);
    press(&mut c, BTN_A);
    check(&mut c, "list", 640);
    press(&mut c, BTN_B);
    check(&mut c, "info", 640);
    press(&mut c, BTN_B);
    // hello runs at 320x240; the overlay follows it.
    press(&mut c, BTN_A);
    for _ in 0..5 {
        c.step(FrameInput::NONE);
    }
    check(&mut c, "cart-hello", 320);
    press(&mut c, BTN_MENU);
    check(&mut c, "pause-320", 320);
    press(&mut c, BTN_DOWN);
    press(&mut c, BTN_DOWN);
    press(&mut c, BTN_A);
    check(&mut c, "settings-320", 320);
    press(&mut c, BTN_B);
    press(&mut c, BTN_B);
    press(&mut c, BTN_MENU);
    for _ in 0..3 {
        press(&mut c, BTN_DOWN);
    }
    press(&mut c, BTN_A);
    check(&mut c, "list-again", 640);
    // broken dies on its frame 60.
    press(&mut c, BTN_DOWN);
    press(&mut c, BTN_A);
    for _ in 0..70 {
        c.step(FrameInput::NONE);
    }
    assert!(c.state().fault().is_some(), "{:?}", c.state());
    check(&mut c, "error-320", 320);
    press(&mut c, BTN_B);
    check(&mut c, "list-after-error", 640);
    // The list stays on broken; spritetest is one down and runs at
    // 640x480, so the overlay uses the 8x16 face.
    press(&mut c, BTN_DOWN);
    press(&mut c, BTN_A);
    for _ in 0..3 {
        c.step(FrameInput::NONE);
    }
    assert!(c.state().fault().is_none(), "{:?}", c.state());
    press(&mut c, BTN_MENU);
    check(&mut c, "pause-640", 640);
    press(&mut c, BTN_DOWN);
    press(&mut c, BTN_DOWN);
    press(&mut c, BTN_A);
    check(&mut c, "settings-640", 640);
    press(&mut c, BTN_B);
    press(&mut c, BTN_B);
    c.step(FrameInput::NONE);
    assert!(c.shell_fault().is_none());
}
