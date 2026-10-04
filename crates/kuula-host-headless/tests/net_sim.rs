use kuula_core::{Console, FrameInput, ReplayGuest, Snapshot, SnapshotLimits};
use kuula_host_headless::net_sim::{simulate, Config, Guest, Run, Scenario, Stall, MAX_DELAY};
use kuula_lua::LuaGuest;
use std::path::PathBuf;
use std::rc::Rc;

fn cart() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/marbles")
}
fn snapshot() -> Snapshot {
    Snapshot::from_dir(&cart(), SnapshotLimits::default()).unwrap()
}
fn guest() -> Guest {
    Guest::new(LuaGuest::factory, kuula_lua::RANDOM_SEED)
}
fn scenario(name: &str) -> Scenario {
    serde_json::from_str(
        &std::fs::read_to_string(cart().join(format!("scenarios/{name}.json"))).unwrap(),
    )
    .unwrap()
}
fn play(name: &str) -> Run {
    let scenario = scenario(name);
    let inputs = scenario.inputs().unwrap();
    simulate(&guest(), [snapshot(), snapshot()], scenario.config, inputs).unwrap()
}

fn replay(run: &Run) {
    for i in 0..2 {
        let text = run.transcripts[i].encode().unwrap();
        let transcript = kuula_core::Transcript::decode(&text).unwrap();
        transcript.check_cart(&snapshot()).unwrap();
        let env = transcript.net.as_ref().unwrap().env.clone();
        let mut console = Console::new(
            Rc::new(snapshot()),
            ReplayGuest::factory(LuaGuest::factory, transcript.net),
        );
        console.set_net_env(env);
        let result = kuula_host_headless::run(
            &mut console,
            transcript.inputs.len() as u64,
            &transcript.inputs,
            &mut |_, _: &kuula_host_headless::OwnedFrame| {},
        );
        assert_eq!(result.fault, None);
        assert_eq!(result.per_frame, run.report.peers[i].per_frame);
        assert_eq!(
            console.state_dump(&run.report.config.state).unwrap(),
            run.report.peers[i].state
        );
    }
}

#[test]
fn both_players_reach_a_complete_round_and_replay_without_a_transport() {
    for name in ["win", "delayed"] {
        let run = play(name);
        for peer in &run.report.peers {
            assert!(peer.fault.is_none(), "{:?}", peer.fault);
            assert!(peer.state.contains("winner = 1"), "{}", peer.state);
            assert!(peer.state.contains("remaining = 0"), "{}", peer.state);
            assert!(peer.state.contains("turn = 5"), "{}", peer.state);
        }
        replay(&run);
    }
}

#[test]
fn stalls_are_reproducible_and_a_disconnect_ends_both_rounds() {
    let first = play("stalled");
    let second = play("stalled");
    assert_eq!(
        serde_json::to_string(&first.report).unwrap(),
        serde_json::to_string(&second.report).unwrap()
    );
    assert!(!first.report.peers[0].state.contains("remaining = 0"));
    replay(&first);
    let disconnected = play("disconnect");
    for peer in &disconnected.report.peers {
        assert!(peer.state.contains("status = \"ended\""), "{}", peer.state);
    }
    replay(&disconnected);
}

#[test]
fn a_joiner_can_start_a_second_fresh_round_under_the_hosts_ticket() {
    let run = play("rematch");
    for peer in &run.report.peers {
        assert!(peer.state.contains("winner = 1"), "{}", peer.state);
        assert!(peer.state.contains("turn = 5"), "{}", peer.state);
    }
    assert_eq!(
        run.report.peers[0]
            .log
            .iter()
            .filter(|l| l.text.starts_with("winner "))
            .count(),
        2
    );
    replay(&run);
}

