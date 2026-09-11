//! Two endpoints in one process, offline, on loopback: the happy path,
//! every failure code the protocol defines, and start/stop cycles.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh_tickets::endpoint::EndpointTicket;
use tokio::runtime::Runtime;

use crate::proto::{self, Frame};
use crate::{Code, Event, Net, NetConfig, NetError, Session};

pub(crate) fn loopback() -> NetConfig {
    NetConfig {
        enabled: true,
        bind: Some("127.0.0.1:0".parse().unwrap()),
        cancel: None,
    }
}

/// A loopback config with a cancel flag, and the flag.
fn cancellable() -> (NetConfig, Arc<AtomicBool>) {
    let flag = Arc::new(AtomicBool::new(false));
    let config = NetConfig {
        cancel: Some(flag.clone()),
        ..loopback()
    };
    (config, flag)
}

/// Set `flag` after `after`, from another thread.
fn cancel_after(flag: &Arc<AtomicBool>, after: Duration) {
    let flag = flag.clone();
    std::thread::spawn(move || {
        std::thread::sleep(after);
        flag.store(true, Ordering::SeqCst);
    });
}

pub(crate) fn net() -> Net {
    Net::new(&loopback()).unwrap()
}

pub(crate) const WAIT: Duration = Duration::from_secs(5);

/// A listener and a joiner with the handshake done.
pub(crate) fn pair() -> (Net, Session, Net, Session) {
    let a = net();
    let mut listener = a.listen().unwrap();
    let b = net();
    let joiner = b.join(listener.ticket()).unwrap();
    let listened = listener.accept(WAIT).unwrap().expect("a session");
    (a, listened, b, joiner)
}

fn expect_text(s: &mut Session, text: &str) {
    assert_eq!(s.recv(WAIT), Some(Event::Text(text.to_string())));
}

#[test]
fn disabled_config_is_refused() {
    let e = Net::new(&NetConfig::default()).unwrap_err();
    assert_eq!(e.code, Code::Disabled);
}

#[test]
fn ticket_carries_a_loopback_address() {
    let a = net();
    let listener = a.listen().unwrap();
    let ticket: EndpointTicket = listener.ticket().parse().unwrap();
    let addrs: Vec<&SocketAddr> = ticket.endpoint_addr().ip_addrs().collect();
    assert!(!addrs.is_empty());
    assert!(
        addrs.iter().all(|a| a.ip().is_loopback()),
        "explicit loopback bind offers only loopback: {addrs:?}"
    );
    assert_eq!(ticket.endpoint_addr().relay_urls().count(), 0);
    assert!(listener.ticket().starts_with("endpoint"));
}

#[test]
fn greeting_ping_and_close_both_ways() {
    let (_a, mut l, _b, mut j) = pair();
    assert_eq!(l.peer_runtime(), crate::RUNTIME_VERSION);
    assert_eq!(j.peer_runtime(), crate::RUNTIME_VERSION);
    assert_ne!(l.peer_id(), j.peer_id());

    l.send_text("hello from listener").unwrap();
    j.send_text("hello from joiner").unwrap();
    expect_text(&mut j, "hello from listener");
    expect_text(&mut l, "hello from joiner");
    let rtt = j.ping().unwrap();
    assert!(rtt < Duration::from_secs(1), "{rtt:?}");
    let rtt = l.ping().unwrap();
    assert!(rtt < Duration::from_secs(1), "{rtt:?}");

    // The joiner leaves; the listener hears Bye.
    let t = Instant::now();
    j.close().unwrap();
    assert_eq!(l.recv(WAIT), Some(Event::PeerClosed));
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    assert_eq!(l.recv(Duration::from_millis(100)), None);
    assert_eq!(l.send_text("late").unwrap_err().code, Code::Connect);

    // And the other way round.
    let (_a, l, _b, mut j) = pair();
    l.close().unwrap();
    assert_eq!(j.recv(WAIT), Some(Event::PeerClosed));
}

#[test]
fn a_hundred_messages_each_way() {
    let (_a, mut l, _b, mut j) = pair();
    for i in 0..100 {
        l.send_text(&format!("l{i}")).unwrap();
        j.send_text(&format!("j{i}")).unwrap();
    }
    for i in 0..100 {
        expect_text(&mut j, &format!("l{i}"));
        expect_text(&mut l, &format!("j{i}"));
    }
}

#[test]
fn text_limits_are_checked_locally() {
    let (_a, mut l, _b, _j) = pair();
    assert_eq!(l.send_text("").unwrap_err().code, Code::Frame);
    let big = "x".repeat(proto::MAX_TEXT + 1);
    assert_eq!(l.send_text(&big).unwrap_err().code, Code::Frame);
    l.send_text(&"x".repeat(proto::MAX_TEXT)).unwrap();
}

