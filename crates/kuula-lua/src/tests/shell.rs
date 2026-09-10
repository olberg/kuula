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

fn press(c: &mut Console, bits: u8) {
    c.step(FrameInput::new(bits));
    c.step(FrameInput::NONE);
}

fn steps(c: &mut Console, n: usize) {
    steps_with(c, 0, n);
}

fn steps_with(c: &mut Console, bits: u8, n: usize) {
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
