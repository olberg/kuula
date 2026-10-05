use super::*;
use crate::snapshot::{Snapshot, SnapshotLimits};

struct Stub {
    fail_on: Option<u64>,
}

impl Guest for Stub {
    fn step(&mut self, state: &mut DrawState, input: FrameInput, frame: u64) -> Result<(), Fault> {
        if self.fail_on == Some(frame) {
            return Err(Fault::new(
                Fault::RUNTIME_ERROR,
                "main.lua",
                Some(3),
                "boom",
            ));
        }
        state.cls(frame as u8);
        state
            .log
            .push(format!("frame {frame} buttons {}", input.buttons));
        Ok(())
    }
}

fn cart(entries: Vec<(&str, &[u8])>) -> Rc<dyn CartSource> {
    Rc::new(
        Snapshot::from_entries(
            entries.into_iter().map(|(k, v)| (k, v.to_vec())),
            SnapshotLimits::default(),
        )
        .unwrap(),
    )
}

#[test]
fn first_step_is_frame_one_with_a_full_size_screen() {
    let mut c = Console::from_guest(Box::new(Stub { fail_on: None }));
    assert_eq!(c.output().frame, 0);
    let out = c.step(FrameInput::new(0b1));
    assert_eq!(out.frame, 1);
    assert_eq!((out.width, out.height), (640, 480));
    assert_eq!(out.screen.len(), 640 * 480);
    assert_eq!(out.palette.len(), PALETTE_SIZE);
    assert!(out.screen.iter().all(|&p| p == 1));
    assert_eq!(out.log, ["frame 1 buttons 1"]);
    assert_eq!(*c.state(), ConsoleState::Running);
}

#[test]
fn log_is_per_frame() {
    let mut c = Console::from_guest(Box::new(Stub { fail_on: None }));
    c.step(FrameInput::NONE);
    let out = c.step(FrameInput::NONE);
    assert_eq!(out.log, ["frame 2 buttons 0"]);
}

#[test]
fn a_fault_freezes_the_console() {
    let mut c = Console::from_guest(Box::new(Stub { fail_on: Some(3) }));
    c.step(FrameInput::NONE);
    c.step(FrameInput::NONE);
    let out = c.step(FrameInput::NONE);
    assert_eq!(out.frame, 3);
    assert!(
        out.screen.iter().all(|&p| p == 2),
        "screen from frame 2 survives"
    );
    let fault = c.state().fault().cloned().expect("faulted");
    assert_eq!(fault.code, "runtime_error");
    assert_eq!(fault.location(), "main.lua:3");
    let out = c.step(FrameInput::NONE);
    assert_eq!(out.frame, 3, "step after a fault is a no-op");
    assert!(out.screen.iter().all(|&p| p == 2));
}

#[test]
fn the_faulting_frames_log_is_returned_once() {
    struct LogThenFail;
    impl Guest for LogThenFail {
        fn step(&mut self, state: &mut DrawState, _: FrameInput, _: u64) -> Result<(), Fault> {
            state.log.push("tick".to_string());
            Err(Fault::new(
                Fault::RUNTIME_ERROR,
                "main.lua",
                Some(1),
                "bang",
            ))
        }
    }
    let mut c = Console::from_guest(Box::new(LogThenFail));
    assert_eq!(c.step(FrameInput::NONE).log, ["tick"]);
    assert!(c.step(FrameInput::NONE).log.is_empty());
    assert!(c.output().log.is_empty());
}

#[test]
fn new_hands_main_lua_to_the_factory() {
    let src = cart(vec![("main.lua", b"return 1")]);
    let mut seen = None;
    let c = Console::new(src, |text, name| {
        seen = Some((text.to_string(), name.to_string()));
        Ok(Box::new(Stub { fail_on: None }) as Box<dyn Guest>)
    });
    assert_eq!(seen, Some(("return 1".to_string(), "main.lua".to_string())));
    assert_eq!(*c.state(), ConsoleState::Running);
    assert_eq!(c.screen_mode(), ScreenMode::High);
}

