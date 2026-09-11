//! The cart-facing networking contract.
//!
//! A cart asks for things with [`Command`]s during its frame and is told
//! things with [`Event`]s at the top of a later frame. Everything a cart
//! can observe is an event admitted at a frame boundary, so a recorded
//! run replays without a peer: the transcript holds the admitted batch
//! of every frame and the commands the cart issued, and a replay feeds
//! the one and checks the other.
//!
//! Three pieces live here:
//!
//! - [`NetState`], the cart's half, inside its `DrawState`: status,
//!   ticket, peer, the bounded inbox and outbox, counters. Present only
//!   for a cart whose manifest declares `services = ["net"]`.
//! - [`Transport`], the trait a host implements to carry commands and
//!   events somewhere: [`MemoryTransport`] in this crate for tests and
//!   the pair fixtures, the Iroh one in `kuula-net`. Both calls are
//!   non-blocking and bounded.
//! - [`Link`], the host's half: the permission gate, the lazily built
//!   transport, and the bounded queue of events waiting for a frame.
//!
//! The core knows no socket, no async and no Iroh type.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// Longest message payload, in bytes; the wire's `Data` limit.
pub const MAX_DATA: usize = 1024;
/// Longest ticket a cart may hand to `join`.
pub const MAX_TICKET: usize = 1024;
/// Longest failure detail carried on an event.
pub const MAX_DETAIL: usize = 256;
/// Events the inbox holds before the host stops offering more. Control
/// events (a permission change) may push it past this by a few.
pub const INBOX_CAP: usize = 256;
/// Most events admitted in one frame, and the batch bound of a
/// transcript record and an IPC step.
pub const MAX_BATCH: usize = 64;
/// `send` calls one frame may queue.
pub const MAX_SENDS: usize = 16;
/// Commands one frame's outbox may hold in all: the sends plus the
/// control commands (`host`, `join`, `leave`).
pub const MAX_COMMANDS: usize = 32;
/// Events of a `Link`'s own making (a close, a failure) it keeps
/// waiting for a frame; past it, non-control ones are dropped, so a
/// cart that hosts and leaves every frame without reading cannot grow
/// the host's queue.
pub const PENDING_CAP: usize = INBOX_CAP;

/// What a cart asks for during a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Host,
    Join { ticket: String },
    Send { data: Vec<u8> },
    Leave,
}

impl Command {
    /// Whether the command is well formed under the size bounds.
    pub fn check(&self) -> Result<(), String> {
        match self {
            Command::Join { ticket } if ticket.len() > MAX_TICKET => Err(format!(
                "ticket of {} bytes, at most {MAX_TICKET}",
                ticket.len()
            )),
            Command::Send { data } if data.is_empty() || data.len() > MAX_DATA => Err(format!(
                "data of {} bytes, must be 1 to {MAX_DATA}",
                data.len()
            )),
            _ => Ok(()),
        }
    }

    /// The `op` name a transcript records.
    pub fn op(&self) -> &'static str {
        match self {
            Command::Host => "host",
            Command::Join { .. } => "join",
            Command::Send { .. } => "send",
            Command::Leave => "leave",
        }
    }
}

/// Why a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The peer said bye.
    Left,
    /// The transport lost the peer.
    Lost,
    /// This side asked to leave.
    Closed,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Left => "left",
            Reason::Lost => "lost",
            Reason::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<Reason> {
        match s {
            "left" => Some(Reason::Left),
            "lost" => Some(Reason::Lost),
            "closed" => Some(Reason::Closed),
            _ => None,
        }
    }
}

/// The stable code of a `Failed` event: the wire's `net_*` codes plus
/// the three the console adds. A finite set, so a worker's reply and a
/// transcript decode through it and never leak an arbitrary string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailCode {
    /// No permission: the run was not started with networking on.
    Denied,
    /// A command that made no sense in the current status.
    Declined,
    /// The outgoing queue was full; the message was dropped.
    QueueFull,
    Disabled,
    NoAddress,
    Ticket,
    Connect,
    ProtocolMismatch,
    Handshake,
    Frame,
    Timeout,
    Busy,
    Cancelled,
}

