use super::*;
use crate::net::{MemoryTransport, MEMORY_TICKET};
use crate::SnapshotLimits;

fn snapshot(code: &[u8]) -> Snapshot {
    Snapshot::from_entries([("main.lua", code.to_vec())], SnapshotLimits::default()).unwrap()
}

#[test]
fn identity_is_order_independent_but_covers_names_content_and_metadata() {
    let a = Snapshot::from_entries(
        [("main.lua", b"ab".to_vec()), ("cart.toml", b"c".to_vec())],
        SnapshotLimits::default(),
    )
    .unwrap();
    let b = Snapshot::from_entries(
        [("cart.toml", b"c".to_vec()), ("main.lua", b"ab".to_vec())],
        SnapshotLimits::default(),
    )
    .unwrap();
    assert_eq!(Identity::new(&a), Identity::new(&b));
    assert_ne!(Identity::new(&a), Identity::new(&snapshot(b"ab")));
    assert_ne!(
        Identity::new(&snapshot(b"ab")),
        Identity::new(&snapshot(b"ac"))
    );
    assert_eq!(Identity::new(&a).hex().len(), 64);
}

fn pair(b: Identity) -> [VerifiedTransport; 2] {
    let (a, other) = MemoryTransport::pair();
    let mut pair = [
        VerifiedTransport::new(Box::new(a), Identity::new(&snapshot(b"a"))),
        VerifiedTransport::new(Box::new(other), b),
    ];
    pair[0].push(Command::Host);
    pair[1].push(Command::Join {
        ticket: MEMORY_TICKET.into(),
    });
    pair
}

#[test]
fn matching_pair_hides_greetings_and_then_delivers_gameplay() {
    let mut pair = pair(Identity::new(&snapshot(b"a")));
    let mut seen = [Vec::new(), Vec::new()];
    for _ in 0..4 {
        for i in 0..2 {
            pair[i].poll(&mut seen[i], 64);
        }
    }
    for events in &seen {
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::Connected { .. }))
                .count(),
            1
        );
        assert!(!events.iter().any(|e| matches!(e, Event::Message { .. })));
    }
    pair[0].push(Command::Send {
        data: b"move".to_vec(),
    });
    let mut out = Vec::new();
    pair[1].poll(&mut out, 64);
    assert_eq!(
        out,
        [Event::Message {
            from: 1,
            data: b"move".to_vec()
        }]
    );
}

#[test]
fn content_and_profile_mismatches_never_admit_connected() {
    for identity in [
        Identity::new(&snapshot(b"different")),
        Identity {
            profile: "incompatible".into(),
            ..Identity::new(&snapshot(b"a"))
        },
    ] {
        let mut pair = pair(identity);
        let mut seen = Vec::new();
        for _ in 0..5 {
            for side in &mut pair {
                side.poll(&mut seen, 64);
            }
        }
        assert!(!seen.iter().any(|e| matches!(e, Event::Connected { .. })));
        assert!(seen.iter().any(|e| matches!(
            e,
            Event::Failed {
                code: FailCode::Handshake,
                ..
            }
        )));
    }
}

#[test]
fn missing_or_malformed_greetings_fail_boundedly() {
    for malformed in [false, true] {
        let (a, mut b) = MemoryTransport::pair();
        let mut a = VerifiedTransport::new(Box::new(a), Identity::new(&snapshot(b"a")));
        a.push(Command::Host);
        b.push(Command::Join {
            ticket: MEMORY_TICKET.into(),
        });
        if malformed {
            b.push(Command::Send {
                data: b"not a greeting".to_vec(),
            });
        }
        let mut events = Vec::new();
        for _ in 0..=HANDSHAKE_POLLS {
            a.poll(&mut events, 64);
        }
        assert!(!events.iter().any(|e| matches!(e, Event::Connected { .. })));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(
                    e,
                    Event::Failed {
                        code: FailCode::Handshake,
                        ..
                    }
                ))
                .count(),
            1
        );
    }
}

#[test]
fn refused_host_clears_its_ticket_and_can_start_a_compatible_session() {
    use crate::net::{NetEnv, NetState, Status};
    let mut pair = pair(Identity::new(&snapshot(b"wrong")));
    let mut state = NetState::new(&NetEnv {
        permitted: true,
        invite: None,
    });
    state.host();
    for _ in 0..5 {
        let mut events = Vec::new();
        pair[0].poll(&mut events, 64);
        state.begin_frame(events);
        pair[1].poll(&mut Vec::new(), 64);
    }
    assert_eq!(state.status, Status::Ended);
    assert!(state.ticket.is_none());
    state.host();
    assert_eq!(state.status, Status::Hosting);
    pair[0].push(Command::Host);
    pair[1].identity = Identity::new(&snapshot(b"a"));
    pair[1].push(Command::Join {
        ticket: MEMORY_TICKET.into(),
    });
    for _ in 0..5 {
        let mut events = Vec::new();
        pair[0].poll(&mut events, 64);
        state.begin_frame(events);
        pair[1].poll(&mut Vec::new(), 64);
    }
    assert_eq!(state.status, Status::Connected);
}

#[test]
fn a_wire_handshake_failure_closes_the_rearmed_listener() {
    use std::cell::Cell;
    use std::rc::Rc;
    struct RearmedListener {
        closed: Rc<Cell<bool>>,
        events: Vec<Event>,
    }
    impl Transport for RearmedListener {
        fn push(&mut self, command: Command) {
            if matches!(command, Command::Leave) {
                self.closed.set(true);
            }
        }
        fn poll(&mut self, out: &mut Vec<Event>, max: usize) {
            let count = max.min(self.events.len());
            out.extend(self.events.drain(..count));
        }
    }
    let closed = Rc::new(Cell::new(false));
    let listener = RearmedListener {
        closed: closed.clone(),
        events: vec![
            Event::Hosting {
                ticket: "old".into(),
            },
            Event::failed(FailCode::Handshake, "bad wire greeting"),
        ],
    };
    let mut transport = VerifiedTransport::new(Box::new(listener), Identity::new(&snapshot(b"a")));
    let mut events = Vec::new();
    transport.poll(&mut events, 64);
    assert!(
        closed.get(),
        "terminal cart status must agree with listener teardown"
    );
    assert!(matches!(
        events.last(),
        Some(Event::Failed {
            code: FailCode::Handshake,
            ..
        })
    ));
}
