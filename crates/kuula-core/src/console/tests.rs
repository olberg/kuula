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
    struct Records(Rc<RefCell<Vec<u8>>>);
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
