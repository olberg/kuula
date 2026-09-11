//! The wire in practice: `Data` frames, the non-blocking calls, the
//! re-armed listener, and the Iroh transport driven by the same script
//! as the memory one.

use std::time::{Duration, Instant};

use kuula_core::net::{Command, Event, FailCode, MemoryTransport, Reason, Transport, MAX_BATCH};

use crate::proto::{self, Frame};
use crate::tests::{loopback, net, pair, Raw, WAIT};
use crate::transport::{live_drivers, IrohTransport};
use crate::{Code, Event as WireEvent};

#[test]
fn data_frames_go_both_ways_and_binary_survives() {
    let (_a, mut l, _b, mut j) = pair();
    l.send_data(&[0, 255, 254, 1]).unwrap();
    j.send_data(&[7; proto::MAX_DATA]).unwrap();
    assert_eq!(j.recv(WAIT), Some(WireEvent::Data(vec![0, 255, 254, 1])));
    assert_eq!(
        l.recv(WAIT),
        Some(WireEvent::Data(vec![7; proto::MAX_DATA]))
    );
    assert_eq!(l.send_data(&[]).unwrap_err().code, Code::Frame);
    assert_eq!(
        l.send_data(&[0; proto::MAX_DATA + 1]).unwrap_err().code,
        Code::Frame
    );
    // Text and data interleave in order.
    l.send_text("t1").unwrap();
    l.send_data(b"d1").unwrap();
    l.send_text("t2").unwrap();
    assert_eq!(j.recv(WAIT), Some(WireEvent::Text("t1".into())));
    assert_eq!(j.recv(WAIT), Some(WireEvent::Data(b"d1".to_vec())));
    assert_eq!(j.recv(WAIT), Some(WireEvent::Text("t2".into())));
}

