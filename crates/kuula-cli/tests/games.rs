//! The games shipped beside the examples, `alien-invaders`, `fenlight` and
//! `kilnhollow`, load and play: their sheets, songs and banks are taken, and
//! fifteen seconds of play with a button pressed now and then end in no
//! fault. A whole run of Fenlight is a test of its own, and is not run by
//! default: it is five minutes of play. A whole walk through Kilnhollow is
//! one too, and is not run by default either.

use std::path::PathBuf;
use std::rc::Rc;

use kuula_core::input::{BTN_A, BTN_DOWN, BTN_LEFT, BTN_RIGHT, BTN_UP};
use kuula_core::{FrameInput, Snapshot, SnapshotLimits};
use kuula_lua::LuaGuest;

fn game(name: &str) -> Rc<Snapshot> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(name);
    Rc::new(Snapshot::from_dir(&dir, SnapshotLimits::default()).unwrap())
}

#[test]
fn the_shipped_games_play_without_a_fault() {
    for name in ["alien-invaders", "fenlight", "kilnhollow"] {
        let mut console = LuaGuest::console(game(name));
        let mut heard = false;
        for frame in 0..900u64 {
            // A on one frame in thirty starts a run, fires and takes an
            // upgrade; between presses the player walks right, or in
            // Kilnhollow turns right.
            let bits = if frame % 30 == 5 { BTN_A } else { BTN_RIGHT };
            let out = console.step(FrameInput::new(bits));
            heard |= out.audio.iter().any(|&s| s != 0);
        }
        assert_eq!(console.state().fault(), None, "{name}");
        assert!(heard, "{name} made no sound");
    }
}

/// Fenlight from its title to dawn. The player walks round an octagon, 72
/// frames a side, and presses A twice on each side, which starts the run
/// and takes the first upgrade a level offers. That is enough to live
/// through the night, so the run has all of the game in it: the swarm at
/// its peak with the bursts and the hulks on top, every weapon, the music
/// turning panicky and back, and the end of the run.
///
/// A run is the same on every machine, so this either passes everywhere or
/// nowhere. When a change to the game makes this player lose, the test no
/// longer reaches the heavy end of the run: give it a walk that survives.
#[test]
#[ignore = "five minutes of play, over a minute unoptimised: \
            cargo test --release -p kuula-cli --test games -- --ignored"]
fn a_whole_run_of_fenlight_ends_at_dawn_without_a_fault() {
    const SIDES: [u16; 8] = [
        BTN_RIGHT,
        BTN_DOWN | BTN_RIGHT,
        BTN_DOWN,
        BTN_DOWN | BTN_LEFT,
        BTN_LEFT,
        BTN_UP | BTN_LEFT,
        BTN_UP,
        BTN_UP | BTN_RIGHT,
    ];
    // The run is 18000 frames, and the game waits while a level's menu is
    // open: a few dozen levels at up to a side each.
    const LIMIT: u64 = 24_000;
    let state = ["state".to_string()];

    let mut console = LuaGuest::console(game("fenlight"));
    let mut ended = None;
    for frame in 0..LIMIT {
        let bits = match frame {
            0..=4 => 0,
            5 => BTN_A,
            _ => {
                let at = frame - 6;
                let side = SIDES[(at / 72 % 8) as usize];
                if matches!(at % 72, 60 | 71) {
                    side | BTN_A
                } else {
                    side
                }
            }
        };
        console.step(FrameInput::new(bits));
        if let Some(fault) = console.state().fault() {
            panic!("frame {frame}: {fault:?}");
        }
        // An end screen stays for 45 frames at least before A leaves it.
        if frame % 16 == 0 {
            let dump = console.state_dump(&state).unwrap();
            if dump.contains("won") || dump.contains("lost") {
                ended = Some((frame, dump));
                break;
            }
        }
    }
    let (frame, dump) = ended.expect("the run ended");
    assert!(dump.contains("won"), "frame {frame}: {dump}");
    assert!(frame >= 18_000, "frame {frame}");
}

/// The buttons held on each frame of the recorded walk through Kilnhollow:
/// it starts the run with A, L2 and R2, which is the caretaker's mercy, and
/// then goes through every room, opens the gate with the key and ends at the
/// lever.
fn kilnhollow_walk() -> Vec<u16> {
    let mut frames = Vec::new();
    for line in include_str!("kilnhollow-walk.txt").lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (bits, count) = line.split_once(' ').unwrap();
        let bits: u16 = bits.parse().unwrap();
        let count: usize = count.parse().unwrap();
        frames.extend(std::iter::repeat_n(bits, count));
    }
    frames
}

/// The number after `name = ` in a state dump.
fn dumped(dump: &str, name: &str) -> f64 {
    let at = dump
        .find(&format!("{name} = "))
        .unwrap_or_else(|| panic!("{name} in {dump}"));
    let rest = &dump[at + name.len() + 3..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e'))
        .unwrap_or(rest.len());
    rest[..end].parse().unwrap()
}

/// Kilnhollow from its title to the lever, along the recorded walk: every
/// room, the key, the furnace doors, the gate. The run must end at the lever
/// (the screen of results) with no fault, and no frame of it may have used
/// more than three quarters of what a frame is allowed.
///
/// The walk is a recording of a bot: when a change to the level or to how a
/// creature moves leaves it stuck, record it again.
#[test]
#[ignore = "75 seconds of play, 20 seconds unoptimised: \
            cargo test --release -p kuula-cli --test games -- --ignored"]
fn the_whole_walk_through_kilnhollow_ends_at_the_lever_inside_the_budget() {
    let walk = kilnhollow_walk();
    let mut console = LuaGuest::console(game("kilnhollow"));
    for (frame, &bits) in walk.iter().enumerate() {
        console.step(FrameInput::new(bits));
        if let Some(fault) = console.state().fault() {
            panic!("frame {frame}: {fault:?}");
        }
    }
    let dump = console
        .state_dump(&["state".to_string(), "perf".to_string()])
        .unwrap();
    assert!(dump.contains("won"), "the walk ended elsewhere: {dump}");
    let peak = dumped(&dump, "peak");
    assert!(peak <= 0.75, "the worst frame used {peak} of the budget");
}
