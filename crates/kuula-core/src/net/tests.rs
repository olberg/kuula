use super::*;

fn drain(t: &mut dyn Transport) -> Vec<Event> {
    let mut out = Vec::new();
    t.poll(&mut out, MAX_BATCH);
    out
}

fn msg(s: &str) -> Event {
    Event::Message {
        from: 1,
        data: s.as_bytes().to_vec(),
    }
}

#[test]
fn the_memory_pair_hosts_joins_sends_and_leaves() {
    let (mut a, mut b) = MemoryTransport::pair();
    a.push(Command::Host);
    assert_eq!(
        drain(&mut a),
        [Event::Hosting {
            ticket: MEMORY_TICKET.into()
        }]
    );
    b.push(Command::Join {
        ticket: MEMORY_TICKET.into(),
    });
    assert_eq!(drain(&mut a), [Event::Connected { peer: 1 }]);
    assert_eq!(drain(&mut b), [Event::Connected { peer: 1 }]);
    for i in 0..3 {
        a.push(Command::Send {
            data: format!("a{i}").into_bytes(),
        });
        b.push(Command::Send {
            data: format!("b{i}").into_bytes(),
        });
    }
    assert_eq!(drain(&mut b), [msg("a0"), msg("a1"), msg("a2")]);
    assert_eq!(drain(&mut a), [msg("b0"), msg("b1"), msg("b2")]);
    b.push(Command::Leave);
    assert_eq!(drain(&mut b), []);
    assert_eq!(
        drain(&mut a),
        [Event::Disconnected {
            reason: Reason::Left
        }]
    );
    // The host still listens: a second join works against the same
    // ticket.
    b.push(Command::Join {
        ticket: MEMORY_TICKET.into(),
    });
    assert_eq!(drain(&mut a), [Event::Connected { peer: 1 }]);
    drop(b);
    assert_eq!(
        drain(&mut a),
        [Event::Disconnected {
            reason: Reason::Lost
        }]
    );
}

#[test]
fn a_host_that_leaves_puts_its_joiner_back_to_off() {
    let (mut a, mut b) = MemoryTransport::pair();
    a.push(Command::Host);
    b.push(Command::Join {
        ticket: MEMORY_TICKET.into(),
    });
    drain(&mut a);
    drain(&mut b);
    a.push(Command::Leave);
    assert_eq!(
        drain(&mut b),
        [Event::Disconnected {
            reason: Reason::Left
        }]
    );
    // The joiner never hosted, so it is not listening now: it may
    // host, and nobody can join it before it does.
    a.push(Command::Join {
        ticket: MEMORY_TICKET.into(),
    });
    assert!(matches!(
        drain(&mut a)[..],
        [Event::Failed {
            code: FailCode::Connect,
            ..
        }]
    ));
    b.push(Command::Host);
    assert_eq!(
        drain(&mut b),
        [Event::Hosting {
            ticket: MEMORY_TICKET.into()
        }]
    );
    // The same when the host is dropped rather than leaving.
    let (mut a, mut b) = MemoryTransport::pair();
    a.push(Command::Host);
    b.push(Command::Join {
        ticket: MEMORY_TICKET.into(),
    });
    drain(&mut a);
    drain(&mut b);
    drop(a);
    drain(&mut b);
    b.push(Command::Host);
    assert!(matches!(drain(&mut b)[..], [Event::Hosting { .. }]));
}

#[test]
fn memory_failures_delay_and_capacity() {
    let (mut a, mut b) = MemoryTransport::pair();
    b.push(Command::Join {
        ticket: MEMORY_TICKET.into(),
    });
    assert!(matches!(
        drain(&mut b)[0],
        Event::Failed {
            code: FailCode::Connect,
            ..
        }
    ));
    b.push(Command::Join {
        ticket: "bogus".into(),
    });
    assert!(matches!(
        drain(&mut b)[0],
        Event::Failed {
            code: FailCode::Ticket,
            ..
        }
    ));
    a.push(Command::Send { data: vec![1] });
    assert!(matches!(
        drain(&mut a)[0],
        Event::Failed {
            code: FailCode::Declined,
            ..
        }
    ));
    a.push(Command::Host);
    a.push(Command::Host);
    let events = drain(&mut a);
    assert!(matches!(events[0], Event::Hosting { .. }));
    assert!(matches!(
        events[1],
        Event::Failed {
            code: FailCode::Declined,
            ..
        }
    ));
    b.push(Command::Join {
        ticket: MEMORY_TICKET.into(),
    });
    drain(&mut a);
    drain(&mut b);
    // A delay of two polls holds a message back twice.
    a.set_delay(2);
    a.push(Command::Send { data: vec![7] });
    assert_eq!(drain(&mut b), []);
    assert_eq!(drain(&mut b), []);
    assert_eq!(drain(&mut b).len(), 1);
    a.set_delay(0);
    a.set_capacity(2);
    for _ in 0..3 {
        a.push(Command::Send { data: vec![7] });
    }
    assert!(matches!(
        drain(&mut a)[0],
        Event::Failed {
            code: FailCode::QueueFull,
            ..
        }
    ));
    let mut out = Vec::new();
    b.poll(&mut out, 1);
    assert_eq!(out.len(), 1, "poll honours max");
    b.poll(&mut out, 10);
    assert_eq!(out.len(), 2);
}