impl FailCode {
    pub const ALL: [FailCode; 13] = [
        FailCode::Denied,
        FailCode::Declined,
        FailCode::QueueFull,
        FailCode::Disabled,
        FailCode::NoAddress,
        FailCode::Ticket,
        FailCode::Connect,
        FailCode::ProtocolMismatch,
        FailCode::Handshake,
        FailCode::Frame,
        FailCode::Timeout,
        FailCode::Busy,
        FailCode::Cancelled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FailCode::Denied => "net_denied",
            FailCode::Declined => "net_declined",
            FailCode::QueueFull => "net_queue_full",
            FailCode::Disabled => "net_disabled",
            FailCode::NoAddress => "net_no_address",
            FailCode::Ticket => "net_ticket",
            FailCode::Connect => "net_connect",
            FailCode::ProtocolMismatch => "net_protocol_mismatch",
            FailCode::Handshake => "net_handshake",
            FailCode::Frame => "net_frame",
            FailCode::Timeout => "net_timeout",
            FailCode::Busy => "net_busy",
            FailCode::Cancelled => "net_cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<FailCode> {
        FailCode::ALL.iter().copied().find(|c| c.as_str() == s)
    }
}

/// What a cart is told at the top of a frame, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The host request succeeded; the ticket to hand to the joiner.
    Hosting { ticket: String },
    /// A session is open with the peer.
    Connected { peer: u32 },
    /// The peer sent a message.
    Message { from: u32, data: Vec<u8> },
    /// The session ended.
    Disconnected { reason: Reason },
    /// A request or a session failed; data, never a cart fault.
    Failed { code: FailCode, detail: String },
    /// The host granted or withdrew the player's permission. Withdrawn
    /// permission ends any session and discards every queued event.
    Permission { granted: bool },
}

impl Event {
    /// The `kind` name a cart sees.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Hosting { .. } => "hosting",
            Event::Connected { .. } => "connected",
            Event::Message { .. } => "message",
            Event::Disconnected { .. } => "disconnected",
            Event::Failed { .. } => "failed",
            Event::Permission { .. } => "permission",
        }
    }

    /// A control event is admitted even when the inbox has no room: it
    /// changes what the inbox holds rather than adding to a backlog.
    pub fn is_control(&self) -> bool {
        matches!(self, Event::Permission { .. })
    }

    /// A failure with its detail clipped to [`MAX_DETAIL`].
    pub fn failed(code: FailCode, detail: impl Into<String>) -> Event {
        let mut detail: String = detail.into();
        if detail.len() > MAX_DETAIL {
            let mut end = MAX_DETAIL;
            while !detail.is_char_boundary(end) {
                end -= 1;
            }
            detail.truncate(end);
        }
        Event::Failed { code, detail }
    }

    /// Whether the event is well formed under the size bounds.
    pub fn check(&self) -> Result<(), String> {
        match self {
            Event::Hosting { ticket } if ticket.len() > MAX_TICKET => {
                Err(format!("ticket of {} bytes", ticket.len()))
            }
            Event::Connected { peer } if *peer != 1 => Err(format!("peer {peer}")),
            Event::Message { from, .. } if *from != 1 => Err(format!("peer {from}")),
            Event::Message { data, .. } if data.is_empty() || data.len() > MAX_DATA => {
                Err(format!("data of {} bytes", data.len()))
            }
            Event::Failed { detail, .. } if detail.len() > MAX_DETAIL => {
                Err(format!("detail of {} bytes", detail.len()))
            }
            _ => Ok(()),
        }
    }
}

/// Where the cart stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Off,
    Hosting,
    Joining,
    Connected,
    Ended,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Off => "off",
            Status::Hosting => "hosting",
            Status::Joining => "joining",
            Status::Connected => "connected",
            Status::Ended => "ended",
        }
    }
}

/// The initial environment of a networked run: what the host decided
/// before the cart started. Part of the transcript header.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetEnv {
    /// Whether the player permitted networking for this run.
    pub permitted: bool,
    /// The ticket the run was launched with (`--net join`), if any.
    pub invite: Option<String>,
}