#[test]
fn new_reports_a_missing_main_lua_as_a_fault() {
    let src = cart(vec![]);
    let c = Console::new(src, |_, _| panic!("factory must not run"));
    let fault = c.state().fault().expect("faulted");
    assert_eq!(fault.code, "cart_read_error");
    assert_eq!(fault.file, "main.lua");
}

#[test]
fn new_reports_a_compile_error_as_a_fault() {
    let src = cart(vec![("main.lua", b"x = = 1")]);
    let mut c = Console::new(src, |_, name| {
        Err(Fault::new(
            Fault::COMPILE_ERROR,
            name,
            Some(1),
            "unexpected symbol",
        ))
    });
    let fault = c.state().fault().cloned().expect("faulted");
    assert_eq!(fault.code, "compile_error");
    assert_eq!(fault.location(), "main.lua:1");
    assert_eq!(c.step(FrameInput::NONE).frame, 0);
}

#[test]
fn manifest_selects_the_screen_mode_and_errors_are_faults() {
    let src = cart(vec![
        ("main.lua", b""),
        ("cart.toml", b"[cart]\nscreen_mode = \"320x240\"\n"),
    ]);
    let mut c = Console::new(src, |_, _| Ok(Box::new(Stub { fail_on: None })));
    let out = c.step(FrameInput::NONE);
    assert_eq!((out.width, out.height), (320, 240));
    assert_eq!(c.screen_mode(), ScreenMode::Low);

    let src = cart(vec![
        ("main.lua", b""),
        ("cart.toml", b"[cart]\nbogus = 1\n"),
    ]);
    let c = Console::new(src, |_, _| panic!("factory must not run"));
    let fault = c.state().fault().expect("faulted");
    assert_eq!(fault.code, "manifest_error");
    assert_eq!(fault.file, "cart.toml");
    assert_eq!(fault.line, Some(2));
    assert!(fault.message.contains("bogus"), "{}", fault.message);
}

#[test]
fn preload_decodes_at_boot_and_a_missing_asset_is_a_fault() {
    let png = crate::assets::encode_indexed_png(8, 8, &[1; 64], &crate::palette::DEFAULT_PALETTE);
    let src = cart(vec![
        ("main.lua", b""),
        ("cart.toml", b"[preload]\nsheets = [\"hero\"]\n"),
        ("gfx/hero.png", &png),
    ]);
    let c = Console::new(src, |_, _| Ok(Box::new(Stub { fail_on: None })));
    assert_eq!(*c.state(), ConsoleState::Running);
    assert_eq!(c.draw_state().res.ledger().used(), 64);
    assert!(c.draw_state().res.named("gfx/hero.png").is_some());

    let src = cart(vec![
        ("main.lua", b""),
        ("cart.toml", b"[preload]\nmaps = [\"nope\"]\n"),
    ]);
    let c = Console::new(src, |_, _| panic!("factory must not run"));
    let fault = c.state().fault().expect("faulted");
    assert_eq!(fault.code, "asset_not_found");
    assert_eq!(fault.file, "map/nope.json");
}

