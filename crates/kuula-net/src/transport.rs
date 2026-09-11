//! The Iroh transport: `kuula_core::net::Transport` over a `Net`.
//!
//! The frame path never waits. Commands go down a channel to a driver
//! thread that owns the `Net`, the listener, the join in progress and
//! the session, and does every call that can block (binding, the
//! address wait, the connect, the close) there; events come back up a
//! bounded channel that `poll` drains without waiting. When the channel
//! is full the driver stops reading the session, the session's own
//! queue fills, and QUIC flow control stalls the peer: nothing is lost
//! and nothing grows. Dropping the transport tells the driver to say
//! bye and leave; the drop itself does not wait for it.
//!
//! The `Net` is built on the first `Host` or `Join`, so a permitted
//! cart that never hosts costs no endpoint, and it is dropped on
//! `Leave` or when a joiner's session ends. A host keeps its endpoint
//! and listener after its peer leaves, so the ticket it displayed stays
//! joinable.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

use kuula_core::net::{Command, Event, FailCode, Reason, Transport, INBOX_CAP};

use crate::{Code, Joining, Listener, Net, NetConfig, NetError, Session};

/// Events the driver may hold for `poll` before it stops reading the
/// session.
pub const BACKLOG: usize = INBOX_CAP;

/// How long the driver waits for a command when it has nothing else to
/// do; also the pacing of its session polls.
const TICK: Duration = Duration::from_millis(2);

/// How long the driver waits for a command while idle with no
/// endpoint.
const IDLE_TICK: Duration = Duration::from_millis(50);

/// Events the driver holds back while the cart's channel is full.
const STALLED_CAP: usize = BACKLOG;

/// Driver threads alive: a test hook for proving teardown and the
/// absence of a transport in a replay.
static DRIVERS: AtomicU32 = AtomicU32::new(0);

/// How many driver threads are alive right now.
pub fn live_drivers() -> u32 {
    DRIVERS.load(Ordering::SeqCst)
}

/// The core's code for a net error.
pub fn fail_code(code: Code) -> FailCode {
    match code {
        Code::Disabled => FailCode::Disabled,
        Code::NoAddress => FailCode::NoAddress,
        Code::Ticket => FailCode::Ticket,
        Code::Connect => FailCode::Connect,
        Code::ProtocolMismatch => FailCode::ProtocolMismatch,
        Code::Handshake => FailCode::Handshake,
        Code::Frame => FailCode::Frame,
        Code::Timeout => FailCode::Timeout,
        Code::Busy => FailCode::Busy,
        Code::Cancelled => FailCode::Cancelled,
        Code::QueueFull => FailCode::QueueFull,
    }
}

fn failed(e: &NetError) -> Event {
    Event::failed(fail_code(e.code), e.detail.clone())
}

pub struct IrohTransport {
    cmds: Option<mpsc::Sender<Command>>,
    events: Receiver<Event>,
    cancel: Arc<AtomicBool>,
}

impl std::fmt::Debug for IrohTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IrohTransport")
    }
}

impl IrohTransport {
    /// A transport whose endpoint, when one is built, binds every
    /// interface (`None`) or one address (tests use loopback).
    pub fn new(bind: Option<SocketAddr>) -> IrohTransport {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::sync_channel(BACKLOG);
        let report = ev_tx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let driver = Driver {
            config: NetConfig {
                enabled: true,
                bind,
                cancel: Some(cancel.clone()),
            },
            cmds: cmd_rx,
            events: ev_tx,
            net: None,
            listener: None,
            joining: None,
            session: None,
            stalled: VecDeque::new(),
        };
        DRIVERS.fetch_add(1, Ordering::SeqCst);
        let spawned = std::thread::Builder::new()
            .name("kuula-net-link".into())
            .spawn(move || {
                driver.run();
                DRIVERS.fetch_sub(1, Ordering::SeqCst);
            });
        if let Err(e) = spawned {
            // No driver: the cart set itself hosting or joining and
            // only an event moves it on, so say so before the channel
            // closes.
            DRIVERS.fetch_sub(1, Ordering::SeqCst);
            let _ = report.try_send(Event::failed(
                FailCode::Disabled,
                format!("no driver thread: {e}"),
            ));
        }
        drop(report);
        IrohTransport {
            cmds: Some(cmd_tx),
            events: ev_rx,
            cancel,
        }
    }
}