#[test]
fn state_applies_events_and_bounds_the_queues() {
    let env = NetEnv {
        permitted: true,
        invite: None,
    };
    let mut s = NetState::new(&env);
    assert_eq!(s.room(), MAX_BATCH);
    s.host();
    assert_eq!(s.status, Status::Hosting);
    assert_eq!(s.outbox, [Command::Host]);
    s.host();
    assert!(matches!(
        s.recv(),
        Some(Event::Failed {
            code: FailCode::Declined,
            ..
        })
    ));
    s.begin_frame(vec![Event::Hosting { ticket: "t".into() }]);
    assert_eq!(s.ticket.as_deref(), Some("t"));
    assert!(s.outbox.is_empty());
    assert_eq!(s.admitted.len(), 1);
    assert!(!s.send(vec![1]), "no session yet");
    s.begin_frame(vec![Event::Connected { peer: 1 }]);
    assert_eq!(s.status, Status::Connected);
    for _ in 0..MAX_SENDS {
        assert!(s.send(vec![1]));
    }
    assert!(!s.send(vec![1]));
    assert_eq!(s.dropped, 1);
    assert_eq!(s.sent, MAX_SENDS as u64);
    assert_eq!(s.outbox.len(), MAX_SENDS);
    // Peer leaves: a host goes back to hosting.
    s.begin_frame(vec![Event::Disconnected {
        reason: Reason::Left,
    }]);
    assert_eq!(s.status, Status::Hosting);
    assert_eq!(s.peer, None);
    s.leave();
    assert_eq!(s.status, Status::Ended);
    assert_eq!(s.outbox, [Command::Leave]);
    s.begin_frame(vec![Event::Disconnected {
        reason: Reason::Closed,
    }]);
    assert_eq!(s.status, Status::Ended);
    // A full inbox drops and counts; a permission withdrawal clears.
    let flood: Vec<Event> = (0..INBOX_CAP + 5).map(|_| msg("x")).collect();
    s.begin_frame(flood);
    assert_eq!(s.room(), 0);
    // One send refused above, then four events already queued plus
    // the five past the cap.
    assert_eq!(s.dropped, 1 + 4 + 5);
    assert_eq!(s.inbox.len(), INBOX_CAP);
    s.begin_frame(vec![Event::Permission { granted: false }]);
    assert_eq!(s.inbox.len(), 1);
    assert!(!s.permitted);
    s.host();
    assert!(matches!(
        s.inbox.back(),
        Some(Event::Failed {
            code: FailCode::Denied,
            ..
        })
    ));
    assert_eq!(s.status, Status::Ended);
}

#[test]
fn a_failed_attempt_ends_and_a_failure_while_connected_does_not() {
    let mut s = NetState::new(&NetEnv {
        permitted: true,
        invite: None,
    });
    s.join("x".into());
    assert_eq!(s.status, Status::Joining);
    s.begin_frame(vec![Event::failed(FailCode::Connect, "no")]);
    assert_eq!(s.status, Status::Ended);
    s.join("x".into());
    s.begin_frame(vec![Event::Connected { peer: 1 }]);
    s.begin_frame(vec![Event::failed(FailCode::QueueFull, "full")]);
    assert_eq!(s.status, Status::Connected);
    let long = "d".repeat(MAX_DETAIL + 10);
    let Event::Failed { detail, .. } = Event::failed(FailCode::Frame, long) else {
        panic!()
    };
    assert_eq!(detail.len(), MAX_DETAIL);
}

