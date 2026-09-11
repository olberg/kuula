//! The recorded pair of `examples/netbuttons`: two consoles from the
//! example over the in-process memory transport, scripted inputs on
//! each side, both recorded. The transcripts and hashes under
//! `examples/netbuttons/replay/` must match what this produces, and
//! `determinism.rs` replays them through the binary. They are
//! conformance fixtures: regenerate with `KUULA_UPDATE_FIXTURES=1`
//! only for an intended change, and say why in the commit.

use std::path::PathBuf;
use std::rc::Rc;

use kuula_core::input::{BTN_A, BTN_B, BTN_DOWN, BTN_LEFT, BTN_RIGHT, BTN_UP};
use kuula_core::net::{Link, MemoryTransport, NetEnv, MEMORY_TICKET};
use kuula_core::transcript::Header;
use kuula_core::{Console, FrameInput, Recorder, RecordingGuest, Snapshot, SnapshotLimits};
use kuula_host_headless::{hashes_text, Hasher};
use kuula_lua::LuaGuest;

const FRAMES: u64 = 90;

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("netbuttons")
}

/// The host presses a run of each button, the joiner presses A and B,
/// then the joiner leaves on frame 70 and both keep running.
fn host_input(frame: u64) -> FrameInput {
    let bits = match frame {
        10..=14 => BTN_UP,
        20..=24 => BTN_DOWN,
        30..=34 => BTN_LEFT | BTN_A,
        40..=44 => BTN_RIGHT,
        _ => 0,
    };
    FrameInput::new(bits)
}

fn join_input(frame: u64) -> FrameInput {
    let bits = match frame {
        15..=17 => BTN_A,
        25..=27 => BTN_B,
        35..=36 => BTN_A | BTN_B,
        _ => 0,
    };
    FrameInput::new(bits)
}

struct Side {
    console: Console,
    link: Link,
    recorder: kuula_core::SharedRecorder,
    hasher: Hasher,
    per_frame: Vec<String>,
}

fn side(snap: &Rc<Snapshot>, transport: MemoryTransport, invite: Option<&str>) -> Side {
    let recorder = Recorder::shared(Header::new(snap, kuula_lua::RANDOM_SEED, Vec::new()));
    let mut console = Console::new(
        snap.clone(),
        RecordingGuest::factory(LuaGuest::factory, Some(recorder.clone())),
    );
    console.set_net_env(NetEnv {
        permitted: true,
        invite: invite.map(str::to_string),
    });
    Side {
        console,
        link: Link::over(Box::new(transport)),
        recorder,
        hasher: Hasher::new(),
        per_frame: Vec::new(),
    }
}

impl Side {
    fn step(&mut self, input: FrameInput) {
        self.console.step_linked(&mut self.link, input);
        let out = self.console.output();
        assert_eq!(
            self.console.state().fault(),
            None,
            "{:?}",
            self.console.state()
        );
        let mut one = Hasher::new();
        one.parts(out.screen, out.palette, out.audio);
        self.per_frame.push(one.hex());
        self.hasher.parts(out.screen, out.palette, out.audio);
    }
}

/// Run the pair; the transcripts and hash lists of both sides.
fn record() -> (String, String, String, String) {
    let snap = Rc::new(Snapshot::from_dir(&example(), SnapshotLimits::default()).unwrap());
    let (a, b) = MemoryTransport::pair();
    let mut host = side(&snap, a, None);
    let mut joiner = side(&snap, b, Some(MEMORY_TICKET));
    for frame in 1..=FRAMES {
        host.step(host_input(frame));
        if frame == 70 {
            // The joiner leaves through the cart's own call: a `leave`
            // is what the example does not do by itself, so the test
            // closes the joiner's link directly, which the host sees
            // as its peer leaving.
            joiner.link.set_permitted(false);
        }
        joiner.step(join_input(frame));
    }
    let hs = host.console.net_state().unwrap();
    assert_eq!(hs.status.as_str(), "hosting", "{hs:?}");
    assert_eq!(hs.received, 4, "{hs:?}");
    let js = joiner.console.net_state().unwrap();
    assert_eq!(js.status.as_str(), "ended", "{js:?}");
    assert!(!js.permitted);
    let host_kr = host.recorder.borrow().transcript().encode().unwrap();
    let join_kr = joiner.recorder.borrow().transcript().encode().unwrap();
    (
        host_kr,
        join_kr,
        hashes_text(&host.per_frame),
        hashes_text(&joiner.per_frame),
    )
}

#[test]
fn the_recorded_pair_matches_the_checked_in_fixtures() {
    let (host_kr, join_kr, host_hashes, join_hashes) = record();
    assert!(host_kr.starts_with("kuula-transcript 2\n"), "{host_kr}");
    assert!(join_kr.contains("invite = \"memory:pair\""), "{join_kr}");
    assert!(host_kr.contains("kind = \"hosting\""), "{host_kr}");
    assert!(host_kr.contains("op = \"send\""), "{host_kr}");
    assert!(join_kr.contains("kind = \"permission\""), "{join_kr}");
    let dir = example().join("replay");
    let files = [
        ("host.kr", host_kr),
        ("join.kr", join_kr),
        ("host-hashes.txt", host_hashes),
        ("join-hashes.txt", join_hashes),
    ];
    if std::env::var_os("KUULA_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in &files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        return;
    }
    for (name, text) in &files {
        let checked_in = std::fs::read_to_string(dir.join(name))
            .unwrap_or_else(|e| panic!("{name}: {e}; run with KUULA_UPDATE_FIXTURES=1"));
        assert_eq!(
            &checked_in, text,
            "{name} differs from the checked-in fixture; if intended, regenerate with KUULA_UPDATE_FIXTURES=1 and say why"
        );
    }
}

/// The same pair twice is the same pair: the run is deterministic
/// before any file is involved.
#[test]
fn the_pair_is_deterministic() {
    assert_eq!(record(), record());
}