#[test]
fn malformed_tickets_are_refused() {
    let a = net();
    for bad in ["", "nonsense", "endpointaaaa", "endpoint"] {
        assert_eq!(a.join(bad).unwrap_err().code, Code::Ticket, "{bad:?}");
    }
}

#[test]
fn absent_peer_is_a_bounded_connect_failure() {
    let ticket = {
        let a = net();
        a.listen().unwrap().ticket().to_string()
    };
    let b = net();
    let t = Instant::now();
    let e = b.join(&ticket).unwrap_err();
    assert_eq!(e.code, Code::Connect, "{e}");
    assert!(
        t.elapsed() < proto::CONNECT_DEADLINE + Duration::from_secs(2),
        "{:?}",
        t.elapsed()
    );
}

#[test]
fn protocol_version_mismatch_is_named() {
    let a = Net::with_alpn(&loopback(), b"kuula/play/0").unwrap();
    let mut listener = a.listen().unwrap();
    let b = net();
    let e = b.join(listener.ticket()).unwrap_err();
    assert_eq!(e.code, Code::ProtocolMismatch, "{e}");
    // The listener's side fails at the QUIC accept, and the accept
    // loop reports that rather than dropping the listener's channel.
    let e = listener.accept(WAIT).unwrap_err();
    assert_eq!(e.code, Code::ProtocolMismatch, "{e}");
}

#[test]
fn second_joiner_is_busy() {
    let (_a, mut l, _b, mut j) = pair();
    let c = net();
    let ticket = {
        // The ticket is the listener's; take it from a fresh listen on
        // the same endpoint, which is refused while one waits, so read
        // the address off the first session's peer instead.
        let t = EndpointTicket::new(_a.inner.endpoint.addr());
        t.to_string()
    };
    let e = c.join(&ticket).unwrap_err();
    assert_eq!(e.code, Code::Busy, "{e}");
    // The first session is untouched.
    l.send_text("still here").unwrap();
    expect_text(&mut j, "still here");
}

#[test]
fn listening_twice_is_busy_until_the_first_is_dropped() {
    let a = net();
    let first = a.listen().unwrap();
    assert_eq!(a.listen().unwrap_err().code, Code::Busy);
    drop(first);
    a.listen().unwrap();
}

/// A listener that was dropped is no listener: the joiner is refused
/// as busy rather than handshaken with nobody to hand the session to.
#[test]
fn a_dropped_listener_refuses_joiners_as_busy() {
    let a = net();
    let listener = a.listen().unwrap();
    let ticket = listener.ticket().to_string();
    drop(listener);
    let b = net();
    let e = b.join(&ticket).unwrap_err();
    assert_eq!(e.code, Code::Busy, "{e}");
}

/// The cancel flag cuts a connect to an absent peer short, well inside
/// the 5 s connect deadline.
#[test]
fn a_join_to_an_absent_peer_can_be_cancelled() {
    let ticket = {
        let a = net();
        a.listen().unwrap().ticket().to_string()
    };
    let (config, flag) = cancellable();
    let b = Net::new(&config).unwrap();
    cancel_after(&flag, Duration::from_millis(300));
    let t = Instant::now();
    let e = b.join(&ticket).unwrap_err();
    assert_eq!(e.code, Code::Cancelled, "{e}");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    // Set, the flag makes every later call return at once.
    let l = net();
    let listener = l.listen().unwrap();
    let t = Instant::now();
    assert_eq!(b.join(listener.ticket()).unwrap_err().code, Code::Cancelled);
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
}

/// The cancel flag cuts a wait for a peer short, and a ping to a peer
/// that withholds its pong; the session still says bye afterwards.
#[test]
fn a_cancelled_ping_still_closes_cleanly() {
    let (config, flag) = cancellable();
    let a = Net::new(&config).unwrap();
    let mut listener = a.listen().unwrap();
    let raw = Raw::new();
    let (conn, mut send, mut recv) = raw.connect(listener.ticket());
    raw.write(&mut send, &proto::handshake("raw"));
    let mut s = listener.accept(WAIT).unwrap().unwrap();
    assert_eq!(raw.read_handshake(&mut recv), crate::RUNTIME_VERSION);
    cancel_after(&flag, Duration::from_millis(300));
    let t = Instant::now();
    let e = s.ping().unwrap_err();
    assert_eq!(e.code, Code::Cancelled, "{e}");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    // The close is not cancelled: the raw peer reads the Bye and a
    // code-0 close follows within the bye deadline.
    let t = Instant::now();
    s.close().unwrap();
    assert!(
        t.elapsed() < proto::BYE_DEADLINE + Duration::from_secs(1),
        "{:?}",
        t.elapsed()
    );
    let closed = raw.rt.block_on(conn.closed());
    let code = match closed {
        iroh::endpoint::ConnectionError::ApplicationClosed(c) => u64::from(c.error_code),
        other => panic!("{other:?}"),
    };
    assert_eq!(code, u64::from(proto::close::BYE));
}

