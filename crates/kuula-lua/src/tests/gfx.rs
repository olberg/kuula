//! Buffers, sheets, maps, `require`, and the sandbox rules
//! that arrived with them.

use super::{cart, console, pixel, run};
use crate::api::Price;
use crate::LuaGuest;
use kuula_core::assets::encode_indexed_png;
use kuula_core::palette::DEFAULT_PALETTE;
use kuula_core::{Console, FrameInput};

fn sheet_png() -> Vec<u8> {
    // 16x8: cell 0 has pixel (0,0)=1 and (1,1)=2, cell 1 is solid 3.
    let mut px = vec![0u8; 16 * 8];
    px[0] = 1;
    px[16 + 1] = 2;
    for y in 0..8 {
        for x in 8..16 {
            px[y * 16 + x] = 3;
        }
    }
    encode_indexed_png(16, 8, &px, &DEFAULT_PALETTE)
}

const LOG_HELPER: &str = "local function log(...)
  local t = {}
  for i = 1, select('#', ...) do t[i] = tostring((select(i, ...))) end
  print(table.concat(t, '\t'))
end
";

fn gfx_console(main: &str) -> Console {
    let main = format!("{LOG_HELPER}{main}");
    let png = sheet_png();
    let map =
        br#"{"tile_size": 8, "width": 2, "height": 2, "layers": [[1, -1, 0, 1], [-1, 1, -1, -1]]}"#;
    let src = cart(&[
        ("main.lua", main.as_bytes()),
        ("cart.toml", b"[cart]\nscreen_mode = \"320x240\"\n"),
        ("gfx/tiles.png", &png),
        ("map/level.json", map),
        (
            "src/util.lua",
            b"local M = {}\nfunction M.double(x) return x * 2 end\nreturn M\n",
        ),
        ("src/a.lua", b"return require('b')\n"),
        ("src/b.lua", b"return require('a')\n"),
        ("src/bad.lua", b"local x = = 1\n"),
        ("src/thrower.lua", b"local t = nil\nreturn t.x\n"),
        ("src/binary.lua", b"\x1bLua\x55\x00binary"),
        ("src/nilmod.lua", b"counter = (counter or 0) + 1\n"),
    ]);
    Console::new(src, LuaGuest::factory)
}

fn ok(c: &Console) {
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
}

#[test]
fn every_primitive_is_reachable() {
    let src = "\
function _init()
  cls(1)
  pset(0, 0, 2)
  line(0, 1, 3, 1, 3)
  rect(0, 2, 2, 4, 4)
  rectfill(4, 2, 6, 4, 5)
  circ(20, 20, 3, 6)
  circfill(30, 20, 3, 7)
  print('A', 40, 0, 8)
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    ok(&c);
    assert_eq!(pixel(&c, 0, 0), 2);
    assert_eq!(pixel(&c, 3, 1), 3);
    assert_eq!(pixel(&c, 0, 3), 4);
    assert_eq!(pixel(&c, 1, 3), 1, "rect is an outline");
    assert_eq!(pixel(&c, 5, 3), 5);
    assert_eq!(pixel(&c, 17, 20), 6);
    assert_eq!(pixel(&c, 30, 20), 7);
    assert_eq!(pixel(&c, 40, 0), 8);
    assert_eq!(c.output().width, 320);
}

#[test]
fn clip_camera_and_fillp() {
    let src = "\
function _init()
  clip(2, 2, 2, 2)
  rectfill(0, 0, 10, 10, 5)
  clip()
  camera(-10, -10)
  pset(0, 0, 6)
  camera()
  fillp(0xffff)
  rectfill(20, 0, 21, 0, 7 + 8 * 256)
  fillp(0xffff, true)
  rectfill(22, 0, 23, 0, 9)
  fillp()
  pset(24, 0, 9)
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    ok(&c);
    assert_eq!(pixel(&c, 1, 1), 0);
    assert_eq!(pixel(&c, 2, 2), 5);
    assert_eq!(pixel(&c, 4, 4), 0);
    assert_eq!(
        pixel(&c, 10, 10),
        6,
        "camera(-10, -10) moves draws right and down"
    );
    assert_eq!(pixel(&c, 20, 0), 8, "secondary colour from bits 8..14");
    assert_eq!(pixel(&c, 22, 0), 0, "transparent pattern skips");
    assert_eq!(pixel(&c, 24, 0), 9);
}