#[test]
fn a_shell_that_faults_while_paused_keeps_the_cart_silent() {
    use crate::save::MemoryStore;
    use crate::shell::Settings;

    struct StubShell;
    impl Guest for StubShell {
        fn step(&mut self, state: &mut DrawState, _: FrameInput, frame: u64) -> Result<(), Fault> {
            let sys = state.sys.as_mut().expect("the shell has sys");
            match frame {
                1 => sys.request(SysRequest::Run("noisy".into())),
                2 => sys.request(SysRequest::Paused(true)),
                3 => {
                    return Err(Fault::new(
                        Fault::RUNTIME_ERROR,
                        "rom/main.lua",
                        Some(1),
                        "shell bug",
                    ))
                }
                _ => {}
            }
            Ok(())
        }
    }
    struct Noisy;
    impl Guest for Noisy {
        fn step(&mut self, state: &mut DrawState, _: FrameInput, _: u64) -> Result<(), Fault> {
            state.sfx("beep", Some(0)).expect("the effect loads");
            Ok(())
        }
    }
    let beep = crate::audio::testsong::omc(&crate::audio::testsong::Song::new(1).ticks(600));
    let source = cart(vec![("main.lua", b""), ("sfx/beep.omc", &beep)]);
    let opener: CartOpener = Rc::new(move |_: &str| {
        Ok((
            source.clone(),
            Box::new(MemoryStore::new()) as Box<dyn SaveStore>,
        ))
    });
    let factory: GuestFactory = Rc::new(|_: &str, _: &str| Ok(Box::new(Noisy) as Box<dyn Guest>));
    let mut c = Console::with_shell(
        Box::new(StubShell),
        Vec::new(),
        Settings::default(),
        opener,
        factory,
        Preload::Decode,
    );
    c.step(FrameInput::NONE);
    c.step(FrameInput::NONE);
    assert!(c.is_paused());
    assert!(
        c.draw_state().audio.output().iter().any(|&s| s != 0),
        "the cart rendered a note before the pause"
    );
    let silent = c.step(FrameInput::NONE).audio.iter().all(|&s| s == 0);
    assert!(c.shell_fault().is_some(), "the shell faulted while paused");
    assert!(silent, "a paused cart stays silent after its shell dies");
    let out = c.step(FrameInput::NONE);
    assert!(out.audio.iter().all(|&s| s == 0));
}

#[test]
fn buttons_held_when_the_shell_starts_a_cart_stay_masked_until_released() {
    use crate::input::BTN_A;
    use crate::save::MemoryStore;
    use crate::shell::Settings;
    use std::cell::RefCell;

    struct StubShell;
    impl Guest for StubShell {
        fn step(&mut self, state: &mut DrawState, _: FrameInput, frame: u64) -> Result<(), Fault> {
            let sys = state.sys.as_mut().expect("the shell has sys");
            if frame == 1 {
                sys.request(SysRequest::Run("cart".into()));
            }
            Ok(())
        }
    }
    struct Records(Rc<RefCell<Vec<u16>>>);
    impl Guest for Records {
        fn step(&mut self, _: &mut DrawState, input: FrameInput, _: u64) -> Result<(), Fault> {
            self.0.borrow_mut().push(input.buttons);
            Ok(())
        }
    }
    let seen = Rc::new(RefCell::new(Vec::new()));
    let source = cart(vec![("main.lua", b"")]);
    let opener: CartOpener = Rc::new(move |_: &str| {
        Ok((
            source.clone(),
            Box::new(MemoryStore::new()) as Box<dyn SaveStore>,
        ))
    });
    let recorded = seen.clone();
    let factory: GuestFactory =
        Rc::new(move |_: &str, _: &str| Ok(Box::new(Records(recorded.clone())) as Box<dyn Guest>));
    let mut c = Console::with_shell(
        Box::new(StubShell),
        Vec::new(),
        Settings::default(),
        opener,
        factory,
        Preload::Decode,
    );
    let a = FrameInput::new(BTN_A);
    // A is held while the shell picks the cart and for two more
    // frames after the cart starts.
    c.step(a);
    c.step(a);
    c.step(a);
    c.step(FrameInput::NONE);
    c.step(a);
    assert_eq!(*seen.borrow(), vec![0, 0, 0, BTN_A]);
}

