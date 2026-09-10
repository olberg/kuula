//! `--record` and `--replay` end to end: a recorded run
//! replays to the same hashes in process and through the worker, a
//! transcript from another cart is refused, and the initial save slots
//! in a transcript reach the cart.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

fn kuula() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kuula"))
}

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join(name)
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("kuula-transcript-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn hash_line(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout)
        .lines()
        .find(|l| l.starts_with("frames "))
        .map(|l| l.to_string())
        .unwrap_or_default()
}

fn run(args: &[&str]) -> std::process::Output {
    kuula().args(args).output().unwrap()
}

#[test]
fn record_then_replay_reproduces_the_hashes() {
    let s = Scratch::new();
    let script = s.path("in.json");
    std::fs::write(
        &script,
        r#"[{"frames": 20, "buttons": ["right"]}, {"frames": 5, "buttons": ["a", "down"]}, {"frames": 10}]"#,
    )
    .unwrap();
    let hello = example("hello");
    let hello = hello.to_str().unwrap();
    let kr = s.path("run.kr");
    let (d1, d2) = (s.path("d1"), s.path("d2"));

    let recorded = run(&[
        "run",
        hello,
        "--headless",
        "--frames",
        "40",
        "--input",
        &script,
        "--record",
        &kr,
        "--out",
        &d1,
    ]);
    assert_eq!(
        recorded.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&recorded.stderr)
    );
    assert!(String::from_utf8_lossy(&recorded.stdout).contains("recorded 40 frames"));
    let text = std::fs::read_to_string(&kr).unwrap();
    assert!(
        text.starts_with("kuula-transcript 1\n{ cart = \"0x"),
        "{text}"
    );
    assert!(text.contains("{ buttons = 8, frames = 20 }\n"), "{text}");
    assert!(text.contains("{ buttons = 18, frames = 5 }\n"), "{text}");
    assert!(text.ends_with("{ buttons = 0, frames = 15 }\n"), "{text}");

    let replayed = run(&["run", hello, "--headless", "--replay", &kr, "--out", &d2]);
    assert_eq!(
        replayed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&replayed.stderr)
    );
    let (h1, h2) = (hash_line(&recorded.stdout), hash_line(&replayed.stdout));
    assert!(h1.starts_with("frames 40 "), "{h1}");
    assert_eq!(h1, h2);
    assert_eq!(
        std::fs::read_to_string(s.0.join("d1").join("hashes.txt")).unwrap(),
        std::fs::read_to_string(s.0.join("d2").join("hashes.txt")).unwrap()
    );

    // The worker records exactly what it was sent, and the transcript is
    // byte for byte the in-process one.
    let kr2 = s.path("worker.kr");
    let worker = run(&[
        "run",
        hello,
        "--headless",
        "--frames",
        "40",
        "--input",
        &script,
        "--worker",
        "--record",
        &kr2,
    ]);
    assert_eq!(
        worker.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&worker.stderr)
    );
    assert_eq!(hash_line(&worker.stdout), h1);
    assert_eq!(std::fs::read_to_string(&kr2).unwrap(), text);

    // A replay with fewer frames stops early; with more, the extra
    // frames get no input.
    let short = run(&[
        "run",
        hello,
        "--headless",
        "--replay",
        &kr,
        "--frames",
        "10",
    ]);
    assert!(hash_line(&short.stdout).starts_with("frames 10 "));
}

#[test]
fn replay_refuses_another_cart_unless_told() {
    let s = Scratch::new();
    let kr = s.path("hello.kr");
    let hello = example("hello");
    let saves = example("saves");
    let ok = run(&[
        "run",
        hello.to_str().unwrap(),
        "--headless",
        "--frames",
        "5",
        "--record",
        &kr,
    ]);
    assert_eq!(ok.status.code(), Some(0));

    let refused = run(&[
        "run",
        saves.to_str().unwrap(),
        "--headless",
        "--replay",
        &kr,
    ]);
    assert_eq!(refused.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("transcript_cart_mismatch"), "{stderr}");
    assert!(stderr.contains("--replay-any-cart"), "{stderr}");
    assert!(!String::from_utf8_lossy(&refused.stdout).contains("frames "));

    let forced = run(&[
        "run",
        saves.to_str().unwrap(),
        "--headless",
        "--replay",
        &kr,
        "--replay-any-cart",
    ]);
    assert_eq!(
        forced.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert!(String::from_utf8_lossy(&forced.stderr).contains("verifies nothing"));
    assert!(hash_line(&forced.stdout).starts_with("frames 5 "));

    // A damaged transcript is a usage error naming the problem.
    let text = std::fs::read_to_string(&kr).unwrap();
    let cut = text.trim_end().rsplit_once('\n').unwrap().0.to_string();
    std::fs::write(&kr, cut).unwrap();
    let bad = run(&[
        "run",
        hello.to_str().unwrap(),
        "--headless",
        "--replay",
        &kr,
    ]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("transcript_format"));
}

#[test]
fn the_saves_cart_replays_and_its_initial_slots_reach_it() {
    let s = Scratch::new();
    let script = s.path("in.json");
    std::fs::write(
        &script,
        r#"[{"frames": 3}, {"frames": 1, "buttons": ["a"]}, {"frames": 3}, {"frames": 1, "buttons": ["a"]}, {"frames": 5}]"#,
    )
    .unwrap();
    let saves = example("saves");
    let saves = saves.to_str().unwrap();
    let kr = s.path("saves.kr");
    let recorded = run(&[
        "run",
        saves,
        "--headless",
        "--frames",
        "13",
        "--input",
        &script,
        "--record",
        &kr,
    ]);
    assert_eq!(
        recorded.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&recorded.stderr)
    );
    let replayed = run(&["run", saves, "--headless", "--replay", &kr]);
    assert_eq!(replayed.status.code(), Some(0));
    let plain = hash_line(&recorded.stdout);
    assert_eq!(plain, hash_line(&replayed.stdout));

    // The same transcript with slot 0 holding an earlier run: the cart
    // boots into run 6 instead of run 1 and draws different numbers.
    let text = std::fs::read_to_string(&kr).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let slot = kuula_core::codec::base64_encode(b"{ best = 0, presses = 0, runs = 5 }");
    lines.insert(
        2,
        format!("{{ data = blob\"{slot}\", part = 1, parts = 1, slot = 0 }}"),
    );
    let seeded = s.path("seeded.kr");
    std::fs::write(&seeded, lines.join("\n") + "\n").unwrap();
    let with_slot = run(&["run", saves, "--headless", "--replay", &seeded]);
    assert_eq!(
        with_slot.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&with_slot.stderr)
    );
    let seeded_hash = hash_line(&with_slot.stdout);
    assert!(seeded_hash.starts_with("frames 13 "), "{seeded_hash}");
    assert_ne!(
        seeded_hash, plain,
        "the initial slot changed what the cart drew"
    );
    // And the replay left nothing on disk: the player's slots are not
    // a replay's store.
    let again = run(&["run", saves, "--headless", "--replay", &seeded]);
    assert_eq!(hash_line(&again.stdout), seeded_hash);
}
