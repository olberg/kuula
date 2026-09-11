use std::rc::Rc;

use crate::LuaGuest;
use kuula_core::input::{BTN_A, BTN_UP};
use kuula_core::{Console, DrawState, FrameInput, Guest, Snapshot, SnapshotLimits};

mod api;
mod audio;
mod gfx;
mod meter;
mod net;
mod numeric;
mod saves;
mod shell;

/// A cart of in-memory files, `main.lua` first.
pub(crate) fn cart(entries: &[(&str, &[u8])]) -> Rc<Snapshot> {
    Rc::new(
        Snapshot::from_entries(
            entries.iter().map(|(k, v)| (*k, v.to_vec())),
            SnapshotLimits::default(),
        )
        .unwrap(),
    )
}

pub(crate) fn console(src: &str) -> Console {
    Console::new(cart(&[("main.lua", src.as_bytes())]), LuaGuest::factory)
}

/// Run `n` frames of no input.
pub(crate) fn run(c: &mut Console, n: usize) {
    for _ in 0..n {
        c.step(FrameInput::NONE);
    }
}

/// The guest exposes no eval, so tests report state by drawing it.
pub(crate) fn pixel(c: &Console, x: i32, y: i32) -> u8 {
    let out = c.output();
    out.screen[y as usize * out.width as usize + x as usize]
}

#[test]
fn syntax_error_is_a_compile_error_with_line_and_never_runs() {
    let src = "marker = 1\nlocal x = = 2\n";
    let guest = LuaGuest::new(src, "main.lua");
    let fault = guest.err().expect("compile error");
    assert_eq!(fault.code, "compile_error");
    assert_eq!(fault.file, "main.lua");
    assert_eq!(fault.line, Some(2));
    assert!(
        fault.message.contains("unexpected symbol"),
        "{}",
        fault.message
    );

    // Through the console, the fault is there before any step, and the
    // body (which would have set the marker and drawn) never runs.
    let mut c = console("marker = 1\npset(0, 0, 5)\nlocal x = = 2\n");
    assert_eq!(c.state().fault().unwrap().code, "compile_error");
    c.step(FrameInput::NONE);
    assert_eq!(c.frame(), 0);
    assert_eq!(pixel(&c, 0, 0), 0);
}

#[test]
fn top_level_runtime_error_reports_the_line() {
    let mut c = console("local t = nil\n\nlocal y = t.field\n");
    c.step(FrameInput::NONE);
    let fault = c.state().fault().cloned().expect("runtime error");
    assert_eq!(fault.code, "runtime_error");
    assert_eq!(fault.location(), "main.lua:3");
    assert!(
        fault.message.contains("attempt to index"),
        "{}",
        fault.message
    );
    assert!(!fault.message.starts_with("main.lua"), "prefix stripped");
}

#[test]
fn error_thrown_with_error_call_reports_the_line() {
    let mut c = console("function _update(dt)\n  error('custom')\nend\n");
    run(&mut c, 2);
    let fault = c.state().fault().cloned().expect("runtime error");
    assert_eq!(fault.location(), "main.lua:2");
    assert_eq!(fault.message, "custom");
}

#[test]
fn init_runs_exactly_once_across_ten_steps() {
    let src = "\
count = 0
function _init() count = count + 1 end
function _draw() pset(0, 0, count) end
";
    let mut c = console(src);
    run(&mut c, 10);
    assert_eq!(pixel(&c, 0, 0), 1);
    assert_eq!(c.frame(), 10);
}