#[test]
fn a_cart_sees_the_buttons_its_manifest_declares_and_a_touch_screen_shows_them() {
    use crate::input::{BASE_BUTTONS, BTN_A, BTN_MENU, BTN_START, BTN_X, CART_BUTTONS};
    use crate::save::MemoryStore;
    use crate::shell::Settings;
    use std::cell::RefCell;

    struct Records(Rc<RefCell<Vec<u16>>>);
    impl Guest for Records {
        fn step(&mut self, _: &mut DrawState, input: FrameInput, _: u64) -> Result<(), Fault> {
            self.0.borrow_mut().push(input.buttons);
            Ok(())
        }
    }
    let held = FrameInput::new(BTN_A | BTN_X | BTN_START | BTN_MENU);
    let all: &[u8] = b"[cart]\nbuttons = \"all\"\n";
    for (manifest, mask) in [(None, BASE_BUTTONS), (Some(all), CART_BUTTONS)] {
        let mut files: Vec<(&str, &[u8])> = vec![("main.lua", b"")];
        files.extend(manifest.map(|m| ("cart.toml", m)));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let recorded = seen.clone();
        let mut c = Console::new(cart(files), move |_: &str, _: &str| {
            Ok(Box::new(Records(recorded)) as Box<dyn Guest>)
        });
        assert_eq!(c.buttons_in_use(), mask);
        c.step(held);
        assert_eq!(*seen.borrow(), vec![held.buttons & mask]);
    }

    // Under a shell: the shell's own six until a cart plays, the cart's
    // while it does, and the six again while the pause menu is up.
    struct StubShell;
    impl Guest for StubShell {
        fn step(&mut self, state: &mut DrawState, _: FrameInput, frame: u64) -> Result<(), Fault> {
            let sys = state.sys.as_mut().expect("the shell has sys");
            if frame == 1 {
                sys.request(SysRequest::Run("cart".into()));
            }
            if sys.menu_pressed {
                sys.request(SysRequest::Paused(true));
            }
            Ok(())
        }
    }
    let source = cart(vec![("main.lua", b""), ("cart.toml", all)]);
    let opener: CartOpener = Rc::new(move |_: &str| {
        Ok((
            source.clone(),
            Box::new(MemoryStore::new()) as Box<dyn SaveStore>,
        ))
    });
    let seen = Rc::new(RefCell::new(Vec::new()));
    let recorded = seen.clone();
    let factory: GuestFactory =
        Rc::new(move |_: &str, _: &str| Ok(Box::new(Records(recorded.clone())) as Box<dyn Guest>));
    let mut c = Console::with_shell(
        Box::new(StubShell),
        Vec::new(),
        Settings::default(),
        opener,
        factory,
        Preload::Decode,
    );
    assert_eq!(c.buttons_in_use(), BASE_BUTTONS);
    c.step(FrameInput::NONE);
    assert_eq!(c.buttons_in_use(), CART_BUTTONS);
    c.step(FrameInput::new(BTN_X));
    assert_eq!(seen.borrow().last(), Some(&BTN_X));
    c.step(FrameInput::new(BTN_MENU));
    assert_eq!(c.buttons_in_use(), BASE_BUTTONS);
}