/// The cart's networking state, in its draw state when the manifest
/// declares the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetState {
    pub status: Status,
    pub ticket: Option<String>,
    pub peer: Option<u32>,
    pub permitted: bool,
    pub invite: Option<String>,
    /// Events admitted and not yet read by the cart.
    pub inbox: VecDeque<Event>,
    /// Commands the cart issued this frame.
    pub outbox: Vec<Command>,
    /// The batch admitted at the top of this frame, for the recorder.
    pub admitted: Vec<Event>,
    /// Messages queued by `send`.
    pub sent: u64,
    /// Message events admitted.
    pub received: u64,
    /// Sends refused for a full outbox and events dropped for a full
    /// inbox.
    pub dropped: u64,
    /// Room reported by a remote console (the worker's), which is the
    /// real inbox; `None` when this state is the real one.
    pub remote_room: Option<usize>,
    sends_this_frame: usize,
}

impl NetState {
    pub fn new(env: &NetEnv) -> NetState {
        NetState {
            status: Status::Off,
            ticket: None,
            peer: None,
            permitted: env.permitted,
            invite: env.invite.clone(),
            inbox: VecDeque::new(),
            outbox: Vec::new(),
            admitted: Vec::new(),
            sent: 0,
            received: 0,
            dropped: 0,
            remote_room: None,
            sends_this_frame: 0,
        }
    }

    /// How many events the host may offer for the next frame.
    pub fn room(&self) -> usize {
        if let Some(r) = self.remote_room {
            return r.min(MAX_BATCH);
        }
        INBOX_CAP.saturating_sub(self.inbox.len()).min(MAX_BATCH)
    }

    /// Whether a transport may be alive for this cart.
    pub fn is_live(&self) -> bool {
        !matches!(self.status, Status::Off | Status::Ended)
    }

    /// Start a frame: clear the outbox and admit `events` in order.
    pub fn begin_frame(&mut self, events: Vec<Event>) {
        self.outbox.clear();
        self.sends_this_frame = 0;
        self.admitted.clear();
        self.admit(events);
    }

    /// Admit events: apply each to the status and queue it.
    pub fn admit(&mut self, events: Vec<Event>) {
        for e in events {
            self.apply(&e);
            if self.inbox.len() >= INBOX_CAP && !e.is_control() {
                self.dropped += 1;
                continue;
            }
            self.admitted.push(e.clone());
            self.inbox.push_back(e);
        }
    }

    fn apply(&mut self, e: &Event) {
        match e {
            Event::Hosting { ticket } => {
                self.ticket = Some(ticket.clone());
                if self.status != Status::Connected {
                    self.status = Status::Hosting;
                }
            }
            Event::Connected { peer } => {
                self.status = Status::Connected;
                self.peer = Some(*peer);
            }
            Event::Message { .. } => self.received += 1,
            Event::Disconnected { reason } => {
                self.peer = None;
                match reason {
                    // The cart asked to leave and already moved on.
                    Reason::Closed => {}
                    _ if self.ticket.is_some() => self.status = Status::Hosting,
                    _ => self.status = Status::Ended,
                }
            }
            Event::Failed { .. } => {
                let attempt = (self.status == Status::Hosting && self.ticket.is_none())
                    || self.status == Status::Joining;
                if attempt {
                    self.status = Status::Ended;
                }
            }
            Event::Permission { granted } => {
                self.permitted = *granted;
                if !granted {
                    self.inbox.clear();
                    self.ticket = None;
                    self.peer = None;
                    if self.status != Status::Off {
                        self.status = Status::Ended;
                    }
                }
            }
        }
    }

    /// A locally decided failure, queued for the cart like any event.
    fn fail(&mut self, code: FailCode, detail: &str) {
        self.admit(vec![Event::failed(code, detail)]);
        // Local failures are not part of the host's batch.
        self.admitted.pop();
    }

    /// `net.host()`.
    pub fn host(&mut self) {
        if !self.permitted {
            return self.fail(FailCode::Denied, "networking is not permitted");
        }
        if self.is_live() {
            return self.fail(FailCode::Declined, "already hosting, joining or connected");
        }
        if self.outbox.len() >= MAX_COMMANDS {
            return self.fail(FailCode::Declined, "too many commands this frame");
        }
        self.status = Status::Hosting;
        self.ticket = None;
        self.outbox.push(Command::Host);
    }