#[test]
fn palette_functions_and_locked_colours() {
    let src = "\
function _init()
  pal(16, 10, 20, 30)
  pal(17, 0x0a141e)
  local okc, err = pcall(pal, 3, 1, 2, 3)
  print(okc, err)
  local ok2, err2 = pcall(pal, 200, 0)
  print(ok2, err2)
  palt(1, true)
  pal_map(2, 5)
  rectfill(0, 0, 0, 0, 1)
  rectfill(1, 0, 1, 0, 2)
  pal_reset()
  rectfill(2, 0, 2, 0, 1)
  rectfill(3, 0, 3, 0, 2)
  pal()
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    ok(&c);
    let log = c.output().log.to_vec();
    assert!(
        log[0].starts_with("false\t") && log[0].contains("palette_index_locked"),
        "{}",
        log[0]
    );
    assert!(log[1].contains("palette_index_out_of_range"), "{}", log[1]);
    assert_eq!(pixel(&c, 0, 0), 0, "transparent 1 leaves the cleared 0");
    assert_eq!(pixel(&c, 1, 0), 5);
    assert_eq!(pixel(&c, 2, 0), 1);
    assert_eq!(pixel(&c, 3, 0), 2);
    assert_eq!(
        c.output().palette[16],
        DEFAULT_PALETTE[16],
        "pal() restored"
    );
}

#[test]
fn uncaught_locked_palette_write_faults_with_its_code() {
    let mut c = gfx_console("function _init()\n  pal(0, 1, 2, 3)\nend\n");
    run(&mut c, 1);
    let f = c.state().fault().cloned().unwrap();
    assert_eq!(f.code, "palette_index_locked");
    assert_eq!(f.location(), "main.lua:7");
}

#[test]
fn sheets_sprites_and_maps() {
    let src = "\
function _init()
  local s = load_sheet('tiles')
  log(s:width(), s:height(), s:kind(), tostring(s))
  sheet(s)
  palt(0, true)
  cls(9)
  spr(0, 0, 0)
  spr(0, 8, 0, 1, 1, true)
  sspr(8, 0, 8, 8, 16, 0, 4, 4)
  local m = load_map('level')
  log(m:width(), m:height(), m:layers(), m:tile_size(), m:kind())
  map(m, 0, 0, 0, 16, 2, 2)
  map(m, 0, 0, 0, 32, 2, 2, 1)
  local again = load_sheet('tiles')
  log(again == s, rawequal(again, s))
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    ok(&c);
    let log = c.output().log.to_vec();
    assert_eq!(log[0], "16\t8\tu8\tbuf(u8 16x8)");
    assert_eq!(log[1], "2\t2\t2\t8\ti16");
    assert_eq!(log[2], "false\tfalse", "same buffer, distinct handles");
    assert_eq!(pixel(&c, 0, 0), 1);
    assert_eq!(pixel(&c, 1, 1), 2);
    assert_eq!(pixel(&c, 2, 0), 9, "transparent 0 kept the background");
    assert_eq!(pixel(&c, 15, 0), 1, "flipped");
    assert_eq!(pixel(&c, 16, 0), 3);
    assert_eq!(pixel(&c, 19, 3), 3);
    assert_eq!(pixel(&c, 20, 0), 9);
    // Map layer 0: [1, -1, 0, 1] with 8 px tiles at y = 16.
    assert_eq!(pixel(&c, 0, 16), 3);
    assert_eq!(pixel(&c, 8, 16), 9);
    assert_eq!(pixel(&c, 0, 24), 1);
    assert_eq!(pixel(&c, 8, 24), 3);
    // Layer 1: only cell (1, 0).
    assert_eq!(pixel(&c, 0, 32), 9);
    assert_eq!(pixel(&c, 8, 32), 3);
    assert_eq!(c.draw_state().res.ledger().used(), 128 + 16);
}

#[test]
fn drawing_without_a_sheet_faults_with_no_sheet() {
    let mut c = gfx_console("function _init()\n  spr(0, 0, 0)\nend\n");
    run(&mut c, 1);
    let f = c.state().fault().cloned().unwrap();
    assert_eq!(f.code, "no_sheet");
    assert_eq!(f.location(), "main.lua:7");
    assert!(f.message.contains("sheet(b)"), "{}", f.message);
}

#[test]
fn asset_errors_have_codes() {
    let src = "\
function _init()
  print(pcall(load_sheet, 'missing'))
  print(pcall(load_map, 'tiles'))
  print(pcall(load_sheet, '../etc'))
  print(pcall(map, load_sheet('tiles'), 0, 0, 0, 0, 1, 1))
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    ok(&c);
    let log = c.output().log.to_vec();
    assert!(
        log[0].contains("asset_not_found") && log[0].contains("gfx/missing.png"),
        "{}",
        log[0]
    );
    assert!(log[1].contains("asset_not_found"), "{}", log[1]);
    assert!(log[2].contains("asset_invalid"), "{}", log[2]);
    assert!(log[3].contains("not_a_map"), "{}", log[3]);
}