#[test]
fn an_oversized_data_header_is_refused_before_allocation() {
    let (_a, mut s, raw, _c, mut send, _r) = crate::tests::raw_session();
    let mut header = [proto::TYPE_DATA, 0, 0, 0, 0];
    header[1..].copy_from_slice(&(64u32 * 1024 * 1024).to_be_bytes());
    raw.write(&mut send, &header);
    match s.recv(WAIT) {
        Some(WireEvent::Error(e)) => {
            assert_eq!(e.code, Code::Frame);
            assert!(e.detail.contains("67108864"), "{e}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn try_send_data_and_try_recv_never_wait() {
    let (_a, mut l, _b, mut j) = pair();
    assert_eq!(l.try_recv(), Ok(None));
    let t = Instant::now();
    for i in 0..proto::OUTGOING_QUEUE * 4 {
        match l.try_send_data(&[i as u8]) {
            Ok(()) => {}
            Err(e) => {
                assert_eq!(e.code, Code::QueueFull, "{e}");
                break;
            }
        }
    }
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
    // The session is intact after a refused send.
    l.send_text("still here").unwrap();
    let mut heard = false;
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        match j.try_recv() {
            Ok(Some(WireEvent::Text(t))) if t == "still here" => {
                heard = true;
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("session ended: {e}"),
        }
    }
    assert!(heard);
    l.close().unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        match j.try_recv() {
            Ok(Some(WireEvent::PeerClosed)) | Err(_) => break,
            _ if Instant::now() > deadline => panic!("no close seen"),
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

#[test]
fn join_start_to_an_absent_peer_fails_within_the_deadline_without_blocking() {
    let ticket = {
        let a = net();
        a.listen().unwrap().ticket().to_string()
    };
    let b = net();
    let t = Instant::now();
    let mut joining = b.join_start(&ticket).unwrap();
    assert!(
        t.elapsed() < Duration::from_millis(200),
        "{:?}",
        t.elapsed()
    );
    let mut slowest = Duration::ZERO;
    let outcome = loop {
        let p = Instant::now();
        let out = joining.poll();
        slowest = slowest.max(p.elapsed());
        if let Some(o) = out {
            break o;
        }
        assert!(
            t.elapsed() < proto::CONNECT_DEADLINE + Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(outcome.unwrap_err().code, Code::Connect);
    assert!(slowest < Duration::from_millis(50), "{slowest:?}");
    // Dropping a join in progress abandons it quietly.
    let joining = b.join_start(&ticket).unwrap();
    drop(joining);
    assert!(b.join_start("nonsense").is_err());
}

#[test]
fn a_rearmed_listener_admits_a_second_joiner_under_the_same_ticket() {
    let a = net();
    let mut listener = a.listen().unwrap();
    let ticket = listener.ticket().to_string();
    let b = net();
    let first = b.join(&ticket).unwrap();
    let mut l = listener.accept(WAIT).unwrap().expect("a session");
    assert_eq!(listener.rearm().unwrap_err().code, Code::Busy);
    drop(first);
    assert!(matches!(
        l.recv(WAIT),
        Some(WireEvent::PeerClosed) | Some(WireEvent::PeerLost(_))
    ));
    drop(l);
    listener.rearm().unwrap();
    let c = net();
    let second = c.join(&ticket).unwrap();
    let mut l = listener.accept(WAIT).unwrap().expect("a second session");
    assert_eq!(l.peer_id(), c.id());
    let _ = second;
    let _ = l.try_recv();
}

/// Poll `t` until `pred` accepts an event or the wait is up; every
/// event seen is appended to `seen`.
fn wait_for(t: &mut dyn Transport, seen: &mut Vec<Event>, pred: impl Fn(&Event) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut out = Vec::new();
        let p = Instant::now();
        t.poll(&mut out, MAX_BATCH);
        assert!(p.elapsed() < Duration::from_millis(50), "poll waited");
        let hit = out.iter().any(&pred);
        seen.extend(out);
        if hit {
            return;
        }
        assert!(Instant::now() < deadline, "timed out; seen {seen:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The shape of an event: kind and payload, minus the ticket text.
fn shape(e: &Event) -> String {
    match e {
        Event::Hosting { .. } => "hosting".into(),
        Event::Message { data, .. } => format!("message:{}", String::from_utf8_lossy(data)),
        Event::Disconnected { reason } => format!("disconnected:{}", reason.as_str()),
        Event::Failed { code, .. } => format!("failed:{}", code.as_str()),
        other => other.kind().to_string(),
    }
}

/// Host, join, three sends each way, the joiner leaves, a second join,
/// the host leaves: the same script over either transport yields the
/// same event shapes on both sides.
fn script(a: &mut dyn Transport, b: &mut dyn Transport) -> (Vec<String>, Vec<String>) {
    let (mut sa, mut sb) = (Vec::new(), Vec::new());
    a.push(Command::Host);
    wait_for(a, &mut sa, |e| matches!(e, Event::Hosting { .. }));
    let Some(Event::Hosting { ticket }) = sa.last() else {
        panic!()
    };
    let ticket = ticket.clone();
    b.push(Command::Join {
        ticket: ticket.clone(),
    });
    wait_for(b, &mut sb, |e| matches!(e, Event::Connected { .. }));
    wait_for(a, &mut sa, |e| matches!(e, Event::Connected { .. }));
    for i in 0..3 {
        a.push(Command::Send {
            data: format!("a{i}").into_bytes(),
        });
        b.push(Command::Send {
            data: format!("b{i}").into_bytes(),
        });
    }
    wait_for(
        b,
        &mut sb,
        |e| matches!(e, Event::Message { data, .. } if data == b"a2"),
    );
    wait_for(
        a,
        &mut sa,
        |e| matches!(e, Event::Message { data, .. } if data == b"b2"),
    );
    b.push(Command::Leave);
    wait_for(a, &mut sa, |e| matches!(e, Event::Disconnected { .. }));
    // The host is still listening under the same ticket.
    b.push(Command::Join { ticket });
    wait_for(b, &mut sb, |e| matches!(e, Event::Connected { .. }));
    wait_for(a, &mut sa, |e| matches!(e, Event::Connected { .. }));
    a.push(Command::Leave);
    wait_for(b, &mut sb, |e| matches!(e, Event::Disconnected { .. }));
    (
        sa.iter().map(shape).collect(),
        sb.iter().map(shape).collect(),
    )
}

#[test]
fn the_iroh_transport_follows_the_memory_transport() {
    let (mut ma, mut mb) = MemoryTransport::pair();
    let memory = script(&mut ma, &mut mb);
    let before = live_drivers();
    let mut ia = IrohTransport::new(Some("127.0.0.1:0".parse().unwrap()));
    let mut ib = IrohTransport::new(Some("127.0.0.1:0".parse().unwrap()));
    let iroh = script(&mut ia, &mut ib);
    assert_eq!(iroh, memory);
    assert_eq!(
        memory.0,
        [
            "hosting",
            "connected",
            "message:b0",
            "message:b1",
            "message:b2",
            "disconnected:left",
            "connected",
        ]
    );
    assert_eq!(
        memory.1,
        [
            "connected",
            "message:a0",
            "message:a1",
            "message:a2",
            "connected",
            "disconnected:left",
        ]
    );
    // The second join needed a fresh joiner endpoint: the first was
    // dropped with its leave.
    drop(ia);
    drop(ib);
    let deadline = Instant::now() + Duration::from_secs(15);
    while live_drivers() > before {
        assert!(Instant::now() < deadline, "drivers did not stop");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn iroh_failures_are_events_and_a_joiner_that_loses_its_host_is_told() {
    let mut a = IrohTransport::new(Some("127.0.0.1:0".parse().unwrap()));
    let mut seen = Vec::new();
    a.push(Command::Join {
        ticket: "nonsense".into(),
    });
    wait_for(&mut a, &mut seen, |e| {
        matches!(
            e,
            Event::Failed {
                code: FailCode::Ticket,
                ..
            }
        )
    });
    a.push(Command::Send { data: vec![1] });
    wait_for(&mut a, &mut seen, |e| {
        matches!(
            e,
            Event::Failed {
                code: FailCode::Declined,
                ..
            }
        )
    });
    // A host that vanishes: its joiner sees the loss.
    let mut host = IrohTransport::new(Some("127.0.0.1:0".parse().unwrap()));
    let mut hs = Vec::new();
    host.push(Command::Host);
    wait_for(&mut host, &mut hs, |e| matches!(e, Event::Hosting { .. }));
    let Some(Event::Hosting { ticket }) = hs.last() else {
        panic!()
    };
    a.push(Command::Join {
        ticket: ticket.clone(),
    });
    wait_for(&mut a, &mut seen, |e| matches!(e, Event::Connected { .. }));
    drop(host);
    wait_for(&mut a, &mut seen, |e| {
        matches!(
            e,
            Event::Disconnected {
                reason: Reason::Left | Reason::Lost
            }
        )
    });
}

/// A raw peer that floods `Data` frames without ever reading: the
/// transport hands out at most `max` per poll, the backlog stays
/// bounded, and polling stays quick throughout.
#[test]
fn a_flooding_peer_is_bounded_by_the_backlog_and_flow_control() {
    let mut host = IrohTransport::new(Some("127.0.0.1:0".parse().unwrap()));
    let mut seen = Vec::new();
    host.push(Command::Host);
    wait_for(&mut host, &mut seen, |e| matches!(e, Event::Hosting { .. }));
    let Some(Event::Hosting { ticket }) = seen.last() else {
        panic!()
    };
    let raw = Raw::new();
    let (_conn, mut send, _recv) = raw.connect(ticket);
    raw.write(&mut send, &proto::handshake("flood"));
    wait_for(&mut host, &mut seen, |e| {
        matches!(e, Event::Connected { .. })
    });
    let frame = proto::encode(&Frame::Data(vec![9; 100])).unwrap();
    let mut burst = Vec::with_capacity(frame.len() * 100);
    for _ in 0..100 {
        burst.extend_from_slice(&frame);
    }
    // 10 000 frames, written without reading anything back; the
    // writer blocks on flow control once the receiver stops draining,
    // so it runs on its own thread.
    let writer = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut written = 0;
        for _ in 0..100 {
            if Instant::now() > deadline {
                break;
            }
            // The raw runtime turns only inside `block_on`: a short
            // driven sleep after each write puts it on the wire.
            let ok = raw.rt.block_on(async {
                let ok = tokio::time::timeout(Duration::from_secs(1), send.write_all(&burst))
                    .await
                    .is_ok_and(|r| r.is_ok());
                tokio::time::sleep(Duration::from_millis(5)).await;
                ok
            });
            if !ok {
                break;
            }
            written += 1;
        }
        raw.rt
            .block_on(tokio::time::sleep(Duration::from_millis(1500)));
        (raw, send, written)
    });
    // Do not drain for a while: the backlog fills and stays put.
    std::thread::sleep(Duration::from_millis(1500));
    let mut slowest = Duration::ZERO;
    let mut total = 0usize;
    for _ in 0..20 {
        let mut out = Vec::new();
        let p = Instant::now();
        host.poll(&mut out, MAX_BATCH);
        slowest = slowest.max(p.elapsed());
        assert!(out.len() <= MAX_BATCH);
        total += out.len();
    }
    assert!(slowest < Duration::from_millis(50), "{slowest:?}");
    let written = writer.join().map(|(_, _, w)| w).unwrap_or(0);
    assert!(
        total >= MAX_BATCH,
        "the flood reached the host: {total} of {written} bursts; seen {seen:?}"
    );
    assert!(
        total <= 20 * MAX_BATCH,
        "at most the batch bound per poll: {total}"
    );
    drop(host);
    let _ = loopback();
}
