//! Transcript format version 2: the networking records.
//!
//! Version 2 is written only for a cart that declares the `net`
//! service; every other cart keeps writing version 1, and the reader
//! accepts both. The header gains `services`, `invite` and `net` (the
//! permission the run had). After the save chunks come the input runs
//! as in version 1, but no run spans past a frame that has a network
//! record, and that record follows the run:
//!
//! ```text
//! { at = 12, events = { [1] = { kind = "connected", peer = 1 } }, io = { [1] = { op = "send", data = blob"..." } } }
//! ```
//!
//! `events` is the batch admitted at the top of frame `at`, in order
//! and of every kind in one array so the order survives; `io` is the
//! commands the cart issued during that frame. At most `MAX_BATCH`
//! events and `MAX_COMMANDS` commands per record, so a record fits the
//! line limit with room. The version 1 refusal of the reserved keys
//! stays for version 1 files.
//!
//! Replay installs a [`ReplayGuest`] around the cart: it admits each
//! frame's recorded events and compares the commands the cart issues
//! with the recorded ones. A cart that issues a different command, or
//! one on a different frame, ends the replay with
//! `transcript_divergence`. No transport exists in a replay.

use std::collections::VecDeque;

use crate::codec::{Key, Table, Value};
use crate::console::Guest;
use crate::draw::DrawState;
use crate::fault::Fault;
use crate::input::FrameInput;
use crate::net::{Command, Event, FailCode, NetEnv, Reason, MAX_BATCH, MAX_COMMANDS};

use super::{get_int, get_str, int_of, table, TranscriptError};

/// The fault code a replay ends with when the cart's commands differ
/// from the recorded ones.
pub const DIVERGENCE: &str = "transcript_divergence";

/// Bytes of network payload one recording may hold before it stops
/// recording and reports itself incomplete: well under the reader's
/// file limit once encoded.
pub const NET_BYTE_BUDGET: usize = 40 * 1024 * 1024;

/// One frame's network traffic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameNet {
    /// The 1-based frame.
    pub at: u64,
    pub events: Vec<Event>,
    pub commands: Vec<Command>,
}

impl FrameNet {
    /// Rough encoded size, for the recording budget.
    fn cost(&self) -> usize {
        let mut n = 32;
        for e in &self.events {
            n += 48;
            match e {
                Event::Message { data, .. } => n += data.len() * 4 / 3,
                Event::Hosting { ticket } => n += ticket.len(),
                Event::Failed { detail, .. } => n += detail.len(),
                _ => {}
            }
        }
        for c in &self.commands {
            n += 32;
            match c {
                Command::Send { data } => n += data.len() * 4 / 3,
                Command::Join { ticket } => n += ticket.len(),
                _ => {}
            }
        }
        n
    }
}

/// The networking half of a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetRecord {
    /// The services the cart declared, as the manifest spells them.
    pub services: Vec<String>,
    pub env: NetEnv,
    /// Frames that had events or commands, in frame order.
    pub frames: Vec<FrameNet>,
}

impl NetRecord {
    pub fn new(env: NetEnv) -> NetRecord {
        NetRecord {
            services: vec!["net".to_string()],
            env,
            frames: Vec::new(),
        }
    }
}

// ----- encoding -----------------------------------------------------------

fn list(items: Vec<Value>) -> Result<Value, TranscriptError> {
    let mut t = Table::new();
    for (i, v) in items.into_iter().enumerate() {
        t.insert(Key::Int(i as i64 + 1), v)
            .map_err(|e| TranscriptError::new(TranscriptError::SIZE, e.to_string()))?;
    }
    Ok(Value::Table(t))
}

fn event_value(e: &Event) -> Result<Value, TranscriptError> {
    let kind = Value::str(e.kind());
    match e {
        Event::Hosting { ticket } => table(vec![("kind", kind), ("ticket", Value::str(ticket))]),
        Event::Connected { peer } => {
            table(vec![("kind", kind), ("peer", Value::Int(*peer as i64))])
        }
        Event::Message { from, data } => table(vec![
            ("data", Value::Blob(data.clone())),
            ("from", Value::Int(*from as i64)),
            ("kind", kind),
        ]),
        Event::Disconnected { reason } => table(vec![
            ("kind", kind),
            ("reason", Value::str(reason.as_str())),
        ]),
        Event::Failed { code, detail } => table(vec![
            ("code", Value::str(code.as_str())),
            ("detail", Value::str(detail)),
            ("kind", kind),
        ]),
        Event::Permission { granted } => {
            table(vec![("granted", Value::Bool(*granted)), ("kind", kind)])
        }
    }
}

fn command_value(c: &Command) -> Result<Value, TranscriptError> {
    let op = Value::str(c.op());
    match c {
        Command::Host | Command::Leave => table(vec![("op", op)]),
        Command::Join { ticket } => table(vec![("op", op), ("ticket", Value::str(ticket))]),
        Command::Send { data } => table(vec![("data", Value::Blob(data.clone())), ("op", op)]),
    }
}