#[test]
fn bufs_allocate_release_and_copy() {
    let src = "\
function _init()
  local b = buf('u8', 4, 4)
  b:fill(7)
  b:set(0, 0, 300)
  log(b:get(0, 0), b:get(1, 1), b:get(9, 9), b:width(), b:height(), b:kind(), b:layers())
  local f = buf('f32', 2, 2)
  f:set(1, 1, 0.5)
  log(f:get(1, 1), f:kind())
  log(pcall(b.copy, b, f, 0, 0, 1, 1, 0, 0))
  screen:copy(b, 0, 0, 4, 4, 10, 10)
  clip(0, 0, 22, 11)
  screen:blit(b, 0, 0, 4, 4, 20, 10)
  clip()
  draw_target(b)
  cls(2)
  pset(0, 0, 3)
  print(pget(0, 0))
  draw_target()
  print(pget(0, 0))
  log(tostring(screen), pcall(screen.release, screen))
  b:release()
  log(pcall(b.get, b, 0, 0))
  print(tostring(b))
  log(pcall(buf, 'u8', 5000, 1))
  log(pcall(buf, 'i64', 1, 1))
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    ok(&c);
    let log = c.output().log.to_vec();
    assert_eq!(log[0], "44\t7\t0\t4\t4\tu8\t1");
    assert_eq!(log[1], "0.5\tf32");
    assert!(log[2].contains("buf_kind_mismatch"), "{}", log[2]);
    assert_eq!(log[3], "3");
    assert_eq!(log[4], "0", "screen untouched by drawing into b");
    assert!(
        log[5].starts_with("buf(u8 320x240)\tfalse") && log[5].contains("buf_protected"),
        "{}",
        log[5]
    );
    assert!(log[6].contains("buf_released"), "{}", log[6]);
    assert_eq!(log[7], "buf(released)");
    assert!(log[8].contains("buf_bad_dimensions"), "{}", log[8]);
    assert!(log[9].contains("i64"), "{}", log[9]);
    assert_eq!(pixel(&c, 10, 10), 44);
    assert_eq!(pixel(&c, 11, 11), 7);
    assert_eq!(pixel(&c, 13, 13), 7);
    assert_eq!(pixel(&c, 20, 10), 44, "blit honours the clip");
    assert_eq!(pixel(&c, 21, 10), 7);
    assert_eq!(pixel(&c, 22, 10), 0);
    assert_eq!(pixel(&c, 20, 11), 0);
    assert_eq!(c.draw_state().res.ledger().used(), 16, "only f survives");
}

#[test]
fn collected_handles_return_their_bytes_at_the_next_step() {
    let src = "\
function _init()
  for i = 1, 4 do buf('u8', 100, 100) end
  keep = buf('u8', 10, 10)
end
function _update(dt)
  collectgarbage('collect')
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    ok(&c);
    assert_eq!(c.draw_state().res.ledger().used(), 40_100);
    run(&mut c, 1);
    // The collector ran during frame 2; the graveyard is reaped at the
    // start of the next step.
    run(&mut c, 1);
    assert_eq!(c.draw_state().res.ledger().used(), 100);
}

#[test]
fn a_cached_sheet_survives_while_any_handle_is_alive() {
    // Two handles share the cached buffer; collecting one must not free
    // it from under the other. Only the last handle buries it.
    let src = "\
function _init()
  keep = load_sheet('tiles')
  local other = load_sheet('tiles')
  other = nil
  collectgarbage('collect')
end
function _update(dt)
  if keep then log(keep:width(), tostring(keep)) end
  if frame == 2 then keep = nil end
  frame = (frame or 1) + 1
  collectgarbage('collect')
end
";
    let mut c = gfx_console(src);
    let before = c.draw_state().res.ledger().used();
    run(&mut c, 3);
    ok(&c);
    // `ok` above covers frame 2; this is frame 3's log, after two collects.
    assert_eq!(c.output().log.to_vec(), ["16\tbuf(u8 16x8)"]);
    assert_eq!(c.draw_state().res.ledger().used(), before + 128);
    // `keep` went in frame 3; the reap at the next step frees the sheet.
    run(&mut c, 1);
    ok(&c);
    assert_eq!(c.draw_state().res.ledger().used(), before);
}