#[test]
fn start_and_select_are_menu_together_everywhere_and_alone_where_the_cart_has_neither() {
    use crate::input::{
        BASE_BUTTONS, BTN_A, BTN_MENU, BTN_SELECT, BTN_START, BTN_UP, CART_BUTTONS,
    };
    use crate::save::MemoryStore;
    use crate::shell::Settings;
    use std::cell::RefCell;

    type Seen = Rc<RefCell<Vec<u16>>>;
    struct Records(Seen);
    impl Guest for Records {
        fn step(&mut self, _: &mut DrawState, input: FrameInput, _: u64) -> Result<(), Fault> {
            self.0.borrow_mut().push(input.buttons);
            Ok(())
        }
    }
    // A shell that starts the cart, pauses and resumes it on Menu, and
    // notes what it is given.
    struct StubShell(Seen);
    impl Guest for StubShell {
        fn step(
            &mut self,
            state: &mut DrawState,
            input: FrameInput,
            frame: u64,
        ) -> Result<(), Fault> {
            self.0.borrow_mut().push(input.buttons);
            let sys = state.sys.as_mut().expect("the shell has sys");
            if frame == 1 {
                sys.request(SysRequest::Run("cart".into()));
            }
            if sys.menu_pressed {
                sys.request(SysRequest::Paused(!sys.paused));
            }
            Ok(())
        }
    }
    let all: &[u8] = b"[cart]\nbuttons = \"all\"\n";
    let both = BTN_START | BTN_SELECT;
    let step = |c: &mut Console, buttons: u16| {
        c.step(FrameInput::new(buttons));
    };

    // Without a shell: the cart, which has every button, never has both.
    let seen: Seen = Rc::default();
    let recorded = seen.clone();
    let mut c = Console::new(
        cart(vec![("main.lua", b""), ("cart.toml", all)]),
        move |_: &str, _: &str| Ok(Box::new(Records(recorded)) as Box<dyn Guest>),
    );
    for buttons in [BTN_START, both | BTN_UP, BTN_SELECT, 0, BTN_SELECT] {
        step(&mut c, buttons);
    }
    assert_eq!(*seen.borrow(), vec![BTN_START, BTN_UP, 0, 0, BTN_SELECT]);

    let under_shell = |manifest: Option<&'static [u8]>| -> (Console, Seen, Seen) {
        let mut files: Vec<(&str, &[u8])> = vec![("main.lua", b"")];
        files.extend(manifest.map(|m| ("cart.toml", m)));
        let source = cart(files);
        let opener: CartOpener = Rc::new(move |_: &str| {
            Ok((
                source.clone(),
                Box::new(MemoryStore::new()) as Box<dyn SaveStore>,
            ))
        });
        let (cart_seen, shell_seen): (Seen, Seen) = (Rc::default(), Rc::default());
        let recorded = cart_seen.clone();
        let factory: GuestFactory = Rc::new(move |_: &str, _: &str| {
            Ok(Box::new(Records(recorded.clone())) as Box<dyn Guest>)
        });
        let mut c = Console::with_shell(
            Box::new(StubShell(shell_seen.clone())),
            Vec::new(),
            Settings::default(),
            opener,
            factory,
            Preload::Decode,
        );
        c.step(FrameInput::NONE);
        (c, cart_seen, shell_seen)
    };
    let last = |seen: &Seen| *seen.borrow().last().expect("it was stepped");

    // A cart with two buttons: Start alone opens the menu, and the cart
    // is never given it.
    let (mut c, cart_seen, shell_seen) = under_shell(None);
    step(&mut c, BTN_START);
    assert_eq!(c.buttons_in_use(), BASE_BUTTONS, "paused");
    // Still held, it is not a press on the menu that just opened.
    step(&mut c, BTN_START);
    assert_eq!(last(&shell_seen), 0);
    step(&mut c, 0);
    // Pressed again it chooses, as A does.
    step(&mut c, BTN_START);
    assert_eq!(last(&shell_seen), BTN_START | BTN_A);
    step(&mut c, 0);
    step(&mut c, BTN_MENU);
    assert_eq!(c.buttons_in_use(), BASE_BUTTONS, "the cart plays again");
    step(&mut c, 0);
    let before = cart_seen.borrow().len();
    step(&mut c, BTN_SELECT | BTN_A);
    assert_eq!(cart_seen.borrow().len(), before + 1);
    assert_eq!(last(&cart_seen), BTN_A, "Select is not the cart's");
    step(&mut c, 0);
    assert_eq!(
        cart_seen.borrow().len(),
        before + 1,
        "and it paused the cart"
    );

    // A cart with every button: Start alone is its own, and the two
    // together are Menu.
    let (mut c, cart_seen, _) = under_shell(Some(all));
    step(&mut c, BTN_START);
    assert_eq!(last(&cart_seen), BTN_START);
    assert_eq!(c.buttons_in_use(), CART_BUTTONS, "not paused");
    step(&mut c, both);
    assert_eq!(c.buttons_in_use(), BASE_BUTTONS, "paused");
    assert!(cart_seen.borrow().iter().all(|b| b & both != both));
}