/// The record line for one frame.
pub(super) fn frame_value(f: &FrameNet) -> Result<Value, TranscriptError> {
    if f.events.len() > MAX_BATCH || f.commands.len() > MAX_COMMANDS {
        return Err(TranscriptError::new(
            TranscriptError::SIZE,
            format!(
                "frame {} has {} events and {} commands",
                f.at,
                f.events.len(),
                f.commands.len()
            ),
        ));
    }
    let events = f
        .events
        .iter()
        .map(event_value)
        .collect::<Result<Vec<_>, _>>()?;
    let commands = f
        .commands
        .iter()
        .map(command_value)
        .collect::<Result<Vec<_>, _>>()?;
    table(vec![
        ("at", Value::Int(f.at as i64)),
        ("events", list(events)?),
        ("io", list(commands)?),
    ])
}

// ----- decoding -----------------------------------------------------------

fn format(n: usize, what: impl std::fmt::Display) -> TranscriptError {
    TranscriptError::format(format!("line {}: {what}", n + 1))
}

fn entries<'a>(v: &'a Value, key: &str, n: usize) -> Result<Vec<&'a Table>, TranscriptError> {
    let Value::Table(t) = v else {
        return Err(format(n, format!("`{key}` is not a list")));
    };
    let mut out = Vec::with_capacity(t.len());
    for i in 1..=t.len() as i64 {
        match t.get(&Key::Int(i)) {
            Some(Value::Table(item)) => out.push(item),
            _ => return Err(format(n, format!("`{key}` is not a list of tables"))),
        }
    }
    Ok(out)
}

fn opt_str(t: &Table, key: &str, n: usize) -> Result<Option<String>, TranscriptError> {
    match t.get(&Key::str(key)) {
        None => Ok(None),
        Some(_) => get_str(t, key, n).map(Some),
    }
}

fn blob(t: &Table, key: &str, n: usize) -> Result<Vec<u8>, TranscriptError> {
    match t.get(&Key::str(key)) {
        Some(Value::Blob(b)) => Ok(b.clone()),
        _ => Err(format(n, format!("missing blob `{key}`"))),
    }
}

fn peer(t: &Table, key: &str, n: usize) -> Result<u32, TranscriptError> {
    match get_int(t, key, n)? {
        1 => Ok(1),
        other => Err(format(n, format!("`{key}` {other} is not a peer"))),
    }
}

fn event_of(t: &Table, n: usize) -> Result<Event, TranscriptError> {
    let kind = get_str(t, "kind", n)?;
    let e = match kind.as_str() {
        "hosting" => Event::Hosting {
            ticket: get_str(t, "ticket", n)?,
        },
        "connected" => Event::Connected {
            peer: peer(t, "peer", n)?,
        },
        "message" => Event::Message {
            from: peer(t, "from", n)?,
            data: blob(t, "data", n)?,
        },
        "disconnected" => Event::Disconnected {
            reason: Reason::parse(&get_str(t, "reason", n)?)
                .ok_or_else(|| format(n, "unknown disconnect reason"))?,
        },
        "failed" => Event::Failed {
            code: FailCode::parse(&get_str(t, "code", n)?)
                .ok_or_else(|| format(n, "unknown failure code"))?,
            detail: get_str(t, "detail", n)?,
        },
        "permission" => Event::Permission {
            granted: match t.get(&Key::str("granted")) {
                Some(Value::Bool(b)) => *b,
                _ => return Err(format(n, "missing boolean `granted`")),
            },
        },
        other => return Err(format(n, format!("unknown event kind {other:?}"))),
    };
    e.check().map_err(|why| format(n, why))?;
    Ok(e)
}

fn command_of(t: &Table, n: usize) -> Result<Command, TranscriptError> {
    let op = get_str(t, "op", n)?;
    let c = match op.as_str() {
        "host" => Command::Host,
        "leave" => Command::Leave,
        "join" => Command::Join {
            ticket: get_str(t, "ticket", n)?,
        },
        "send" => Command::Send {
            data: blob(t, "data", n)?,
        },
        other => return Err(format(n, format!("unknown op {other:?}"))),
    };
    c.check().map_err(|why| format(n, why))?;
    Ok(c)
}

/// A `{ at, events, io }` record, checked against the bounds and
/// against `frames_so_far`: the record must follow the run that ends at
/// its frame.
pub(super) fn frame_of(
    record: &Table,
    n: usize,
    frames_so_far: usize,
    last_at: Option<u64>,
) -> Result<FrameNet, TranscriptError> {
    let at = int_of(record.get(&Key::str("at")).expect("checked"), "at", n)?;
    if at < 1 || at as usize != frames_so_far {
        return Err(format(
            n,
            format!("network record for frame {at} after {frames_so_far} input frames"),
        ));
    }
    if last_at.is_some_and(|l| l >= at as u64) {
        return Err(format(
            n,
            format!("network record for frame {at} out of order"),
        ));
    }
    let events = match record.get(&Key::str("events")) {
        Some(v) => entries(v, "events", n)?,
        None => Vec::new(),
    };
    let commands = match record.get(&Key::str("io")) {
        Some(v) => entries(v, "io", n)?,
        None => Vec::new(),
    };
    if events.len() > MAX_BATCH || commands.len() > MAX_COMMANDS {
        return Err(TranscriptError::new(
            TranscriptError::SIZE,
            format!("line {}: network record over the batch bounds", n + 1),
        ));
    }
    let events = events
        .iter()
        .map(|t| event_of(t, n))
        .collect::<Result<Vec<_>, _>>()?;
    let commands = commands
        .iter()
        .map(|t| command_of(t, n))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(FrameNet {
        at: at as u64,
        events,
        commands,
    })
}