#[test]
fn invalid_schedules_and_mismatched_carts_are_refused() {
    for config in [
        Config {
            frames: 0,
            ..Config::default()
        },
        Config {
            capacity: 257,
            ..Config::default()
        },
        Config {
            stalls: vec![Stall {
                peer: 2,
                start: 1,
                end: 3,
            }],
            ..Config::default()
        },
        // A delay or a contiguous stall past the greeting timeout's half
        // would refuse identical carts as unresponsive.
        Config {
            delay: MAX_DELAY + 1,
            ..Config::default()
        },
        Config {
            frames: 1000,
            stalls: vec![
                Stall {
                    peer: 1,
                    start: 200,
                    end: 400,
                },
                Stall {
                    peer: 1,
                    start: 1,
                    end: 200,
                },
            ],
            ..Config::default()
        },
    ] {
        assert!(simulate(
            &guest(),
            [snapshot(), snapshot()],
            config,
            [Vec::new(), Vec::new()]
        )
        .is_err());
    }
    // The bounds themselves still connect identical carts.
    let run = simulate(
        &guest(),
        [snapshot(), snapshot()],
        Config {
            frames: 1000,
            delay: MAX_DELAY,
            stalls: vec![Stall {
                peer: 1,
                start: 2,
                end: 2 + MAX_DELAY,
            }],
            ..Config::default()
        },
        [Vec::new(), Vec::new()],
    )
    .unwrap();
    assert!(
        run.report.peers.iter().all(|p| p.net_status == "connected"),
        "{:?}",
        run.report
            .peers
            .iter()
            .map(|p| &p.net_status)
            .collect::<Vec<_>>()
    );
    let original = snapshot();
    let changed = Snapshot::from_entries(
        original.entries().map(|(name, bytes)| {
            let bytes = if name == "cart.toml" {
                [bytes, b"\n# different content\n"].concat()
            } else {
                bytes.to_vec()
            };
            (name, bytes)
        }),
        SnapshotLimits::default(),
    )
    .unwrap();
    let run = simulate(
        &guest(),
        [original, changed],
        Config {
            frames: 30,
            ..Config::default()
        },
        [Vec::new(), Vec::new()],
    )
    .unwrap();
    assert!(run.report.peers.iter().all(|p| p.net_status != "connected"));
    assert!(run
        .report
        .peers
        .iter()
        .any(|p| p.state.contains("digest differs")));
}

#[test]
fn a_cart_that_reconnects_after_the_disconnect_is_refused_not_stranded() {
    let source = br#"
game = { refused = 0 }
local started = false
local function connect()
  if net.invite() then net.join(net.invite()) else net.host() end
end
function _update()
  if not started then started = true; connect() end
  while true do local e = net.recv(); if not e then break end
    if e.kind == 'disconnected' then connect() end
    if e.kind == 'failed' then game.refused = game.refused + 1 end
  end
end
"#;
    let make = || {
        Snapshot::from_entries(
            [
                ("main.lua", source.to_vec()),
                (
                    "cart.toml",
                    b"[cart]\nservices=['net']\nscreen_mode='320x240'".to_vec(),
                ),
            ],
            SnapshotLimits::default(),
        )
        .unwrap()
    };
    let run = simulate(
        &guest(),
        [make(), make()],
        Config {
            frames: 60,
            disconnect: Some(30),
            ..Config::default()
        },
        [Vec::new(), Vec::new()],
    )
    .unwrap();
    // The host keeps listening when its peer is lost, so its second
    // `host` is declined; the joiner's second `join` meets the severed
    // link. Neither waits for an answer that never comes.
    for (peer, status) in run.report.peers.iter().zip(["hosting", "ended"]) {
        assert!(peer.fault.is_none());
        assert_eq!(peer.net_status, status, "{}", peer.state);
        assert!(peer.state.contains("refused = 1"), "{}", peer.state);
    }
}

#[test]
fn stalled_small_queues_report_pressure_without_unbounded_logs() {
    let source = br#"
game = { errors = 0 }
local started = false
function _update()
  if not started then started = true; if net.invite() then net.join(net.invite()) else net.host() end end
  while true do local e = net.recv(); if not e then break end
    if e.kind == 'failed' then game.errors = game.errors + 1 end
  end
  if net.status() == 'connected' then for i=1,16 do net.send('x') end end
end
"#;
    let make = || {
        Snapshot::from_entries(
            [
                ("main.lua", source.to_vec()),
                (
                    "cart.toml",
                    b"[cart]\nservices=['net']\nscreen_mode='320x240'".to_vec(),
                ),
            ],
            SnapshotLimits::default(),
        )
        .unwrap()
    };
    let config = Config {
        frames: 100,
        capacity: 1,
        stalls: vec![Stall {
            peer: 1,
            start: 20,
            end: 80,
        }],
        ..Config::default()
    };
    let run = simulate(
        &guest(),
        [make(), make()],
        config,
        [vec![FrameInput::NONE; 100], Vec::new()],
    )
    .unwrap();
    assert!(run.report.peers.iter().all(|p| p.fault.is_none()));
    assert!(
        !run.report.peers[0].state.contains("errors = 0"),
        "{}",
        run.report.peers[0].state
    );
}