    /// `net.join(ticket)`; the ticket has been size-checked.
    pub fn join(&mut self, ticket: String) {
        if !self.permitted {
            return self.fail(FailCode::Denied, "networking is not permitted");
        }
        if self.is_live() {
            return self.fail(FailCode::Declined, "already hosting, joining or connected");
        }
        if self.outbox.len() >= MAX_COMMANDS {
            return self.fail(FailCode::Declined, "too many commands this frame");
        }
        self.status = Status::Joining;
        self.ticket = None;
        self.outbox.push(Command::Join { ticket });
    }

    /// `net.send(data)`: whether it was queued. The data has been
    /// size-checked.
    pub fn send(&mut self, data: Vec<u8>) -> bool {
        if self.status != Status::Connected {
            return false;
        }
        if self.sends_this_frame >= MAX_SENDS || self.outbox.len() >= MAX_COMMANDS {
            self.dropped += 1;
            return false;
        }
        self.sends_this_frame += 1;
        self.sent += 1;
        self.outbox.push(Command::Send { data });
        true
    }

    /// `net.leave()`.
    pub fn leave(&mut self) {
        if !self.is_live() {
            return;
        }
        self.status = Status::Ended;
        self.ticket = None;
        self.peer = None;
        if self.outbox.len() < MAX_COMMANDS {
            self.outbox.push(Command::Leave);
        }
    }

    /// `net.recv()`.
    pub fn recv(&mut self) -> Option<Event> {
        self.inbox.pop_front()
    }
}

/// Carries commands out and events in, without blocking. `poll`
/// appends at most `max` events that have arrived since the last call,
/// in order, and leaves the rest for later, so a slow cart stalls its
/// peer through the transport's own flow control rather than growing a
/// backlog here.
pub trait Transport {
    fn push(&mut self, cmd: Command);
    fn poll(&mut self, out: &mut Vec<Event>, max: usize);
}

// ----- the memory transport ---------------------------------------------------

/// The ticket the memory pair's host hands out.
pub const MEMORY_TICKET: &str = "memory:pair";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SideStatus {
    Off,
    Hosting,
    Connected,
}

#[derive(Default)]
struct Side {
    status: Option<SideStatus>,
    /// Whether this side called `host`: a host goes back to listening
    /// when its peer leaves, a joiner goes back to off.
    host: bool,
    /// Events for this side, each with the poll count it becomes
    /// deliverable at.
    queue: VecDeque<(u64, Event)>,
    polls: u64,
    alive: bool,
}

impl Side {
    /// The status after the peer of a connected side goes away.
    fn after_peer(&self) -> SideStatus {
        if self.host {
            SideStatus::Hosting
        } else {
            SideStatus::Off
        }
    }
}

#[derive(Default)]
struct Hub {
    sides: [Side; 2],
    /// Polls an event waits before it is deliverable.
    delay: u64,
    /// Queue length at which a send is refused as full.
    capacity: usize,
}

/// Two transports joined in process. `Host` on either side yields
/// `Hosting` with [`MEMORY_TICKET`]; `Join` with that ticket connects
/// them; `Send` delivers to the other side; `Leave` tells the other side
/// `Left`; dropping a side tells the other `Lost`. Knobs: a delivery
/// delay in polls and a queue capacity, so a harness can stall and
/// pressure a cart.
pub struct MemoryTransport {
    hub: Rc<RefCell<Hub>>,
    me: usize,
}

impl MemoryTransport {
    pub fn pair() -> (MemoryTransport, MemoryTransport) {
        let hub = Rc::new(RefCell::new(Hub {
            sides: [
                Side {
                    status: Some(SideStatus::Off),
                    alive: true,
                    ..Side::default()
                },
                Side {
                    status: Some(SideStatus::Off),
                    alive: true,
                    ..Side::default()
                },
            ],
            delay: 0,
            capacity: INBOX_CAP,
        }));
        (
            MemoryTransport {
                hub: hub.clone(),
                me: 0,
            },
            MemoryTransport { hub, me: 1 },
        )
    }

    /// Polls an event waits before it is delivered (default 0).
    pub fn set_delay(&self, polls: u64) {
        self.hub.borrow_mut().delay = polls;
    }

    /// Queued events at which the other side's send is refused as
    /// `net_queue_full` (default [`INBOX_CAP`]).
    pub fn set_capacity(&self, n: usize) {
        self.hub.borrow_mut().capacity = n;
    }