#[test]
fn require_stops_at_the_module_limit() {
    let n = crate::require::MAX_MODULES + 1;
    let mut files: Vec<(String, Vec<u8>)> = (0..n)
        .map(|i| (format!("src/m{i}.lua"), b"return true\n".to_vec()))
        .collect();
    let main = format!(
        "function _init()
  local loaded = 0
  for i = 0, {n} - 1 do
    local ok, err = pcall(require, 'm' .. i)
    if ok then loaded = loaded + 1 else print(loaded, err) break end
  end
end
"
    );
    files.push(("main.lua".into(), main.into_bytes()));
    let entries: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    let mut c = Console::new(cart(&entries), LuaGuest::factory);
    run(&mut c, 1);
    ok(&c);
    let log = c.output().log.to_vec();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(
        log[0].starts_with(&format!("{}\t", crate::require::MAX_MODULES)),
        "{}",
        log[0]
    );
    assert!(log[0].contains("more than 256 modules"), "{}", log[0]);
}

#[test]
fn a_full_budget_collects_before_failing() {
    let src = "\
function _init()
  for i = 1, 2 do buf('u8', 2048, 2048) end
  local ok, err = pcall(buf, 'u8', 1, 1)
  print(ok, err)
  collectgarbage('collect')
end
function _update(dt)
  print(pcall(buf, 'u8', 2048, 2048))
end
";
    let mut c = gfx_console(src);
    run(&mut c, 2);
    ok(&c);
    let log = c.output().log.to_vec();
    assert!(
        log[0].starts_with("true	buf(u8 2048x2048)"),
        "the collector freed both unreferenced buffers: {}",
        log[0]
    );
}

#[test]
fn locked_ledger_over_cap_faults_with_the_budget_code() {
    let src = "\
keep = {}
function _init()
  for i = 1, 3 do keep[i] = buf('u8', 2048, 2048) end
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    let f = c.state().fault().cloned().unwrap();
    assert_eq!(f.code, "graphics_budget_exceeded");
    assert_eq!(f.location(), "main.lua:8");
}

#[test]
fn require_loads_once_caches_and_reports_module_lines() {
    let src = "\
local util = require('util')
local again = require('util')
function _init()
  log(util.double(21), util == again)
  log(require('nilmod'), require('nilmod'), counter)
  print(pcall(require, 'missing'))
  print(pcall(require, '../x'))
  print(pcall(require, 'binary'))
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    ok(&c);
    let log = c.output().log.to_vec();
    assert_eq!(log[0], "42\ttrue");
    assert_eq!(log[1], "true\ttrue\t1");
    assert!(
        log[2].contains("module_not_found") && log[2].contains("src/missing.lua"),
        "{}",
        log[2]
    );
    assert!(log[3].contains("module_name_invalid"), "{}", log[3]);
    assert!(
        log[4].starts_with("false") && log[4].contains("binary"),
        "{}",
        log[4]
    );
}

#[test]
fn require_cycle_faults_with_the_cycle_code() {
    let mut c = gfx_console("require('a')\n");
    run(&mut c, 1);
    let f = c.state().fault().cloned().unwrap();
    assert_eq!(f.code, "require_cycle");
    assert_eq!(f.file, "src/b.lua");
    assert_eq!(f.line, Some(1));
}

#[test]
fn errors_inside_modules_name_the_module_file() {
    let mut c = gfx_console("require('thrower')\n");
    run(&mut c, 1);
    let f = c.state().fault().cloned().unwrap();
    assert_eq!(f.code, "runtime_error");
    assert_eq!(f.location(), "src/thrower.lua:2");
    let mut c = gfx_console("require('bad')\n");
    run(&mut c, 1);
    let f = c.state().fault().cloned().unwrap();
    assert_eq!(f.code, "compile_error");
    assert_eq!(f.location(), "src/bad.lua:1");
}

#[test]
fn binary_chunks_and_string_dump_are_unreachable() {
    let src = "\
function _init()
  -- `load` is the save-slot binding: a string is not a slot.
  print(string.dump == nil, pcall(load, 'return 1') == false, loadstring == nil)
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    ok(&c);
    assert_eq!(c.output().log[0], "true\ttrue\ttrue");
}

#[test]
fn buf_metatables_are_locked() {
    let src = "\
function _init()
  print(getmetatable(screen))
  print(pcall(setmetatable, screen, {}))
  local b = buf('u8', 1, 1)
  print(getmetatable(b), pcall(function() b.get = nil end))
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    ok(&c);
    let log = c.output().log.to_vec();
    // mlua sets `__metatable` on userdata metatables itself.
    assert_eq!(log[0], "false");
    assert!(log[1].starts_with("false"), "{}", log[1]);
    assert!(log[2].starts_with("false	false"), "{}", log[2]);
}