impl Transport for IrohTransport {
    fn push(&mut self, cmd: Command) {
        if let Some(tx) = &self.cmds {
            let _ = tx.send(cmd);
        }
    }

    fn poll(&mut self, out: &mut Vec<Event>, max: usize) {
        for _ in 0..max {
            match self.events.try_recv() {
                Ok(e) => out.push(e),
                Err(_) => break,
            }
        }
    }
}

impl Drop for IrohTransport {
    fn drop(&mut self) {
        // The driver sees the closed channel, says bye if a session is
        // open and stops; a blocking wait it is in returns through the
        // cancel flag.
        self.cancel.store(true, Ordering::SeqCst);
        self.cmds.take();
    }
}

struct Driver {
    config: NetConfig,
    cmds: Receiver<Command>,
    events: SyncSender<Event>,
    net: Option<Net>,
    listener: Option<Listener>,
    joining: Option<Joining>,
    session: Option<Session>,
    /// An event the channel had no room for; while it waits the
    /// session is not read.
    /// Events the full channel would not take, in order, within
    /// [`STALLED_CAP`]. Message events stop being pulled from the
    /// session while any wait, so what accumulates here is the driver's
    /// own reports.
    stalled: VecDeque<Event>,
}

impl Driver {
    fn run(mut self) {
        loop {
            // Deliver what waits before reading more.
            if !self.flush() {
                break;
            }
            let wait = if self.net.is_none() { IDLE_TICK } else { TICK };
            match self.cmds.recv_timeout(wait) {
                Ok(cmd) => {
                    if !self.handle(cmd) {
                        break;
                    }
                    // Drain the rest of the frame's commands at once.
                    while let Ok(cmd) = self.cmds.try_recv() {
                        if !self.handle(cmd) {
                            return self.finish();
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if !self.pump() {
                break;
            }
        }
        self.finish();
    }

    /// Say bye if a session is open and drop everything.
    fn finish(mut self) {
        if let Some(session) = self.session.take() {
            let _ = session.close();
        }
        self.joining = None;
        self.listener = None;
        self.net = None;
    }

    /// Send what waits, oldest first, until the channel is full again.
    /// False when the receiver is gone.
    fn flush(&mut self) -> bool {
        while let Some(e) = self.stalled.pop_front() {
            match self.events.try_send(e) {
                Ok(()) => {}
                Err(TrySendError::Full(e)) => {
                    self.stalled.push_front(e);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => return false,
            }
        }
        true
    }

    /// Queue an event for the cart, behind whatever already waits so
    /// the order holds. Past [`STALLED_CAP`] the oldest waiting event
    /// goes, as the cart's inbox drops when it is not read.
    fn emit(&mut self, e: Event) -> bool {
        if !self.stalled.is_empty() {
            self.stalled.push_back(e);
            if self.stalled.len() > STALLED_CAP {
                self.stalled.pop_front();
            }
            return true;
        }
        match self.events.try_send(e) {
            Ok(()) => true,
            Err(TrySendError::Full(e)) => {
                self.stalled.push_back(e);
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    fn net(&mut self) -> Result<&Net, NetError> {
        if self.net.is_none() {
            self.net = Some(Net::new(&self.config)?);
        }
        Ok(self.net.as_ref().expect("built above"))
    }

    /// One command; `false` when the transport is gone.
    fn handle(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::Host => {
                if self.listener.is_some() || self.session.is_some() || self.joining.is_some() {
                    return self.emit(Event::failed(FailCode::Declined, "already hosting"));
                }
                let listener = self.net().and_then(|net| net.listen());
                match listener {
                    Ok(l) => {
                        let ticket = l.ticket().to_string();
                        self.listener = Some(l);
                        self.emit(Event::Hosting { ticket })
                    }
                    Err(e) => {
                        self.net = None;
                        self.emit(failed(&e))
                    }
                }
            }
            Command::Join { ticket } => {
                if self.listener.is_some() || self.session.is_some() || self.joining.is_some() {
                    return self.emit(Event::failed(FailCode::Declined, "already joining"));
                }
                match self.net().and_then(|net| net.join_start(&ticket)) {
                    Ok(j) => {
                        self.joining = Some(j);
                        true
                    }
                    Err(e) => {
                        self.net = None;
                        self.emit(failed(&e))
                    }
                }
            }
            Command::Send { data } => {
                let Some(session) = self.session.as_mut() else {
                    return self.emit(Event::failed(FailCode::Declined, "no session"));
                };
                match session.try_send_data(&data) {
                    Ok(()) => true,
                    Err(e) if e.code == Code::QueueFull => self.emit(failed(&e)),
                    Err(e) => {
                        // The session is gone; `pump` reports how.
                        let _ = e;
                        true
                    }
                }
            }
            Command::Leave => {
                if let Some(session) = self.session.take() {
                    let _ = session.close();
                }
                self.joining = None;
                self.listener = None;
                self.net = None;
                true
            }
        }
    }

    /// Advance whatever is in progress; `false` when the transport is
    /// gone.
    fn pump(&mut self) -> bool {
        if !self.stalled.is_empty() {
            return true;
        }
        if let Some(j) = self.joining.as_mut() {
            match j.poll() {
                None => {}
                Some(Ok(session)) => {
                    self.joining = None;
                    self.session = Some(session);
                    return self.emit(Event::Connected { peer: 1 });
                }
                Some(Err(e)) => {
                    self.joining = None;
                    self.net = None;
                    return self.emit(failed(&e));
                }
            }
        }
        if self.session.is_none() {
            if let Some(l) = self.listener.as_mut() {
                match l.try_accept() {
                    Ok(None) => {}
                    Ok(Some(session)) => {
                        self.session = Some(session);
                        return self.emit(Event::Connected { peer: 1 });
                    }
                    Err(e) => {
                        // A joiner that failed its handshake; keep
                        // listening for the next one.
                        let rearmed = l.rearm();
                        if !self.emit(failed(&e)) {
                            return false;
                        }
                        if rearmed.is_err() {
                            self.listener = None;
                            self.net = None;
                        }
                    }
                }
            }
        }
        // Read the session until the backlog is full or it is quiet.
        let mut ended: Option<Vec<Event>> = None;
        for _ in 0..BACKLOG {
            let Some(session) = self.session.as_mut() else {
                break;
            };
            let next = match session.try_recv() {
                Ok(Some(e)) => e,
                Ok(None) => break,
                Err(_) => {
                    ended = Some(vec![Event::Disconnected {
                        reason: Reason::Lost,
                    }]);
                    break;
                }
            };
            let events = match next {
                crate::Event::Data(data) => vec![Event::Message { from: 1, data }],
                // A text from a diagnostic peer is not a cart message.
                crate::Event::Text(_) => continue,
                crate::Event::PeerClosed => vec![Event::Disconnected {
                    reason: Reason::Left,
                }],
                crate::Event::PeerLost(_) => vec![Event::Disconnected {
                    reason: Reason::Lost,
                }],
                crate::Event::Error(e) => vec![
                    failed(&e),
                    Event::Disconnected {
                        reason: Reason::Lost,
                    },
                ],
            };
            if events.len() == 1 && matches!(events[0], Event::Message { .. }) {
                let e = events.into_iter().next().expect("one");
                if !self.emit(e) {
                    return false;
                }
                if !self.stalled.is_empty() {
                    break;
                }
                continue;
            }
            ended = Some(events);
            break;
        }
        if let Some(events) = ended {
            let session = self.session.take();
            drop(session);
            for e in events {
                if !self.emit(e) {
                    return false;
                }
            }
            match self.listener.as_mut() {
                // A host keeps its ticket joinable.
                Some(l) => {
                    if l.rearm().is_err() {
                        self.listener = None;
                        self.net = None;
                    }
                }
                None => self.net = None,
            }
        }
        true
    }
}
