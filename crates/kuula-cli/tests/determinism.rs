//! Conformance hashes. If a value here or in
//! `examples/conformance/hashes.txt` changes, the change to the console's
//! observable behaviour must be intended; update it in the same commit
//! and say why.

use std::path::PathBuf;
use std::rc::Rc;

use kuula_core::input::{BTN_A, BTN_B, BTN_DOWN, BTN_LEFT, BTN_RIGHT, BTN_UP};
use kuula_core::{FrameInput, Snapshot, SnapshotLimits};
use kuula_host_headless::Hasher;
use kuula_lua::LuaGuest;

/// The first hash of `examples/hello` was 0xe904c9fb68835452 and
/// survived the drawing API. Folding each frame's audio into the hash
/// after the palette changed it; the cart is silent, so the pixels are
/// as before and only the 1470 zero bytes per frame moved the value.
const HELLO_HASH: u64 = 0x892b2d0255430ec2;
const HELLO_FRAMES: u64 = 120;

/// The scripted input: ten-frame runs of each button, then a chord, then
/// nothing, repeated.
fn input_for(frame: u64) -> FrameInput {
    let phase = (frame / 10) % 8;
    let bits = match phase {
        0 => BTN_RIGHT,
        1 => BTN_DOWN,
        2 => BTN_LEFT | BTN_A,
        3 => BTN_UP,
        4 => BTN_RIGHT | BTN_DOWN | BTN_B,
        5 => BTN_A | BTN_B,
        6 => BTN_LEFT | BTN_UP,
        _ => 0,
    };
    FrameInput::new(bits)
}

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join(name)
}

fn snapshot(name: &str) -> Rc<Snapshot> {
    Rc::new(Snapshot::from_dir(&example(name), SnapshotLimits::default()).unwrap())
}

#[test]
fn hello_cart_hash_is_stable() {
    let mut console = LuaGuest::console(snapshot("hello"));
    let mut hash = Hasher::new();
    for frame in 0..HELLO_FRAMES {
        let out = console.step(input_for(frame));
        assert_eq!((out.width, out.height), (320, 240));
        hash.parts(out.screen, out.palette, out.audio);
    }
    assert_eq!(console.state().fault(), None, "{:?}", console.state());
    assert_eq!(
        hash.value(),
        HELLO_HASH,
        "conformance hash changed: got {}, expected {HELLO_HASH:#018x}",
        hash.hex()
    );
}

#[test]
fn two_runs_produce_the_same_hash() {
    let run = || {
        let mut console = LuaGuest::console(snapshot("hello"));
        let mut hash = Hasher::new();
        for frame in 0..HELLO_FRAMES {
            let out = console.step(input_for(frame));
            hash.parts(out.screen, out.palette, out.audio);
        }
        hash.value()
    };
    assert_eq!(run(), run());
}

/// The second oracle: the numeric cart's log against
/// `examples/numeric/expected.txt`, which pins the line count, the hash
/// of the body and the tail verbatim, so a failure says which moved.
#[test]
fn numeric_cart_matches_pinned_output() {
    let pinned = std::fs::read_to_string(example("numeric").join("expected.txt")).unwrap();
    let mut lines = None;
    let mut hash = None;
    let mut tail = Vec::new();
    let mut in_tail = false;
    for line in pinned.lines() {
        if in_tail {
            tail.push(line.to_string());
        } else if let Some(n) = line.strip_prefix("lines ") {
            lines = Some(n.parse::<usize>().unwrap());
        } else if let Some(h) = line.strip_prefix("fnv1a64 ") {
            hash = Some(h.to_string());
        } else if line == "tail" {
            in_tail = true;
        }
    }
    let (lines, hash) = (lines.unwrap(), hash.unwrap());
    assert_eq!(tail.len(), 23);

    let mut console = LuaGuest::console(snapshot("numeric"));
    let summary = kuula_host_headless::run(&mut console, 502, &[], |_, _| {});
    assert_eq!(summary.state, "running", "{:?}", summary.fault);
    let log = summary.log;
    assert_eq!(
        log.len(),
        lines,
        "the numeric cart printed a different number of lines"
    );
    assert_eq!(
        &log[log.len() - tail.len()..],
        &tail[..],
        "the tail of examples/numeric differs (specials or NaN spelling)"
    );
    let mut h = Hasher::new();
    for line in &log {
        h.bytes(line.as_bytes());
        h.bytes(b"\n");
    }
    assert_eq!(
        h.hex(),
        hash,
        "the body of examples/numeric differs; dump it with `kuula run examples/numeric --headless --frames 502`"
    );
}

/// The conformance cart against its checked-in per-frame
/// hashes, so a failure names the first frame that differs.
#[test]
fn conformance_cart_matches_checked_in_hashes() {
    let expected = std::fs::read_to_string(example("conformance").join("hashes.txt")).unwrap();
    let expected: Vec<(u64, String)> = expected
        .lines()
        .map(|l| {
            let (n, h) = l.split_once(' ').unwrap();
            (n.parse().unwrap(), h.to_string())
        })
        .collect();
    assert_eq!(expected.len(), 60);

    let mut console = LuaGuest::console(snapshot("conformance"));
    let mut whole = Hasher::new();
    for (n, want) in &expected {
        let out = console.step(FrameInput::NONE);
        assert_eq!(out.frame, *n);
        let mut one = Hasher::new();
        one.parts(out.screen, out.palette, out.audio);
        assert_eq!(
            &one.hex(),
            want,
            "frame {n} differs; regenerate examples/conformance/hashes.txt if intended"
        );
        whole.parts(out.screen, out.palette, out.audio);
        assert_eq!(console.state().fault(), None, "{:?}", console.state());
    }
    // The same run through the headless runner gives the same digest.
    let mut console = LuaGuest::console(snapshot("conformance"));
    let summary = kuula_host_headless::run(&mut console, 60, &[], |_, _| {});
    assert_eq!(summary.hash, whole.hex());
    assert_eq!(summary.state, "running");
}
