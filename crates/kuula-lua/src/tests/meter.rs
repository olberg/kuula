//! The cycle budget, the memory cap and containment against carts that
//! try to dodge them. Every test
//! here ends with the process alive and a stable fault code; the hostile
//! example carts under `examples/hostile` cover the same ground through
//! the binary.

use super::{cart, console, pixel, run};
use crate::{LuaGuest, LUA_HEAP_SOFT_CAP};
use kuula_core::meter::{FRAME_BUDGET, HOOK_CHARGE, INIT_BUDGET};
use kuula_core::{Category, Console, ConsoleState, Fault, FrameInput};

/// Step until the console faults, at most `max` frames.
fn run_until_fault(c: &mut Console, max: usize) -> Fault {
    for _ in 0..max {
        c.step(FrameInput::NONE);
        if let Some(f) = c.state().fault() {
            return f.clone();
        }
    }
    panic!("no fault after {max} frames: {:?}", c.state());
}

/// After any fault, a fresh guest must run untouched (the process and
/// the mlua state pool carry nothing over).
fn fresh_guest_still_works() {
    let mut c = console("function _draw() cls(9) end");
    run(&mut c, 2);
    assert_eq!(c.state().fault(), None);
    assert_eq!(pixel(&c, 0, 0), 9);
}

#[test]
fn an_infinite_loop_faults_with_budget_exceeded_naming_the_callback() {
    let src = "\
n = 0
function _draw() cls(4) end
function _update(dt)
  n = n + 1
  if n == 2 then while true do end end
end
";
    let mut c = console(src);
    c.step(FrameInput::NONE);
    c.step(FrameInput::NONE);
    assert_eq!(pixel(&c, 0, 0), 4, "frame 2 drew");
    let fault = run_until_fault(&mut c, 1);
    assert_eq!(fault.code, "budget_exceeded");
    assert_eq!(fault.file, "main.lua");
    assert_eq!(fault.line, Some(5), "{fault}");
    assert!(
        fault.message.starts_with("_update used"),
        "callback named: {}",
        fault.message
    );
    assert!(
        fault.message.contains(&format!("of {FRAME_BUDGET} cycles")),
        "{}",
        fault.message
    );
    assert_eq!(pixel(&c, 0, 0), 4, "the last complete frame is kept");
    let out = c.output();
    assert!(out.profile.total() > FRAME_BUDGET);
    assert!(out.profile.get(Category::Lua) > 0);
    fresh_guest_still_works();
}

#[test]
fn the_main_chunk_and_init_share_the_init_budget() {
    let mut c = console("while true do end");
    let fault = run_until_fault(&mut c, 1);
    assert_eq!(fault.code, "budget_exceeded");
    assert!(
        fault.message.starts_with("main chunk used"),
        "{}",
        fault.message
    );
    assert!(fault.message.contains(&format!("of {INIT_BUDGET} cycles")));
    let mut c = console("function _init() while true do end end");
    let fault = run_until_fault(&mut c, 1);
    assert!(fault.message.starts_with("_init used"), "{}", fault.message);
    assert_eq!(fault.line, Some(1));
}

#[test]
fn pcall_cannot_keep_a_cart_alive_past_its_budget() {
    // The cart swallows the fault and tries to carry on drawing; the
    // re-armed hook raises again outside the pcall before `pset` runs,
    // and the meter's own record is what the console reports.
    let src = "\
survived = 0
function _update(dt)
  local ok, err = pcall(function() while true do end end)
  survived = survived + 1
  pset(0, 0, 7)
  while true do pcall(function() end) end
end
";
    let mut c = console(src);
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
    assert!(
        fault.message.starts_with("_update used"),
        "{}",
        fault.message
    );
    assert_eq!(pixel(&c, 0, 0), 0, "no drawing after the budget");
    fresh_guest_still_works();
}

#[test]
fn an_xpcall_handler_that_loops_still_reports_the_original_fault() {
    let src = "\
function _update(dt)
  xpcall(function() while true do end end, function(e) while true do end end)
  cls(7)
end
";
    let mut c = console(src);
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
    assert_eq!(pixel(&c, 0, 0), 0);
}