#[test]
fn the_frame_log_is_bounded() {
    let src = "\
function _init()
  for i = 1, 300 do print(i) end
  print(string.rep('x', 5000))
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    ok(&c);
    let log = c.output().log;
    assert_eq!(log.len(), 257);
    assert!(log[256].contains("truncated"));
}

#[test]
fn a_cart_without_a_manifest_runs_at_640x480() {
    let mut c = console("function _init() cls(4) end");
    run(&mut c, 1);
    ok(&c);
    let out = c.output();
    assert_eq!((out.width, out.height), (640, 480));
    assert_eq!(out.screen.len(), 640 * 480);
}

#[test]
fn preloaded_assets_are_the_same_buffers_load_returns() {
    let png = sheet_png();
    let src = cart(&[
        (
            "main.lua",
            b"function _init() local s = load_sheet('tiles') sheet(s) spr(1, 0, 0) end",
        ),
        ("cart.toml", b"[preload]\nsheets = [\"tiles\"]\n"),
        ("gfx/tiles.png", &png),
    ]);
    let mut c = Console::new(src, LuaGuest::factory);
    assert_eq!(c.draw_state().res.ledger().used(), 128);
    c.step(FrameInput::NONE);
    ok(&c);
    assert_eq!(c.draw_state().res.ledger().used(), 128, "no second decode");
    assert_eq!(pixel(&c, 0, 0), 3);
}

/// Each pricing family of the generated reference, measured through
/// the meter: the descriptors say what a call costs, this says the
/// binding charges it.
#[test]
fn priced_families_charge_what_the_reference_says() {
    let src = "\
function _init()
  cls(0)
  local function delta(f)
    local a = stat('cpu_cycles')
    f()
    return stat('cpu_cycles') - a - 1
  end
  local s
  local sheet_first = delta(function() s = load_sheet('tiles') end)
  local sheet_again = delta(function() load_sheet('tiles') end)
  sheet(s)
  local m = load_map('level')
  local map_cost = delta(function() map(m, 0, 0, 100, 100, 2, 2) end)
  local text_cost = delta(function() print('AB', 0, 0, 7) end)
  local b
  local buf_cost = delta(function() b = buf('u8', 64, 64) end)
  local fill_cost = delta(function() b:fill(1) end)
  local copy_cost = delta(function() b:copy(s, 0, 0, 16, 8, 0, 0) end)
  local line = string.rep('x', 80)
  local log_cost = delta(function() print(line) end)
  local t = {8, 7, 6, 5, 4, 3, 2, 1}
  local sort_cost = delta(function() table.sort(t) end)
  local ink = 0
  for y = 0, 5 do for x = 0, 7 do if pget(x, y) == 7 then ink = ink + 1 end end end
  log(sheet_first, sheet_again, map_cost, text_cost, ink, fill_cost, copy_cost, log_cost, sort_cost, buf_cost)
end
";
    let mut c = gfx_console(src);
    run(&mut c, 1);
    ok(&c);
    // The measured `print(line)` is the first log line; the numbers
    // are the last.
    let log = c.output().log.to_vec();
    let n: Vec<u64> = log[1].split('\t').map(|p| p.parse().unwrap()).collect();
    assert_eq!(n[0], 1 + 1 + 128 / 8, "load_sheet decodes 128 bytes");
    assert_eq!(n[0], Price::asset(Some(128)), "as the reference says");
    assert_eq!(n[1], 1, "a live sheet costs one cycle");
    assert_eq!(n[1], Price::asset(None), "as the reference says");
    assert_eq!(n[9], 1 + 1 + 4096 / 128, "buf: accepted, then by bytes");
    assert_eq!(
        n[9],
        Price::Alloc.formula().unwrap().eval(&[4096]),
        "as the reference says"
    );
    // Three of the four cells draw, 64 pixels each, 2 cycles per cell.
    assert_eq!(n[2], 4 * 2 + 1 + 3 * 64 / 3, "map");
    assert_eq!(n[3], 2 + n[4] / 3, "print draws two characters");
    assert!(n[4] > 0, "the glyphs drew something");
    assert_eq!(n[5], 1 + 4096 / 128, "fill by bytes");
    assert_eq!(n[6], 1 + 128 / 128, "copy by bytes");
    assert_eq!(n[7], 1 + 80 / 8, "log line by bytes");
    assert_eq!(n[8], 1 + 8 * 4, "sort n log2 n");
}
