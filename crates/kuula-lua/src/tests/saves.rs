//! `save`, `load` and `Guest::state` through the canonical codec.

use std::cell::RefCell;
use std::rc::Rc;

use super::{console, run};
use kuula_core::save::{MemoryStore, SaveError, SaveStore};
use kuula_core::{Console, FrameInput};

/// A store two consoles can share, standing in for a file on disk.
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<MemoryStore>>);

impl SaveStore for Shared {
    fn read(&mut self, slot: u8) -> Result<Option<Vec<u8>>, SaveError> {
        self.0.borrow_mut().read(slot)
    }
    fn write(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError> {
        self.0.borrow_mut().write(slot, bytes)
    }
}

fn console_with(src: &str, store: &Shared) -> Console {
    let mut c = console(src);
    c.set_save_store(Box::new(store.clone()));
    c
}

fn slot_text(store: &Shared, slot: u8) -> Option<String> {
    store
        .0
        .borrow_mut()
        .read(slot)
        .unwrap()
        .map(|b| String::from_utf8(b).unwrap())
}

fn log(c: &Console) -> Vec<String> {
    c.output().log.to_vec()
}

#[test]
fn load_of_an_empty_slot_is_nil() {
    let mut c = console("print(load(0) == nil, load(7) == nil)");
    c.step(FrameInput::NONE);
    assert_eq!(log(&c), ["true\ttrue"]);
    assert!(c.state().fault().is_none());
}

#[test]
fn a_counter_persists_across_consoles_sharing_a_store() {
    let store = Shared::default();
    let src = "local d = load(0) or { runs = 0 }\nd.runs = d.runs + 1\nsave(0, d)\nprint(d.runs)";
    for expected in ["1", "2", "3"] {
        let mut c = console_with(src, &store);
        c.step(FrameInput::NONE);
        assert_eq!(log(&c), [expected]);
        assert!(c.state().fault().is_none());
    }
    assert_eq!(slot_text(&store, 0).unwrap(), "{ runs = 3 }");
}

#[test]
fn the_slot_holds_the_canonical_text() {
    let store = Shared::default();
    let mut c = console_with(
        "save(1, { x = 1.5, [1] = 2, k = 'v', y = true, n = { -0.0, 'a\\n' }, e = {} })",
        &store,
    );
    c.step(FrameInput::NONE);
    assert!(c.state().fault().is_none(), "{:?}", c.state());
    assert_eq!(
        slot_text(&store, 1).unwrap(),
        "{ [1] = 2, e = {}, k = \"v\", n = { [1] = -0.0, [2] = \"a\\n\" }, x = 1.5, y = true }"
    );
    assert_eq!(slot_text(&store, 0), None);
}

#[test]
fn a_cycle_is_a_catchable_error_with_its_code() {
    let mut c = console(
        "local t = {}\nt.me = t\nlocal ok, err = pcall(save, 0, t)\nprint(ok, err)\n\
         print(load(0) == nil)\nsave(0, t)",
    );
    c.step(FrameInput::NONE);
    let lines = log(&c);
    assert!(lines[0].starts_with("false\t"), "{lines:?}");
    assert!(lines[0].contains("codec_cycle"), "{lines:?}");
    assert_eq!(lines[1], "true", "nothing was written");
    let fault = c
        .state()
        .fault()
        .cloned()
        .expect("uncaught save fails the cart");
    assert_eq!(fault.code, "codec_cycle");
    assert_eq!(fault.location(), "main.lua:6");
}

#[test]
fn unsupported_values_and_bad_slots_have_codes() {
    let mut c = console(
        "print(select(2, pcall(save, 0, { f = print })))\n\
         print(select(2, pcall(save, 0, { c = coroutine.create(function() end) })))\n\
         print(select(2, pcall(save, 0, { [{}] = 1 })))\n\
         print(select(2, pcall(save, 0, { x = 0/0 })))\n\
         print(select(2, pcall(save, 8, {})))\n\
         print(select(2, pcall(load, -1)))\n\
         print(select(2, pcall(save, 0, 'no')))",
    );
    c.step(FrameInput::NONE);
    let lines = log(&c);
    let codes = [
        "codec_unsupported",
        "codec_unsupported",
        "codec_key",
        "codec_number",
        "save_slot",
        "save_slot",
        "save takes a table",
    ];
    for (line, code) in lines.iter().zip(codes) {
        assert!(line.contains(code), "{line} should mention {code}");
    }
    assert_eq!(lines.len(), codes.len());
    assert!(c.state().fault().is_none());
}

#[test]
fn a_buf_saves_and_loads_as_a_blob() {
    let store = Shared::default();
    let mut c = console_with(
        "local b = buf('i16', 3, 2)\nb:set(0, 0, -5)\nb:set(2, 1, 300)\n\
         save(2, { img = b, n = 1 })\n\
         local d = load(2)\n\
         print(d.n, d.img:kind(), d.img:width(), d.img:height(), d.img:get(0, 0), d.img:get(2, 1), d.img:get(1, 0))\n\
         print(d.img ~= b)",
        &store,
    );
    c.step(FrameInput::NONE);
    assert!(c.state().fault().is_none(), "{:?}", c.state());
    assert_eq!(log(&c), ["1\ti16\t3\t2\t-5\t300\t0", "true"]);
    let text = slot_text(&store, 2).unwrap();
    // "KBUF" then kind 1, width 3, height 2 in the header.
    assert!(text.starts_with("{ img = blob\"S0JVRg"), "{text}");
    assert!(text.ends_with(", n = 1 }"), "{text}");
}

#[test]
fn a_blob_without_a_buf_header_does_not_load() {
    let store = Shared::default();
    store
        .0
        .borrow_mut()
        .write(0, b"{ b = blob\"AAEC\" }")
        .unwrap();
    let mut c = console_with("print(select(2, pcall(load, 0)))", &store);
    c.step(FrameInput::NONE);
    assert!(log(&c)[0].contains("codec_unsupported"), "{:?}", log(&c));
}

#[test]
fn a_corrupt_slot_is_a_syntax_error_not_code() {
    let store = Shared::default();
    store
        .0
        .borrow_mut()
        .write(0, b"{ x = os.execute('rm -rf /') }")
        .unwrap();
    let mut c = console_with("local ok, err = pcall(load, 0)\nprint(ok, err)", &store);
    c.step(FrameInput::NONE);
    let line = &log(&c)[0];
    assert!(
        line.starts_with("false\t") && line.contains("codec_syntax"),
        "{line}"
    );
}

#[test]
fn metamethods_do_not_run_during_a_save() {
    let store = Shared::default();
    let mut c = console_with(
        "local hits = 0\n\
         local t = setmetatable({ real = 1 }, {\n\
           __index = function() hits = hits + 1 return 99 end,\n\
           __pairs = function() hits = hits + 1 return function() end end,\n\
         })\n\
         save(0, t)\nprint(hits)",
        &store,
    );
    c.step(FrameInput::NONE);
    assert_eq!(log(&c), ["0"]);
    assert_eq!(slot_text(&store, 0).unwrap(), "{ real = 1 }");
}

#[test]
fn saves_are_priced_by_bytes() {
    let mut c = console(
        "local before = stat('cpu_cycles')\nsave(0, {})\nlocal small = stat('cpu_cycles') - before\n\
         local big = {}\nfor i = 1, 200 do big[i] = 'abcdefgh' end\n\
         before = stat('cpu_cycles')\nsave(1, big)\nlocal large = stat('cpu_cycles') - before\n\
         print(small >= 64, large > small + 200)",
    );
    c.step(FrameInput::NONE);
    assert_eq!(log(&c), ["true\ttrue"]);
}

#[test]
fn state_dumps_named_globals_leniently() {
    let mut c = console(
        "score = 3\nname = 'a'\nt = { 1, 2, nested = { ok = true } }\nf = print\n\
         b = buf('u8', 1, 1)\nloop = {}\nloop.me = loop\nfunction _update() end",
    );
    run(&mut c, 2);
    let dump = c
        .state_dump(&[
            "score".into(),
            "name".into(),
            "t".into(),
            "f".into(),
            "b".into(),
            "loop".into(),
            "missing".into(),
        ])
        .unwrap();
    assert_eq!(
        dump,
        "{ b = \"<buf>\", f = \"<function>\", loop = { me = \"<cycle>\" }, name = \"a\", \
         score = 3, t = { [1] = 1, [2] = 2, nested = { ok = true } } }"
    );
    assert!(c.state().fault().is_none());
}

#[test]
fn state_omits_entries_with_unsupported_keys_but_bounds_the_walk() {
    let mut c = console("t = { [print] = 1, x = 2 }\nfunction _update() end");
    run(&mut c, 1);
    assert_eq!(c.state_dump(&["t".into()]).unwrap(), "{ t = { x = 2 } }");
    let mut c = console("big = {}\nfor i = 1, 70000 do big[i] = true end\nfunction _update() end");
    run(&mut c, 1);
    let err = c.state_dump(&["big".into()]).unwrap_err();
    assert_eq!(err.code, "codec_size");
}

#[test]
fn a_save_that_fails_on_size_still_pays_for_the_walk() {
    let mut c = console(
        "local big = {}\nfor i = 1, 300 do big[i] = string.rep('x', 1000) end\n\
         local before = stat('cpu_cycles')\nlocal ok, err = pcall(save, 0, big)\n\
         local cost = stat('cpu_cycles') - before\n\
         print(ok, tostring(err):find('codec_size') ~= nil, cost > 30000)",
    );
    c.step(FrameInput::NONE);
    assert_eq!(log(&c), ["false\ttrue\ttrue"], "{:?}", c.state().fault());
}

#[test]
fn a_lenient_dump_does_not_leak_depth_across_dropped_tables() {
    // `g` reaches the depth limit (the dump's root table is level 1);
    // its innermost table holds 32 tables that are each dropped as
    // `<depth>`. The sibling global `b` must still dump whole.
    let mut c = console(
        "g = {}\nlocal t = g\nfor i = 1, 30 do t.n = {} t = t.n end\n\
         for i = 1, 32 do t[i] = {} end\nb = { ok = true }\nfunction _update() end",
    );
    run(&mut c, 1);
    let dump = c.state_dump(&["g".into(), "b".into()]).unwrap();
    assert!(dump.contains("\"<depth>\""), "{dump}");
    assert!(dump.contains("b = { ok = true }"), "{dump}");
}