#[test]
fn xpcall_stays_yieldable_and_checks_its_handler() {
    // The native xpcall is kept (a Rust replacement is a C-call
    // boundary that a coroutine cannot yield across); only the handler
    // is guarded.
    let src = "\
function _init()
  local co = coroutine.create(function()
    return xpcall(function() coroutine.yield(1) return 'done' end, print)
  end)
  local ok, v = coroutine.resume(co)
  local ok2, ok3, v2 = coroutine.resume(co)
  print(tostring(ok) .. ' ' .. tostring(v) .. ' ' .. tostring(ok2) .. ' ' .. tostring(ok3) .. ' ' .. tostring(v2))
  print(select(2, xpcall(error, function(e) return 'handled ' .. e end, 'x')))
  print(pcall(xpcall, print, 5))
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = c.output().log.to_vec();
    assert_eq!(log[0], "true 1 true true done");
    assert_eq!(log[1], "handled x");
    assert!(log[2].starts_with("false"), "{}", log[2]);
    assert!(log[2].contains("function expected"), "{}", log[2]);
}

#[test]
fn a_binding_is_refused_once_the_meter_is_dead() {
    // Catch the fault, then call the API from inside the same pcall
    // chain: the call itself must fail, not just the next instruction.
    let src = "\
function _update(dt)
  pcall(function()
    pcall(function() while true do end end)
    cls(7)
  end)
end
";
    let mut c = console(src);
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
    assert_eq!(pixel(&c, 0, 0), 0, "cls refused");
}

#[test]
fn a_coroutine_storm_is_priced_and_faults() {
    let src = "\
function _update(dt)
  while true do
    local co = coroutine.wrap(function() local x = 0 for i = 1, 500 do x = x + i end end)
    co()
  end
end
";
    let mut c = console(src);
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
    // Resuming an existing coroutine forever is caught inside it.
    let src = "\
co = coroutine.create(function() while true do coroutine.yield() end end)
function _update(dt)
  while true do coroutine.resume(co) end
end
";
    let mut c = console(src);
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
    // And a loop inside a coroutine that never yields.
    let src = "\
function _update(dt)
  coroutine.wrap(function() while true do end end)()
end
";
    let mut c = console(src);
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
    assert_eq!(fault.line, Some(2));
}

#[test]
fn coroutine_creation_costs_one_hook_interval() {
    let src = "\
function _init()
  local before = stat('cpu_cycles')
  local co = coroutine.create(function() end)
  local after = stat('cpu_cycles')
  print(after - before)
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let cost: u64 = c.output().log[0].parse().unwrap();
    assert!((HOOK_CHARGE..HOOK_CHARGE + 10).contains(&cost), "{cost}");
}

/// Lua runs finalisers with hooks off (lgc.c `GCTM`), so a `__gc` that
/// loops could never be metered; `setmetatable` refuses it instead.
/// Adding the field after the call does not mark the object, so the
/// loop never runs.
#[test]
fn gc_finalizers_are_refused_and_cannot_be_smuggled_in() {
    let src = "\
function _update(dt)
  setmetatable({}, {__gc = function() while true do end end})
  cls(7)
end
";
    let mut c = console(src);
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "runtime_error");
    assert!(fault.message.contains("__gc"), "{}", fault.message);
    assert_eq!(fault.line, Some(2));
    assert_eq!(pixel(&c, 0, 0), 0);
    let src = "\
function _update(dt)
  local mt = {}
  setmetatable({}, mt)
  mt.__gc = function() while true do end end
  collectgarbage()
  local closed = 0
  do
    local x <close> = setmetatable({}, {__close = function() closed = 1 end})
  end
  cls(closed == 1 and 5 or 6)
end
";
    let mut c = console(src);
    run(&mut c, 2);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    assert_eq!(pixel(&c, 0, 0), 5, "__close still works");
}

#[test]
fn a_table_sort_comparator_that_loops_faults() {
    let src = "\
function _update(dt)
  local t = {3, 1, 2}
  table.sort(t, function(a, b) while true do end end)
end
";
    let mut c = console(src);
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
}