#[test]
fn update_then_draw_once_per_step_after_the_first() {
    // Log of calls as a string; drawn as a pixel row where u=1, d=2.
    let src = "\
calls = {}
function _update(dt)
  assert(dt > 0.016 and dt < 0.017, 'dt is 1/60')
  calls[#calls + 1] = 1
end
function _draw()
  calls[#calls + 1] = 2
  for i, v in ipairs(calls) do pset(i - 1, 0, v) end
end
";
    let mut c = console(src);
    let out = c.step(FrameInput::NONE);
    assert!(
        out.screen.iter().all(|&p| p == 0),
        "first step runs neither"
    );
    run(&mut c, 3);
    assert_eq!(c.state().fault(), None);
    let row: Vec<u8> = (0..7).map(|x| pixel(&c, x, 0)).collect();
    assert_eq!(row, [1, 2, 1, 2, 1, 2, 0]);
}

#[test]
fn draw_calling_cls_fills_the_screen() {
    let mut c = console("function _draw() cls(3) end");
    run(&mut c, 2);
    let out = c.output();
    assert_eq!(out.screen.len(), 640 * 480, "default mode is 640x480");
    assert!(out.screen.iter().all(|&p| p == 3));
}

#[test]
fn pset_then_pget_agree_on_both_sides() {
    let src = "\
function _init()
  pset(10, 10, 7)
  got = pget(10, 10)
  pset(11, 10, got)
  pset(12, 10, pget(-1, -1))
end
";
    let mut c = console(src);
    c.step(FrameInput::NONE);
    assert_eq!(c.state().fault(), None);
    assert_eq!(pixel(&c, 10, 10), 7);
    assert_eq!(pixel(&c, 11, 10), 7, "Lua saw the same byte");
    assert_eq!(pixel(&c, 12, 10), 0, "pget outside the screen is 0");
}

#[test]
fn btn_reads_the_frame_input_snapshot() {
    let src = "\
function _draw()
  pset(0, 0, btn(0) and 1 or 2)
  pset(1, 0, btn(4) and 1 or 2)
  pset(2, 0, btn(9) and 1 or 2)
  pset(3, 0, btn(-1) and 1 or 2)
end
";
    let mut c = console(src);
    c.step(FrameInput::NONE);
    c.step(FrameInput::new(BTN_UP));
    assert_eq!(
        [
            pixel(&c, 0, 0),
            pixel(&c, 1, 0),
            pixel(&c, 2, 0),
            pixel(&c, 3, 0)
        ],
        [1, 2, 2, 2]
    );
    c.step(FrameInput::new(BTN_A));
    assert_eq!([pixel(&c, 0, 0), pixel(&c, 1, 0)], [2, 1]);
    c.step(FrameInput::NONE);
    assert_eq!([pixel(&c, 0, 0), pixel(&c, 1, 0)], [2, 2]);
}

#[test]
fn after_an_update_error_draw_is_skipped_and_the_screen_is_kept() {
    let src = "\
n = 0
function _update(dt)
  n = n + 1
  if n == 2 then error('bang') end
end
function _draw() cls(n) end
";
    let mut c = console(src);
    run(&mut c, 2);
    assert!(c.output().screen.iter().all(|&p| p == 1));
    let out = c.step(FrameInput::NONE);
    assert_eq!(out.frame, 3);
    assert!(out.screen.iter().all(|&p| p == 1), "draw did not run");
    let fault = c.state().fault().cloned().unwrap();
    assert_eq!(fault.code, "runtime_error");
    assert_eq!(fault.location(), "main.lua:4");
    let out = c.step(FrameInput::NONE);
    assert_eq!(out.frame, 3);
    assert!(out.screen.iter().all(|&p| p == 1));
}

#[test]
fn dangerous_libraries_are_nil() {
    let src = "\
function _init()
  local names = {os, io, package, debug, dofile, loadfile, loadstring}
  pset(0, 0, #names == 0 and 1 or 2)
  pset(1, 0, (os == nil and io == nil and package == nil and debug == nil) and 1 or 2)
  -- `load` is the save-slot binding now; it takes a slot
  -- number and never compiles a chunk.
  local ran, err = pcall(load, 'return 1')
  pset(2, 0, (type(require) == 'function' and dofile == nil and loadfile == nil and not ran and err ~= 1) and 1 or 2)
  pset(3, 0, (math and string and table and pairs and ipairs and tostring) and 1 or 2)
end
";
    let mut c = console(src);
    c.step(FrameInput::NONE);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    assert_eq!(
        [
            pixel(&c, 0, 0),
            pixel(&c, 1, 0),
            pixel(&c, 2, 0),
            pixel(&c, 3, 0)
        ],
        [1, 1, 1, 1]
    );
}

#[test]
fn bare_print_goes_to_the_log_and_positioned_print_draws() {
    let src = "\
function _draw()
  print('hello', 1)
  print(42)
  local x = print('A', 0, 0, 7)
  print(x)
end
";
    let mut c = console(src);
    run(&mut c, 2);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    assert_eq!(c.output().log, ["hello\t1", "42", "4"]);
    assert_eq!(pixel(&c, 0, 0), 7);
}

#[test]
fn print_with_non_numeric_second_argument_logs_instead_of_faulting() {
    let src = "function _init()
  print('x =', 'y')
  print('pos', {})
  print('a', nil)
  print('hp', 3)
  print('n', 1, 2)
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = c.output().log;
    assert_eq!(log[0], "x =	y");
    assert!(log[1].starts_with("pos	table:"), "{}", log[1]);
    assert_eq!(log[2], "a	nil");
    assert_eq!(log[3], "hp	3");
    assert_eq!(log.len(), 4, "print('n', 1, 2) draws, so it is not logged");
    let c = &c;
    let mut cell = (1..5).flat_map(|x| (2..8).map(move |y| pixel(c, x, y)));
    assert!(cell.any(|p| p == 7), "glyph drawn at (1, 2)");
}

/// `math.random` and `pairs` order over string keys are the two places a
/// stock Lua state pulls in the wall clock and an address. Both are pinned
/// here: the values only change if the seeding does, and that would break
/// determinism across runs.
#[test]
fn math_random_and_pairs_order_are_fixed_across_states() {
    let src = "function _init()
  print(math.random(1, 1000000) .. ' ' .. math.random(1, 1000000) .. ' ' .. math.random())
  local t = {alpha=1, beta=1, gamma=1, delta=1, eps=1, zeta=1, eta=1, theta=1, iota=1, kappa=1}
  local ks = {}
  for k in pairs(t) do ks[#ks + 1] = k end
  print(table.concat(ks, ','))
end
";
    let runs: Vec<Vec<String>> = (0..3)
        .map(|_| {
            let mut c = console(src);
            run(&mut c, 1);
            assert_eq!(c.state().fault(), None, "{:?}", c.state());
            c.output().log.to_vec()
        })
        .collect();
    assert_eq!(runs[0], runs[1]);
    assert_eq!(runs[1], runs[2]);
    assert_eq!(runs[0], [MATH_RANDOM_LINE, PAIRS_ORDER_LINE]);
}

/// A bare `math.randomseed()` in stock Lua reseeds from the clock; here it
/// restores the fixed seed, and an explicit seed still works.
#[test]
fn bare_randomseed_is_deterministic() {
    let src = "function _init()
  math.random()
  math.randomseed()
  print(math.random(1, 1000000) .. ' ' .. math.random(1, 1000000) .. ' ' .. math.random())
  math.randomseed(42)
  local a = math.random(1, 1000000)
  math.randomseed(42)
  print(a == math.random(1, 1000000))
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    assert_eq!(c.output().log.to_vec(), [MATH_RANDOM_LINE, "true"]);
}

/// Pinned observations for [`math_random_and_pairs_order_are_fixed_across_states`].
/// A change here is a change to what carts observe; update deliberately.
const MATH_RANDOM_LINE: &str = "275394 909833 0.069275939529084729";
const PAIRS_ORDER_LINE: &str = "theta,beta,kappa,alpha,eta,zeta,eps,iota,delta,gamma";

#[test]
fn bad_argument_from_a_binding_is_a_runtime_error_with_a_line() {
    let src = "function _draw()\n  pset('a', 'b')\nend\n";
    let mut c = console(src);
    run(&mut c, 2);
    let fault = c.state().fault().cloned().unwrap();
    assert_eq!(fault.code, "runtime_error");
    assert_eq!(fault.file, "main.lua");
    assert_eq!(fault.line, Some(2), "{fault}");
}

#[test]
fn guests_drop_cleanly_and_do_not_leak() {
    let src = "t = {} for i = 1, 1000 do t[i] = tostring(i) end";
    let mut state = DrawState::default();
    let mut first = LuaGuest::new(src, "main.lua").unwrap();
    first.step(&mut state, FrameInput::NONE, 1).unwrap();
    first.gc_collect();
    let baseline = first.used_memory();
    drop(first);

    for _ in 0..100 {
        let mut g = LuaGuest::new(src, "main.lua").unwrap();
        g.step(&mut state, FrameInput::NONE, 1).unwrap();
        drop(g);
    }

    let mut last = LuaGuest::new(src, "main.lua").unwrap();
    last.step(&mut state, FrameInput::NONE, 1).unwrap();
    last.gc_collect();
    assert_eq!(
        last.used_memory(),
        baseline,
        "state memory is per guest and returns to baseline"
    );
    assert!(baseline > 0);
}

#[test]
fn a_missing_draw_leaves_the_screen_alone() {
    let mut c = console("function _init() cls(5) end");
    run(&mut c, 5);
    assert_eq!(c.state().fault(), None);
    assert!(c.output().screen.iter().all(|&p| p == 5));
}

#[test]
fn factory_is_usable_as_a_trait_object() {
    let g: Box<dyn Guest> = LuaGuest::factory("function _init() cls(9) end", "main.lua").unwrap();
    let mut c = Console::from_guest(g);
    c.step(FrameInput::NONE);
    assert!(c.output().screen.iter().all(|&p| p == 9));
}