/// A ping whose pong never comes ends the session with code 3, as
/// `docs/net.md` says, rather than only failing the call.
#[test]
fn a_missing_pong_ends_the_session_with_code_3() {
    let (_a, mut s, raw, conn, _send, _recv) = raw_session();
    // The raw runtime only turns inside `block_on`, so it watches for
    // the close on a thread of its own while the ping runs out.
    let closed = std::thread::scope(|scope| {
        let watcher = scope.spawn(|| raw.rt.block_on(conn.closed()));
        let t = Instant::now();
        let e = s.ping().unwrap_err();
        assert_eq!(e.code, Code::Timeout, "{e}");
        assert!(t.elapsed() >= proto::PING_DEADLINE - Duration::from_millis(200));
        match s.recv(WAIT) {
            Some(Event::Error(e)) => assert_eq!(e.code, Code::Timeout, "{e}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(s.send_text("late").unwrap_err().code, Code::Connect);
        watcher.join().unwrap()
    });
    let code = match closed {
        iroh::endpoint::ConnectionError::ApplicationClosed(c) => u64::from(c.error_code),
        other => panic!("{other:?}"),
    };
    assert_eq!(code, u64::from(proto::close::TIMEOUT));
}

/// A caller that has not drained its events still gets its `Bye`
/// through: the event queue fills, the reader stalls, and close ends
/// the session within its bound with the peer told.
#[test]
fn a_full_event_queue_still_lets_close_through() {
    let (_a, l, _b, mut j) = pair();
    for i in 0..(proto::EVENT_QUEUE + 40) {
        j.send_text(&format!("flood {i}")).unwrap();
    }
    // Let the flood land and the listener's queue fill.
    std::thread::sleep(Duration::from_millis(500));
    let t = Instant::now();
    l.close().unwrap();
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert_eq!(j.recv(WAIT), Some(Event::PeerClosed));
}

/// A QUIC peer that speaks the ALPN but not the protocol.
pub(crate) struct Raw {
    pub(crate) rt: Runtime,
    pub(crate) ep: iroh::Endpoint,
}

impl Raw {
    pub(crate) fn new() -> Raw {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ep = rt.block_on(crate::bind(&loopback(), proto::ALPN)).unwrap();
        Raw { rt, ep }
    }

    pub(crate) fn connect(&self, ticket: &str) -> (Connection, SendStream, RecvStream) {
        let ticket: EndpointTicket = ticket.parse().unwrap();
        self.rt.block_on(async {
            let conn = self
                .ep
                .connect(ticket.endpoint_addr().clone(), proto::ALPN)
                .await
                .unwrap();
            let (s, r) = conn.open_bi().await.unwrap();
            (conn, s, r)
        })
    }

    pub(crate) fn write(&self, send: &mut SendStream, bytes: &[u8]) {
        self.rt.block_on(async {
            send.write_all(bytes).await.unwrap();
            // Let the endpoint's tasks put it on the wire.
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
    }

    fn read_handshake(&self, recv: &mut RecvStream) -> String {
        self.rt.block_on(async {
            let mut h = [0u8; proto::HANDSHAKE_HEADER];
            recv.read_exact(&mut h).await.unwrap();
            let n = proto::handshake_header(&h).unwrap();
            let mut body = vec![0u8; n];
            recv.read_exact(&mut body).await.unwrap();
            proto::handshake_body(&body).unwrap()
        })
    }
}

fn bad_handshake(bytes: &[u8]) -> NetError {
    let a = net();
    let mut listener = a.listen().unwrap();
    let raw = Raw::new();
    let (_conn, mut send, _recv) = raw.connect(listener.ticket());
    raw.write(&mut send, bytes);
    listener.accept(WAIT).unwrap_err()
}

#[test]
fn bad_handshakes_are_net_handshake() {
    let mut v2 = proto::handshake("raw");
    v2[5] = 2;
    let e = bad_handshake(&v2);
    assert_eq!(e.code, Code::Handshake, "{e}");
    assert!(e.detail.contains("version 2"), "{e}");

    let mut long = proto::handshake("raw");
    long[6..8].copy_from_slice(&65u16.to_be_bytes());
    let e = bad_handshake(&long);
    assert_eq!(e.code, Code::Handshake, "{e}");

    let e = bad_handshake(b"HTTP/1.1");
    assert_eq!(e.code, Code::Handshake, "{e}");
}

/// A raw peer past the handshake, and the listener's session.
pub(crate) fn raw_session() -> (Net, Session, Raw, Connection, SendStream, RecvStream) {
    let a = net();
    let mut listener = a.listen().unwrap();
    let raw = Raw::new();
    let (conn, mut send, mut recv) = raw.connect(listener.ticket());
    raw.write(&mut send, &proto::handshake("raw"));
    let session = listener.accept(WAIT).unwrap().unwrap();
    assert_eq!(session.peer_runtime(), "raw");
    assert_eq!(raw.read_handshake(&mut recv), crate::RUNTIME_VERSION);
    (a, session, raw, conn, send, recv)
}

#[test]
fn oversized_and_unknown_frames_are_net_frame() {
    let (_a, mut s, raw, _c, mut send, _r) = raw_session();
    let mut header = [proto::TYPE_TEXT, 0, 0, 0, 0];
    header[1..].copy_from_slice(&(1024u32 * 1024).to_be_bytes());
    raw.write(&mut send, &header);
    match s.recv(WAIT) {
        Some(Event::Error(e)) => {
            assert_eq!(e.code, Code::Frame);
            assert!(e.detail.contains("1048576"), "{e}");
        }
        other => panic!("{other:?}"),
    }

    let (_a, mut s, raw, _c, mut send, _r) = raw_session();
    raw.write(&mut send, &[42, 0, 0, 0, 0]);
    match s.recv(WAIT) {
        Some(Event::Error(e)) => assert_eq!(e.code, Code::Frame),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_stalled_frame_is_net_timeout() {
    let (_a, mut s, raw, _c, mut send, _r) = raw_session();
    raw.write(&mut send, &[proto::TYPE_TEXT]);
    let t = Instant::now();
    match s.recv(proto::FRAME_DEADLINE + WAIT) {
        Some(Event::Error(e)) => assert_eq!(e.code, Code::Timeout, "{e}"),
        other => panic!("{other:?}"),
    }
    assert!(t.elapsed() >= proto::FRAME_DEADLINE - Duration::from_millis(200));
}

#[test]
fn a_raw_peer_that_vanishes_is_peer_lost_within_the_idle_timeout() {
    let (_a, mut s, raw, conn, send, recv) = raw_session();
    s.send_text("are you there").unwrap();
    // Drop the raw runtime without closing anything: no packet leaves.
    drop((send, recv));
    std::mem::forget(conn);
    raw.rt.shutdown_background();
    std::mem::forget(raw.ep);
    let t = Instant::now();
    match s.recv(proto::IDLE_TIMEOUT + WAIT) {
        Some(Event::PeerLost(why)) => assert!(!why.is_empty()),
        other => panic!("{other:?}"),
    }
    assert!(
        t.elapsed() < proto::IDLE_TIMEOUT + Duration::from_secs(3),
        "{:?}",
        t.elapsed()
    );
}

#[test]
fn a_raw_peer_gets_its_pings_answered() {
    let (_a, _s, raw, _c, mut send, mut recv) = raw_session();
    raw.write(&mut send, &proto::encode(&Frame::Ping(77)).unwrap());
    let frame = raw.rt.block_on(async {
        let mut h = [0u8; proto::FRAME_HEADER];
        recv.read_exact(&mut h).await.unwrap();
        let (ty, len) = proto::frame_header(&h).unwrap();
        let mut body = vec![0u8; len];
        recv.read_exact(&mut body).await.unwrap();
        proto::frame_body(ty, &body).unwrap()
    });
    assert_eq!(frame, Frame::Pong(77));
}

#[test]
fn listen_join_close_cycles() {
    let t = Instant::now();
    for i in 0..10 {
        let (_a, mut l, _b, mut j) = pair();
        j.send_text(&format!("cycle {i}")).unwrap();
        expect_text(&mut l, &format!("cycle {i}"));
        l.close().unwrap();
        assert_eq!(j.recv(WAIT), Some(Event::PeerClosed));
    }
    assert!(t.elapsed() < Duration::from_secs(30), "{:?}", t.elapsed());
}

#[test]
fn a_net_stops_within_the_bound() {
    let t = Instant::now();
    for _ in 0..20 {
        let a = net();
        let _l = a.listen().unwrap();
        drop(_l);
        a.shutdown();
    }
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
}