#[test]
fn an_allocation_loop_faults_with_out_of_memory() {
    let src = "\
t = {}
function _update(dt)
  for i = 1, 8000 do t[#t + 1] = {i} end
end
";
    let mut c = console(src);
    let fault = run_until_fault(&mut c, 200);
    assert_eq!(fault.code, "out_of_memory", "{fault}");
    assert!(
        fault.message.contains(&LUA_HEAP_SOFT_CAP.to_string()),
        "{}",
        fault.message
    );
    assert!(c.output().profile.lua_mem > LUA_HEAP_SOFT_CAP as u64);
    fresh_guest_still_works();
}

#[test]
fn one_huge_allocation_hits_the_hard_limit() {
    // 30 MiB is over the hard limit but its price fits the init budget,
    // so mlua's allocator refuses it inside the callback.
    let src = "function _init() s = string.rep('x', 30 * 1024 * 1024) end";
    let mut c = console(src);
    let fault = run_until_fault(&mut c, 1);
    assert_eq!(fault.code, "out_of_memory", "{fault}");
    assert_eq!(fault.file, "main.lua");
    // Catching it and carrying on does not help: the soft cap check
    // after the step sees whatever survived, and the meter records it.
    let src = "\
function _update(dt)
  keep = keep or {}
  pcall(function() keep[#keep + 1] = string.rep('y', 1024 * 1024) end)
end
";
    let mut c = console(src);
    let fault = run_until_fault(&mut c, 60);
    assert_eq!(fault.code, "out_of_memory", "{fault}");
}

#[test]
fn a_pattern_bomb_faults_before_it_runs() {
    let src = "\
function _init()
  local s = string.rep('a', 100000)
  local t = os and os.clock()
  print(s:find('.-.-.-.-b'))
end
";
    let mut c = console(src);
    let fault = run_until_fault(&mut c, 1);
    assert_eq!(fault.code, "budget_exceeded", "{fault}");
    assert_eq!(fault.line, Some(4));
    // Ordinary matching on ordinary strings is cheap and works, also
    // through the string metatable and with a plain find.
    let src = "\
function _init()
  local before = stat('cpu_cycles')
  local a, b = ('x=1,y=22'):match('x=(%d+),y=(%d+)')
  local s = ('hello world'):gsub('%s+', '_')
  local i = string.find('a.b', '.', 1, true)
  for w in ('one two'):gmatch('%a+') do end
  print(a, b, s, i, stat('cpu_cycles') - before)
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = c.output().log[0].clone();
    let parts: Vec<&str> = log.split('\t').collect();
    assert_eq!(&parts[..4], ["1", "22", "hello_world", "2"]);
    let cycles: u64 = parts[4].parse().unwrap();
    assert!(cycles < 2000, "ordinary matching stays cheap: {cycles}");
}

#[test]
fn string_rep_and_format_are_priced_by_their_output() {
    let src = "\
function _init()
  local before = stat('cpu_cycles')
  local s = string.rep('ab', 4000)
  local mid = stat('cpu_cycles')
  local f = string.format('%s%s', s, s)
  print((mid - before) .. ' ' .. (stat('cpu_cycles') - mid) .. ' ' .. #f)
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = c.output().log[0].clone();
    let n: Vec<u64> = log.split(' ').map(|p| p.parse().unwrap()).collect();
    assert!(n[0] >= 1000 && n[0] < 1010, "rep 8000 bytes: {}", n[0]);
    assert!(n[1] >= 2000 && n[1] < 2010, "format 16000 bytes: {}", n[1]);
    assert_eq!(n[2], 16000);
    let mut c = console("function _update(dt) local s = string.rep('x', 1 << 30) end");
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
    // `str_rep` coerces a numeric string, so the price must too.
    let mut c = console("function _update(dt) local s = string.rep('x', '1073741824') end");
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
    let mut c = console("function _update(dt) local s = string.rep('x', '1e9') end");
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
}

#[test]
fn gsub_and_the_linear_natives_are_priced() {
    let src = "\
function _init()
  local s = string.rep('x', 8000)
  local a = stat('cpu_cycles')
  local out = s:gsub('x', 'yy')
  local b = stat('cpu_cycles')
  local u = s:upper()
  local c = stat('cpu_cycles')
  local part = s:sub(1, 8)
  local d = stat('cpu_cycles')
  local n = utf8.len(s)
  local e = stat('cpu_cycles')
  local t = {} for i = 1, 800 do t[i] = i end
  local f = stat('cpu_cycles')
  table.insert(t, 1, 0)
  local g = stat('cpu_cycles')
  print(#out .. ' ' .. (b - a) .. ' ' .. (c - b) .. ' ' .. (d - c) .. ' ' .. (e - d) .. ' ' .. (g - f))
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = c.output().log[0].clone();
    let n: Vec<u64> = log.split(' ').map(|p| p.parse().unwrap()).collect();
    assert_eq!(n[0], 16000);
    // gsub: the pattern price on 8000 bytes plus 16000 output bytes / 8.
    assert!(n[1] >= 2000 && n[1] < 2100, "gsub: {}", n[1]);
    assert!(n[2] >= 1000 && n[2] < 1010, "upper: {}", n[2]);
    assert!(n[3] < 10, "a short sub of a long string is cheap: {}", n[3]);
    assert!(n[4] >= 1000 && n[4] < 1010, "utf8.len: {}", n[4]);
    assert!(n[5] >= 100 && n[5] < 110, "insert at the front: {}", n[5]);
    // Each of these did seconds of host work per frame under the old
    // instruction-only price.
    for body in [
        "for i = 1, 100000 do collectgarbage('collect') end",
        "for i = 1, 10000 do utf8.len(big) end",
        "for i = 1, 10000 do local u = big:upper() end",
        "for i = 1, 100000 do table.insert(list, 1, i) end",
    ] {
        let src = format!(
            "big = string.rep('x', 1 << 20)
list = {{}} for i = 1, 100000 do list[i] = i end
function _update(dt) {body} end"
        );
        let mut c = console(&src);
        let fault = run_until_fault(&mut c, 2);
        assert_eq!(fault.code, "budget_exceeded", "{body}: {fault}");
    }
    let mut c = console("function _init() collectgarbage('stop') end");
    let fault = run_until_fault(&mut c, 1);
    assert_eq!(fault.code, "runtime_error");
    assert!(fault.message.contains("not available"), "{}", fault.message);
    let mut c = console("function _init() print(collectgarbage('count') > 0) end");
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    assert_eq!(c.output().log[0], "true");
}

#[test]
fn a_range_dash_in_a_set_is_not_a_quantifier() {
    let src = "\
function _init()
  local text = string.rep('abc ', 1024)
  local before = stat('cpu_cycles')
  local out, n = text:gsub('[a-z]+', '')
  print(n .. ' ' .. (stat('cpu_cycles') - before))
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = c.output().log[0].clone();
    let n: Vec<u64> = log.split(' ').map(|p| p.parse().unwrap()).collect();
    assert_eq!(n[0], 1024);
    // One quantifier: 6 * 4096^2 / 256; two would be over a billion.
    assert!(n[1] > 390_000 && n[1] < 400_000, "{}", n[1]);
}

#[test]
fn drawing_is_priced_by_pixels_touched() {
    let src = "\
function _init()
  local a = stat('cpu_cycles')
  cls(1)
  local b = stat('cpu_cycles')
  rectfill(0, 0, 99, 99, 2)
  local c = stat('cpu_cycles')
  rectfill(-1000, -1000, -900, -900, 2)
  local d = stat('cpu_cycles')
  print((b - a) .. ' ' .. (c - b) .. ' ' .. (d - c))
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = c.output().log[0].clone();
    let n: Vec<u64> = log.split(' ').map(|p| p.parse().unwrap()).collect();
    // Each reading includes the stat call itself (1 cycle).
    assert_eq!(n[0], 1 + 640 * 480 / 64 + 1, "cls");
    assert_eq!(n[1], 1 + 10000 / 3 + 1, "rectfill");
    assert_eq!(n[2], 1 + 1, "off-screen rectfill is the minimum");
    // A circle's midpoint walk is not clipped, so a huge radius that
    // barely overlaps the clip is priced by the radius.
    let src = "\
function _init()
  local a = stat('cpu_cycles')
  circ(-32000, -32000, 32768, 1)
  local b = stat('cpu_cycles')
  circfill(-32000, -32000, 32768, 1)
  local c = stat('cpu_cycles')
  print((b - a) .. ' ' .. (c - b))
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = c.output().log[0].clone();
    let n: Vec<u64> = log.split(' ').map(|p| p.parse().unwrap()).collect();
    assert_eq!(n[0], 1 + 32768 * 6 / 3 + 1, "circ");
    assert_eq!(n[1], 1 + 32768 * 6 / 3 + 1, "circfill");
    let mut c =
        console("function _draw() for i = 1, 5000 do circ(-32000, -32000, 32768, 1) end end");
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
    let mut c = console("function _draw() for i = 1, 1000 do cls(7) end end");
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
    assert!(fault.message.starts_with("_draw used"), "{}", fault.message);
}

#[test]
fn a_refused_buffer_is_a_graphics_error_not_a_dead_meter() {
    // The price is charged for what was allocated, so a request the
    // ledger refuses stays catchable and costs nothing.
    let src = "\
function _init()
  local ok, err = pcall(buf, 'u8', 4096, 4096)
  local ok2, err2 = pcall(buf, 'u8', -1, -1)
  local before = stat('cpu_cycles')
  local b = buf('u8', 256, 512)
  print(tostring(ok) .. '|' .. tostring(err) .. '|' .. tostring(ok2) .. '|' .. (stat('cpu_cycles') - before))
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = c.output().log[0].clone();
    let parts: Vec<&str> = log.split('|').collect();
    assert_eq!(parts[0], "false");
    assert!(parts[1].contains("graphics_budget_exceeded"), "{log}");
    assert_eq!(parts[2], "false");
    let cycles: u64 = parts[3].parse().unwrap();
    assert_eq!(cycles, 1 + 1 + 256 * 512 / 128 + 1, "{log}");
}

#[test]
fn stat_reports_the_meter_and_the_caps() {
    let src = "\
function _draw()
  cls(0)
  print(tostring(stat('cpu') > 0 and stat('cpu') < 1))
  print(stat('cpu_budget'))
  print(tostring(stat('mem') > 0) .. ' ' .. stat('mem_limit') .. ' ' .. stat('gfx_mem') .. ' ' .. tostring(stat('gfx_limit') > 0))
  print(stat('frame'))
end
";
    let mut c = console(src);
    run(&mut c, 3);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = c.output().log.to_vec();
    assert_eq!(log[0], "true");
    assert_eq!(log[1], FRAME_BUDGET.to_string());
    assert_eq!(log[2], format!("true {LUA_HEAP_SOFT_CAP} 0 true"));
    assert_eq!(log[3], "3");
    let mut c = console("function _init() stat('bogus') end");
    let fault = run_until_fault(&mut c, 1);
    assert_eq!(fault.code, "runtime_error");
    assert!(fault.message.contains("unknown stat"), "{}", fault.message);
}

#[test]
fn the_profile_adds_up_and_is_reset_every_frame() {
    let src = "\
function _draw()
  cls(1)
  print('hi', 0, 0, 7)
  local b = buf('u8', 64, 64)
  b:fill(3)
  local x = 0
  for i = 1, 5000 do x = x + i end
end
";
    let mut c = console(src);
    run(&mut c, 2);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let p = c.output().profile.clone();
    assert_eq!(p.budget, FRAME_BUDGET);
    assert!(p.get(Category::Draw) > 640 * 480 / 64);
    assert!(p.get(Category::Text) >= 2);
    assert!(p.get(Category::Buf) >= 2 * (1 + 4096 / 128));
    assert!(p.get(Category::Lua) >= 10_000, "{:?}", p);
    assert!(p.lua_mem > 0);
    assert_eq!(
        p.total(),
        Category::ALL.iter().map(|&c| p.get(c)).sum::<u64>()
    );
    let first = p.total();
    run(&mut c, 1);
    let again = c.output().profile.total();
    assert!(again.abs_diff(first) <= HOOK_CHARGE, "{first} vs {again}");
}

#[test]
fn require_compilation_is_priced_once() {
    let src = "\
function _init()
  local a = stat('cpu_cycles')
  require('big')
  local b = stat('cpu_cycles')
  require('big')
  print(b - a, stat('cpu_cycles') - b)
end
";
    let module = format!("{}\nreturn 1\n", "-- padding\n".repeat(400));
    let mut c = Console::new(
        cart(&[
            ("main.lua", src.as_bytes()),
            ("src/big.lua", module.as_bytes()),
        ]),
        LuaGuest::factory,
    );
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = c.output().log[0].clone();
    let n: Vec<u64> = log.split('\t').map(|p| p.parse().unwrap()).collect();
    assert!(
        n[0] > module.len() as u64 / 4,
        "first load compiled: {}",
        n[0]
    );
    assert!(n[1] < 20, "second load is cached: {}", n[1]);
}

#[test]
fn a_faulted_guest_stays_faulted_and_the_console_freezes() {
    let mut c = console("function _update(dt) while true do end end");
    let fault = run_until_fault(&mut c, 2);
    assert_eq!(fault.code, "budget_exceeded");
    for _ in 0..3 {
        let out = c.step(FrameInput::NONE);
        assert_eq!(out.frame, 2);
    }
    assert!(matches!(c.state(), ConsoleState::Faulted(f) if f.code == "budget_exceeded"));
}

/// Hook overhead at three intervals.
/// Run with `cargo test -p kuula-lua --release -- --ignored --nocapture
/// hook_overhead`.
#[test]
#[ignore]
fn hook_overhead() {
    use mlua::{HookTriggers, Lua, VmState};
    use std::time::Instant;
    const LOOP: &str = "local x = 0 for i = 1, 20000000 do x = x + i end return x";
    let measure = |every: Option<u32>| {
        let lua = Lua::new();
        if let Some(n) = every {
            lua.set_global_hook(HookTriggers::new().every_nth_instruction(n), |_, _| {
                Ok(VmState::Continue)
            })
            .unwrap();
        }
        let f = lua.load(LOOP).into_function().unwrap();
        let start = Instant::now();
        let _: i64 = f.call(()).unwrap();
        start.elapsed()
    };
    let base = measure(None);
    println!("no hook: {base:?}");
    for n in [100, 1000, 10000] {
        let t = measure(Some(n));
        println!(
            "every {n}: {t:?} ({:.1}% over no hook)",
            (t.as_secs_f64() / base.as_secs_f64() - 1.0) * 100.0
        );
    }
}