    fn other(&self) -> usize {
        1 - self.me
    }
}

impl Hub {
    fn deliver(&mut self, to: usize, e: Event) {
        let due = self.sides[to].polls + self.delay;
        self.sides[to].queue.push_back((due, e));
    }
}

impl Transport for MemoryTransport {
    fn push(&mut self, cmd: Command) {
        let (me, other) = (self.me, self.other());
        let mut hub = self.hub.borrow_mut();
        let status = hub.sides[me].status.unwrap_or(SideStatus::Off);
        match cmd {
            Command::Host => {
                if status != SideStatus::Off {
                    hub.deliver(me, Event::failed(FailCode::Declined, "already hosting"));
                } else {
                    hub.sides[me].status = Some(SideStatus::Hosting);
                    hub.sides[me].host = true;
                    hub.deliver(
                        me,
                        Event::Hosting {
                            ticket: MEMORY_TICKET.to_string(),
                        },
                    );
                }
            }
            Command::Join { ticket } => {
                if ticket != MEMORY_TICKET {
                    hub.deliver(me, Event::failed(FailCode::Ticket, "not a memory ticket"));
                } else if !hub.sides[other].alive
                    || hub.sides[other].status != Some(SideStatus::Hosting)
                {
                    hub.deliver(me, Event::failed(FailCode::Connect, "no host is listening"));
                } else {
                    hub.sides[me].status = Some(SideStatus::Connected);
                    hub.sides[other].status = Some(SideStatus::Connected);
                    hub.deliver(me, Event::Connected { peer: 1 });
                    hub.deliver(other, Event::Connected { peer: 1 });
                }
            }
            Command::Send { data } => {
                if status != SideStatus::Connected {
                    hub.deliver(me, Event::failed(FailCode::Declined, "no session"));
                } else if hub.sides[other].queue.len() >= hub.capacity {
                    hub.deliver(
                        me,
                        Event::failed(FailCode::QueueFull, "the peer's queue is full"),
                    );
                } else {
                    hub.deliver(other, Event::Message { from: 1, data });
                }
            }
            Command::Leave => {
                if status == SideStatus::Connected {
                    hub.deliver(
                        other,
                        Event::Disconnected {
                            reason: Reason::Left,
                        },
                    );
                    // A host keeps listening after its peer leaves.
                    hub.sides[other].status = Some(hub.sides[other].after_peer());
                }
                hub.sides[me].status = Some(SideStatus::Off);
                hub.sides[me].host = false;
                hub.sides[me].queue.clear();
            }
        }
    }

    fn poll(&mut self, out: &mut Vec<Event>, max: usize) {
        let mut hub = self.hub.borrow_mut();
        let side = &mut hub.sides[self.me];
        side.polls += 1;
        let mut n = 0;
        while n < max {
            match side.queue.front() {
                Some((due, _)) if *due < side.polls => {
                    let (_, e) = side.queue.pop_front().expect("checked");
                    out.push(e);
                    n += 1;
                }
                _ => break,
            }
        }
    }
}

impl Drop for MemoryTransport {
    fn drop(&mut self) {
        let (me, other) = (self.me, self.other());
        let mut hub = self.hub.borrow_mut();
        hub.sides[me].alive = false;
        if hub.sides[me].status == Some(SideStatus::Connected) {
            hub.sides[other].status = Some(hub.sides[other].after_peer());
            hub.deliver(
                other,
                Event::Disconnected {
                    reason: Reason::Lost,
                },
            );
        }
        hub.sides[me].status = Some(SideStatus::Off);
    }
}

// ----- the link -----------------------------------------------------------------

/// Builds a transport when a permitted cart first hosts or joins.
pub type TransportFactory = Box<dyn FnMut() -> Box<dyn Transport>>;

/// The host's half: owns the transport, applies the permission gate
/// and keeps the events waiting for a frame. Every host that runs a
/// networked cart (headless runner, window, broker, MCP) steps through
/// one of these; a replay has none.
pub struct Link {
    transport: Option<Box<dyn Transport>>,
    factory: Option<TransportFactory>,
    permitted: bool,
    pending: VecDeque<Event>,
    constructed: u32,
}