#[test]
fn the_link_gates_builds_lazily_and_revokes() {
    let (a, mut b) = MemoryTransport::pair();
    let mut link = Link::over(Box::new(a));
    assert_eq!(link.constructions(), 0);
    link.push(vec![Command::Host]);
    assert_eq!(link.constructions(), 1);
    let mut out = Vec::new();
    link.poll(&mut out, 64);
    assert!(matches!(out[0], Event::Hosting { .. }));
    b.push(Command::Join {
        ticket: MEMORY_TICKET.into(),
    });
    out.clear();
    link.poll(&mut out, 0);
    assert!(out.is_empty(), "no room, nothing offered");
    link.poll(&mut out, 1);
    assert_eq!(out, [Event::Connected { peer: 1 }]);
    // Withdrawn permission: the peer sees `Left`, the cart a
    // permission event and nothing else that was waiting.
    link.set_permitted(false);
    assert!(!link.is_live());
    out.clear();
    link.poll(&mut out, 0);
    assert_eq!(out, [Event::Permission { granted: false }]);
    assert_eq!(
        drain(&mut b),
        [
            Event::Connected { peer: 1 },
            Event::Disconnected {
                reason: Reason::Left
            }
        ]
    );
    // Denied: commands go nowhere and no transport is built.
    link.push(vec![Command::Host]);
    assert_eq!(link.constructions(), 1);
    assert!(!link.is_live());
    let mut offline = Link::offline();
    offline.set_permitted(true);
    offline.push(vec![Command::Host]);
    out.clear();
    offline.poll(&mut out, 64);
    assert!(matches!(out[0], Event::Permission { granted: true }));
    assert!(matches!(
        out[1],
        Event::Failed {
            code: FailCode::Disabled,
            ..
        }
    ));
}

#[test]
fn a_cart_that_hosts_and_leaves_every_frame_cannot_grow_the_queue() {
    let mut link = Link::new(Box::new(|| {
        let (a, _b) = MemoryTransport::pair();
        Box::new(a)
    }));
    link.set_permitted(true);
    // Never polled: the cart's inbox is full, so the host offers no
    // room, while the cart keeps closing sessions.
    for _ in 0..PENDING_CAP * 2 {
        link.push(vec![Command::Host, Command::Leave]);
    }
    assert_eq!(link.pending.len(), PENDING_CAP);
    // A control event still gets through, and a revoke clears.
    link.set_permitted(false);
    assert_eq!(link.pending.len(), 1);
    assert!(matches!(
        link.pending[0],
        Event::Permission { granted: false }
    ));
}

#[test]
fn a_link_builds_at_most_one_transport_a_frame() {
    let mut link = Link::new(Box::new(|| {
        let (a, _b) = MemoryTransport::pair();
        Box::new(a)
    }));
    link.set_permitted(true);
    link.push(vec![
        Command::Host,
        Command::Leave,
        Command::Host,
        Command::Leave,
        Command::Host,
    ]);
    assert_eq!(link.constructions(), 1);
    // The refused hosts were told so, and nothing is left open.
    assert!(!link.is_live());
    assert!(link.pending.iter().any(|e| matches!(
        e,
        Event::Failed {
            code: FailCode::Declined,
            ..
        }
    )));
    // The next frame may build again.
    link.push(vec![Command::Host]);
    assert_eq!(link.constructions(), 2);
    assert!(link.is_live());
}

#[test]
fn a_leave_closes_and_reports_closed() {
    let (a, mut b) = MemoryTransport::pair();
    let mut link = Link::over(Box::new(a));
    link.push(vec![Command::Host]);
    b.push(Command::Join {
        ticket: MEMORY_TICKET.into(),
    });
    let mut out = Vec::new();
    link.poll(&mut out, 64);
    link.push(vec![Command::Leave]);
    out.clear();
    link.poll(&mut out, 64);
    assert_eq!(
        out,
        [Event::Disconnected {
            reason: Reason::Closed
        }]
    );
    assert!(!link.is_live());
    assert!(drain(&mut b).contains(&Event::Disconnected {
        reason: Reason::Left
    }));
    assert!(Command::Send { data: vec![] }.check().is_err());
    assert!(Command::Send {
        data: vec![0; MAX_DATA + 1]
    }
    .check()
    .is_err());
    assert!(Command::Join {
        ticket: "t".repeat(MAX_TICKET + 1)
    }
    .check()
    .is_err());
    assert!(Event::Connected { peer: 0 }.check().is_err());
    assert_eq!(FailCode::parse("net_busy"), Some(FailCode::Busy));
    assert_eq!(FailCode::parse("nope"), None);
    for c in FailCode::ALL {
        assert_eq!(FailCode::parse(c.as_str()), Some(c));
    }
}