/// The header's version 2 fields.
pub(super) fn header_net(header: &Table, n: usize) -> Result<NetRecord, TranscriptError> {
    let services = match header.get(&Key::str("services")) {
        Some(Value::Table(t)) => {
            let mut out = Vec::new();
            for i in 1..=t.len() as i64 {
                match t.get(&Key::Int(i)) {
                    Some(Value::Str(s)) => out.push(
                        String::from_utf8(s.clone())
                            .map_err(|_| format(n, "service name is not UTF-8"))?,
                    ),
                    _ => return Err(format(n, "`services` is not a list of strings")),
                }
            }
            out
        }
        _ => return Err(format(n, "missing `services`")),
    };
    let invite = opt_str(header, "invite", n)?.filter(|s| !s.is_empty());
    let permitted = match header.get(&Key::str("net")) {
        Some(Value::Bool(b)) => *b,
        _ => return Err(format(n, "missing boolean `net`")),
    };
    Ok(NetRecord {
        services,
        env: NetEnv { permitted, invite },
        frames: Vec::new(),
    })
}

// ----- recording ----------------------------------------------------------

/// The networking side of a `Recorder`.
#[derive(Debug, Clone)]
pub(super) struct NetRecorder {
    pub(super) record: NetRecord,
    bytes: usize,
}

impl NetRecorder {
    pub(super) fn new(env: NetEnv) -> NetRecorder {
        NetRecorder {
            record: NetRecord::new(env),
            bytes: 0,
        }
    }

    /// Add a frame's traffic; `false` once the budget is spent.
    pub(super) fn record(&mut self, frame: FrameNet) -> bool {
        if frame.events.is_empty() && frame.commands.is_empty() {
            return true;
        }
        let cost = frame.cost();
        if self.bytes + cost > NET_BYTE_BUDGET {
            return false;
        }
        self.bytes += cost;
        self.record.frames.push(frame);
        true
    }
}

// ----- replay -------------------------------------------------------------

/// A guest wrapper that replays a version 2 transcript's network
/// records: each frame's recorded events are admitted before the inner
/// guest runs, and the commands it issues are compared with the
/// recorded ones afterwards. Never constructs a transport.
pub struct ReplayGuest {
    inner: Box<dyn Guest>,
    frames: VecDeque<FrameNet>,
}

impl ReplayGuest {
    pub fn new(inner: Box<dyn Guest>, record: &NetRecord) -> ReplayGuest {
        ReplayGuest {
            inner,
            frames: record.frames.iter().cloned().collect(),
        }
    }

    /// A factory wrapping `inner`'s guests with the records, when there
    /// are any.
    pub fn factory<F: crate::console::FactoryFn>(
        inner: F,
        record: Option<NetRecord>,
    ) -> impl crate::console::FactoryFn {
        move |text: &str, name: &str| {
            let guest = inner(text, name)?;
            Ok(match &record {
                Some(r) => Box::new(ReplayGuest::new(guest, r)) as Box<dyn Guest>,
                None => guest,
            })
        }
    }
}

fn divergence(frame: u64, expected: &[Command], got: &[Command]) -> Fault {
    let show = |cmds: &[Command]| -> String {
        if cmds.is_empty() {
            return "nothing".to_string();
        }
        cmds.iter()
            .map(|c| match c {
                Command::Send { data } => format!("send({} bytes)", data.len()),
                other => other.op().to_string(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    Fault::new(
        DIVERGENCE,
        "net",
        None,
        format!(
            "frame {frame}: the recording has {}, the cart issued {}",
            show(expected),
            show(got)
        ),
    )
}

impl Guest for ReplayGuest {
    fn step(&mut self, state: &mut DrawState, input: FrameInput, frame: u64) -> Result<(), Fault> {
        let recorded = match self.frames.front() {
            Some(f) if f.at == frame => self.frames.pop_front(),
            Some(f) if f.at < frame => {
                return Err(divergence(f.at, &f.commands, &[]));
            }
            _ => None,
        };
        if let (Some(net), Some(r)) = (state.net.as_mut(), &recorded) {
            net.admit(r.events.clone());
        }
        self.inner.step(state, input, frame)?;
        let Some(net) = state.net.as_ref() else {
            return Ok(());
        };
        let expected: &[Command] = recorded.as_ref().map(|r| &r.commands[..]).unwrap_or(&[]);
        if net.outbox != expected {
            return Err(divergence(frame, expected, &net.outbox));
        }
        Ok(())
    }

    fn state(&mut self, names: &[String]) -> Result<String, Fault> {
        self.inner.state(names)
    }
}