impl Link {
    /// A link that builds its transport with `factory` on the first
    /// `Host` or `Join`. Not permitted until `set_permitted(true)`.
    pub fn new(factory: TransportFactory) -> Link {
        Link {
            transport: None,
            factory: Some(factory),
            permitted: false,
            pending: VecDeque::new(),
            constructed: 0,
        }
    }

    /// A link with no transport at all: every command is dropped.
    pub fn offline() -> Link {
        Link {
            transport: None,
            factory: None,
            permitted: false,
            pending: VecDeque::new(),
            constructed: 0,
        }
    }

    /// A permitted link over a ready transport (the memory pair).
    pub fn over(transport: Box<dyn Transport>) -> Link {
        let cell = RefCell::new(Some(transport));
        let mut link = Link::new(Box::new(move || {
            cell.borrow_mut()
                .take()
                .expect("a link over one transport hosts or joins once")
        }));
        link.permitted = true;
        link
    }

    pub fn is_permitted(&self) -> bool {
        self.permitted
    }

    /// Grant or withdraw permission. Withdrawing it ends any session,
    /// drops the transport and discards every waiting event; the cart
    /// is told through a `Permission` event.
    pub fn set_permitted(&mut self, on: bool) {
        if on == self.permitted {
            return;
        }
        self.permitted = on;
        if !on {
            self.close();
            self.pending.clear();
        }
        self.queue(Event::Permission { granted: on });
    }

    /// Queue an event of this link's own making, within [`PENDING_CAP`]:
    /// past it a non-control event is dropped, as the cart's inbox
    /// drops, so the queue is bounded whether or not the cart reads.
    fn queue(&mut self, e: Event) {
        if !e.is_control() && self.pending.len() >= PENDING_CAP {
            return;
        }
        self.pending.push_back(e);
    }

    /// Whether a transport is currently constructed.
    pub fn is_live(&self) -> bool {
        self.transport.is_some()
    }

    /// How many transports this link has built: a test hook, so a
    /// replay or a denied run can prove none was.
    pub fn constructions(&self) -> u32 {
        self.constructed
    }

    fn close(&mut self) {
        if let Some(mut t) = self.transport.take() {
            t.push(Command::Leave);
        }
    }

    /// Hand the frame's commands to the transport. Without permission
    /// every command is dropped (the cart already saw `net_denied`), and
    /// a `Leave` always closes.
    pub fn push(&mut self, commands: Vec<Command>) {
        // One transport per frame: a cart that hosts and leaves in the
        // same frame builds one, not one per command pair.
        let mut built = false;
        for cmd in commands {
            if cmd.check().is_err() {
                continue;
            }
            match cmd {
                Command::Leave => {
                    if self.transport.is_some() {
                        self.close();
                        self.queue(Event::Disconnected {
                            reason: Reason::Closed,
                        });
                    }
                }
                _ if !self.permitted => {}
                Command::Host | Command::Join { .. } => {
                    if self.transport.is_none() {
                        if built {
                            self.queue(Event::failed(FailCode::Declined, "one session per frame"));
                            continue;
                        }
                        match self.factory.as_mut() {
                            Some(make) => {
                                self.transport = Some(make());
                                self.constructed += 1;
                                built = true;
                            }
                            None => {
                                self.queue(Event::failed(
                                    FailCode::Disabled,
                                    "this host has no transport",
                                ));
                                continue;
                            }
                        }
                    }
                    self.transport.as_mut().expect("built above").push(cmd);
                }
                Command::Send { .. } => {
                    if let Some(t) = self.transport.as_mut() {
                        t.push(cmd);
                    }
                }
            }
        }
    }

    /// Up to `room` events for the next frame: what was waiting, then
    /// what the transport has. Control events do not count.
    pub fn poll(&mut self, out: &mut Vec<Event>, room: usize) {
        let mut n = 0;
        while let Some(e) = self.pending.front() {
            if !e.is_control() && n >= room {
                break;
            }
            let e = self.pending.pop_front().expect("peeked");
            if !e.is_control() {
                n += 1;
            }
            out.push(e);
        }
        if let Some(t) = self.transport.as_mut() {
            if n < room {
                t.poll(out, room - n);
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
}
