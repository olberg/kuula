//! The bounded framed protocol between the broker and the worker
//!. One frame is a little-endian `u32` length, a tag
//! byte and the payload; the length covers tag plus payload and may not
//! exceed [`MAX_FRAME`]. Every field is length-checked before it is read
//! and a payload must be consumed exactly.

use std::io::{self, Read, Write};

use kuula_core::net::{
    Command, Event, FailCode, NetEnv, Reason, MAX_BATCH, MAX_COMMANDS, MAX_DATA, MAX_DETAIL,
    MAX_TICKET,
};
use kuula_core::save::{SLOT_BYTES, SLOT_COUNT};
use kuula_core::{
    ConsoleState, Fault, FrameInput, FrameProfile, Snapshot, SnapshotLimits, PALETTE_SIZE,
};
use kuula_host_headless::OwnedFrame;

/// Save slots as they travel: `(slot, bytes)`, at most one entry per
/// slot, each within the slot size. A `Load` carries the cart's slots as
/// the broker found them; a `Frame` carries what the cart wrote during
/// that frame, for the broker to persist.
pub type Saves = Vec<(u8, Vec<u8>)>;

/// Bytes the save slots may add to a message: every slot full, framed.
const SAVES_BYTES: usize = SLOT_COUNT as usize * (SLOT_BYTES + 5);

/// Largest frame either side accepts: a 1024x1024 frame with palette,
/// log and a frame of audio is about 1.1 MiB; a `Load` carrying a cart at
/// the snapshot limits plus every save slot full is the largest message,
/// so the cap is those limits plus per-entry framing (length prefixes and
/// a name of up to 255 bytes), the slots, and slack.
pub const MAX_FRAME: usize = {
    let limits = SnapshotLimits::DEFAULT;
    limits.max_total_bytes + limits.max_files * (8 + 255) + SAVES_BYTES + 1024
};

/// Largest screen side a `Frame` may claim.
const MAX_SIDE: u32 = 1024;

/// Audio samples a `Frame` may carry: one frame's worth.
const MAX_AUDIO_SAMPLES: usize = kuula_core::audio::SAMPLES_PER_FRAME;

/// String bounds shared by the encoder and the decoder. The encoder clips
/// to these on a character boundary, so a long cart error message still
/// arrives as a fault rather than being refused as a protocol violation.
const MAX_CODE_BYTES: usize = 64;
const MAX_FILE_BYTES: usize = 255;
const MAX_MESSAGE_BYTES: usize = 4096;

/// The longest prefix of `s` within `max` bytes that ends on a char boundary.
fn clip(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

const TAG_LOAD: u8 = 0x01;
const TAG_STEP: u8 = 0x02;
const TAG_STOP: u8 = 0x03;
const TAG_READY: u8 = 0x81;
const TAG_FRAME: u8 = 0x82;
const TAG_ERROR: u8 = 0x83;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum Message {
    /// Broker to worker: the whole cart, its save slots and the
    /// networking environment (permission and invite) of the run.
    Load {
        snapshot: Snapshot,
        saves: Saves,
        net: NetEnv,
    },
    /// Broker to worker: run one frame with this input and these
    /// network events, at most `MAX_BATCH` of them.
    Step {
        input: FrameInput,
        events: Vec<Event>,
    },
    /// Broker to worker: exit.
    Stop,
    /// Worker to broker: loaded, screen is this size.
    Ready { width: u32, height: u32 },
    /// Worker to broker: one frame, the slots the cart wrote in it, the
    /// network commands it issued (at most `MAX_COMMANDS`) and how many
    /// events its inbox has room for.
    Frame {
        frame: OwnedFrame,
        saves: Saves,
        commands: Vec<Command>,
        net_room: u32,
    },
    /// Worker to broker: something the worker could not do.
    Error { code: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoError {
    pub message: String,
}

impl ProtoError {
    fn new(m: impl Into<String>) -> ProtoError {
        ProtoError { message: m.into() }
    }
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProtoError {}

/// Errors from reading a frame: the pipe broke or the bytes were wrong.
#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    Proto(ProtoError),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "pipe: {e}"),
            ReadError::Proto(e) => write!(f, "protocol: {e}"),
        }
    }
}

impl std::error::Error for ReadError {}

// ----- encoding -----------------------------------------------------------

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    fn saves(&mut self, saves: &Saves) {
        self.u32(saves.len() as u32);
        for (slot, bytes) in saves {
            self.u8(*slot);
            self.bytes(bytes);
        }
    }
    fn events(&mut self, events: &[Event]) {
        let events = &events[..events.len().min(MAX_BATCH)];
        self.u32(events.len() as u32);
        for e in events {
            match e {
                Event::Hosting { ticket } => {
                    self.u8(EV_HOSTING);
                    self.str(clip(ticket, MAX_TICKET));
                }
                Event::Connected { peer } => {
                    self.u8(EV_CONNECTED);
                    self.u32(*peer);
                }
                Event::Message { from, data } => {
                    self.u8(EV_MESSAGE);
                    self.u32(*from);
                    self.bytes(&data[..data.len().min(MAX_DATA)]);
                }
                Event::Disconnected { reason } => {
                    self.u8(EV_DISCONNECTED);
                    self.str(reason.as_str());
                }
                Event::Failed { code, detail } => {
                    self.u8(EV_FAILED);
                    self.str(code.as_str());
                    self.str(clip(detail, MAX_DETAIL));
                }
                Event::Permission { granted } => {
                    self.u8(EV_PERMISSION);
                    self.u8(*granted as u8);
                }
            }
        }
    }
    fn commands(&mut self, commands: &[Command]) {
        let commands = &commands[..commands.len().min(MAX_COMMANDS)];
        self.u32(commands.len() as u32);
        for c in commands {
            match c {
                Command::Host => self.u8(CMD_HOST),
                Command::Join { ticket } => {
                    self.u8(CMD_JOIN);
                    self.str(clip(ticket, MAX_TICKET));
                }
                Command::Send { data } => {
                    self.u8(CMD_SEND);
                    self.bytes(&data[..data.len().min(MAX_DATA)]);
                }
                Command::Leave => self.u8(CMD_LEAVE),
            }
        }
    }
}

const EV_HOSTING: u8 = 1;
const EV_CONNECTED: u8 = 2;
const EV_MESSAGE: u8 = 3;
const EV_DISCONNECTED: u8 = 4;
const EV_FAILED: u8 = 5;
const EV_PERMISSION: u8 = 6;
const CMD_HOST: u8 = 1;
const CMD_JOIN: u8 = 2;
const CMD_SEND: u8 = 3;
const CMD_LEAVE: u8 = 4;

/// Encode a message as one frame, ready to write.
pub fn encode(msg: &Message) -> Vec<u8> {
    let mut w = Writer(Vec::new());
    w.u32(0); // length, patched below
    match msg {
        Message::Load {
            snapshot,
            saves,
            net,
        } => {
            w.u8(TAG_LOAD);
            w.u32(snapshot.len() as u32);
            for (name, data) in snapshot.entries() {
                w.str(name);
                w.bytes(data);
            }
            w.saves(saves);
            w.u8(net.permitted as u8);
            w.str(clip(net.invite.as_deref().unwrap_or(""), MAX_TICKET));
        }
        Message::Step { input, events } => {
            w.u8(TAG_STEP);
            w.u8(input.buttons);
            w.events(events);
        }
        Message::Stop => w.u8(TAG_STOP),
        Message::Ready { width, height } => {
            w.u8(TAG_READY);
            w.u32(*width);
            w.u32(*height);
        }
        Message::Frame {
            frame: f,
            saves,
            commands,
            net_room,
        } => {
            w.u8(TAG_FRAME);
            w.u64(f.frame);
            w.u32(f.width);
            w.u32(f.height);
            w.0.extend_from_slice(&f.pixels);
            for rgb in &f.palette {
                w.0.extend_from_slice(rgb);
            }
            match &f.state {
                ConsoleState::Running => w.u8(0),
                ConsoleState::Faulted(fault) => {
                    w.u8(1);
                    w.str(clip(&fault.code, MAX_CODE_BYTES));
                    w.str(clip(&fault.file, MAX_FILE_BYTES));
                    w.u32(fault.line.unwrap_or(0));
                    w.str(clip(&fault.message, MAX_MESSAGE_BYTES));
                }
            }
            w.u32(f.log.len() as u32);
            for line in &f.log {
                w.str(line);
            }
            w.u64(f.profile.budget);
            w.u64(f.profile.lua_mem);
            for c in f.profile.cycles {
                w.u64(c);
            }
            let audio = &f.audio[..f.audio.len().min(MAX_AUDIO_SAMPLES)];
            w.u32(audio.len() as u32);
            for s in audio {
                w.0.extend_from_slice(&s.to_le_bytes());
            }
            w.saves(saves);
            w.commands(commands);
            w.u32(*net_room);
        }
        Message::Error { code, message } => {
            w.u8(TAG_ERROR);
            w.str(clip(code, MAX_CODE_BYTES));
            w.str(clip(message, MAX_MESSAGE_BYTES));
        }
    }
    let len = (w.0.len() - 4) as u32;
    w.0[..4].copy_from_slice(&len.to_le_bytes());
    w.0
}

// ----- decoding -----------------------------------------------------------

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtoError> {
        if self.buf.len() - self.pos < n {
            return Err(ProtoError::new(format!(
                "payload short: wanted {n} bytes at {}, have {}",
                self.pos,
                self.buf.len() - self.pos
            )));
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, ProtoError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, ProtoError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, ProtoError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self, max: usize) -> Result<&'a [u8], ProtoError> {
        let n = self.u32()? as usize;
        if n > max {
            return Err(ProtoError::new(format!("field of {n} bytes exceeds {max}")));
        }
        self.take(n)
    }
    fn str(&mut self, max: usize) -> Result<String, ProtoError> {
        let b = self.bytes(max)?;
        String::from_utf8(b.to_vec()).map_err(|_| ProtoError::new("string is not UTF-8"))
    }
    /// Save slots: at most one entry per slot, each within the slot size.
    fn saves(&mut self) -> Result<Saves, ProtoError> {
        let count = self.u32()? as usize;
        if count > SLOT_COUNT as usize {
            return Err(ProtoError::new(format!(
                "{count} save entries, more than {SLOT_COUNT} slots"
            )));
        }
        let mut out: Saves = Vec::with_capacity(count);
        for _ in 0..count {
            let slot = self.u8()?;
            if slot >= SLOT_COUNT || out.iter().any(|(s, _)| *s == slot) {
                return Err(ProtoError::new(format!("save slot {slot} refused")));
            }
            let bytes = self.bytes(SLOT_BYTES)?;
            out.push((slot, bytes.to_vec()));
        }
        Ok(out)
    }
    /// A peer number: only 1 exists.
    fn peer(&mut self) -> Result<u32, ProtoError> {
        match self.u32()? {
            1 => Ok(1),
            other => Err(ProtoError::new(format!("peer {other} refused"))),
        }
    }
    /// Network events, each checked against the core's bounds.
    fn events(&mut self) -> Result<Vec<Event>, ProtoError> {
        let count = self.u32()? as usize;
        if count > MAX_BATCH {
            return Err(ProtoError::new(format!(
                "{count} events, more than {MAX_BATCH}"
            )));
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let e = match self.u8()? {
                EV_HOSTING => Event::Hosting {
                    ticket: self.str(MAX_TICKET)?,
                },
                EV_CONNECTED => Event::Connected { peer: self.peer()? },
                EV_MESSAGE => Event::Message {
                    from: self.peer()?,
                    data: self.bytes(MAX_DATA)?.to_vec(),
                },
                EV_DISCONNECTED => Event::Disconnected {
                    reason: Reason::parse(&self.str(16)?)
                        .ok_or_else(|| ProtoError::new("unknown disconnect reason"))?,
                },
                EV_FAILED => Event::Failed {
                    code: FailCode::parse(&self.str(MAX_CODE_BYTES)?)
                        .ok_or_else(|| ProtoError::new("unknown failure code"))?,
                    detail: self.str(MAX_DETAIL)?,
                },
                EV_PERMISSION => Event::Permission {
                    granted: match self.u8()? {
                        0 => false,
                        1 => true,
                        other => return Err(ProtoError::new(format!("bad flag {other}"))),
                    },
                },
                other => return Err(ProtoError::new(format!("unknown event tag {other}"))),
            };
            e.check().map_err(ProtoError::new)?;
            out.push(e);
        }
        Ok(out)
    }
    /// Network commands, each checked against the core's bounds.
    fn commands(&mut self) -> Result<Vec<Command>, ProtoError> {
        let count = self.u32()? as usize;
        if count > MAX_COMMANDS {
            return Err(ProtoError::new(format!(
                "{count} commands, more than {MAX_COMMANDS}"
            )));
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let c = match self.u8()? {
                CMD_HOST => Command::Host,
                CMD_JOIN => Command::Join {
                    ticket: self.str(MAX_TICKET)?,
                },
                CMD_SEND => Command::Send {
                    data: self.bytes(MAX_DATA)?.to_vec(),
                },
                CMD_LEAVE => Command::Leave,
                other => return Err(ProtoError::new(format!("unknown command tag {other}"))),
            };
            c.check().map_err(ProtoError::new)?;
            out.push(c);
        }
        Ok(out)
    }
    fn done(&self) -> Result<(), ProtoError> {
        if self.pos == self.buf.len() {
            Ok(())
        } else {
            Err(ProtoError::new(format!(
                "{} trailing bytes in payload",
                self.buf.len() - self.pos
            )))
        }
    }
}

/// Decode one frame's body (tag plus payload).
pub fn decode(body: &[u8]) -> Result<Message, ProtoError> {
    let mut r = Reader { buf: body, pos: 0 };
    let tag = r.u8()?;
    let msg = match tag {
        TAG_LOAD => {
            let count = r.u32()? as usize;
            let limits = SnapshotLimits::default();
            if count > limits.max_files {
                return Err(ProtoError::new("too many snapshot entries"));
            }
            let mut entries = Vec::with_capacity(count.min(1024));
            for _ in 0..count {
                let name = r.str(255)?;
                let data = r.bytes(limits.max_file_bytes)?;
                entries.push((name, data.to_vec()));
            }
            let snapshot = Snapshot::from_entries(entries, limits)
                .map_err(|e| ProtoError::new(format!("snapshot: {e}")))?;
            let saves = r.saves()?;
            let permitted = match r.u8()? {
                0 => false,
                1 => true,
                other => return Err(ProtoError::new(format!("bad flag {other}"))),
            };
            let invite = r.str(MAX_TICKET)?;
            Message::Load {
                snapshot,
                saves,
                net: NetEnv {
                    permitted,
                    invite: (!invite.is_empty()).then_some(invite),
                },
            }
        }
        TAG_STEP => {
            let input = FrameInput::new(r.u8()?);
            let events = r.events()?;
            Message::Step { input, events }
        }
        TAG_STOP => Message::Stop,
        TAG_READY => Message::Ready {
            width: r.u32()?,
            height: r.u32()?,
        },
        TAG_FRAME => {
            let frame = r.u64()?;
            let width = r.u32()?;
            let height = r.u32()?;
            if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
                return Err(ProtoError::new(format!(
                    "frame size {width}x{height} refused"
                )));
            }
            let pixels = r.take(width as usize * height as usize)?.to_vec();
            let mut palette = [[0u8; 3]; PALETTE_SIZE];
            for rgb in palette.iter_mut() {
                rgb.copy_from_slice(r.take(3)?);
            }
            let state = match r.u8()? {
                0 => ConsoleState::Running,
                1 => {
                    let code = r.str(MAX_CODE_BYTES)?;
                    let file = r.str(MAX_FILE_BYTES)?;
                    let line = r.u32()?;
                    let message = r.str(MAX_MESSAGE_BYTES)?;
                    ConsoleState::Faulted(Fault::new(
                        &code,
                        &file,
                        if line == 0 { None } else { Some(line) },
                        message,
                    ))
                }
                other => return Err(ProtoError::new(format!("bad state byte {other}"))),
            };
            let count = r.u32()? as usize;
            if count > kuula_core::draw::MAX_LOG_LINES + 1 {
                return Err(ProtoError::new("too many log lines"));
            }
            let mut log = Vec::with_capacity(count);
            for _ in 0..count {
                log.push(r.str(kuula_core::draw::MAX_LOG_LINE_BYTES + 8)?);
            }
            let mut profile = FrameProfile {
                budget: r.u64()?,
                lua_mem: r.u64()?,
                ..FrameProfile::default()
            };
            for c in profile.cycles.iter_mut() {
                *c = r.u64()?;
            }
            let samples = r.u32()? as usize;
            if samples > MAX_AUDIO_SAMPLES {
                return Err(ProtoError::new("too many audio samples"));
            }
            let audio = r
                .take(samples * 2)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b))
                .collect();
            let saves = r.saves()?;
            let commands = r.commands()?;
            let net_room = r.u32()?;
            if net_room as usize > MAX_BATCH {
                return Err(ProtoError::new(format!("net room {net_room} refused")));
            }
            Message::Frame {
                frame: OwnedFrame {
                    frame,
                    width,
                    height,
                    pixels,
                    palette,
                    log,
                    state,
                    profile,
                    audio,
                },
                saves,
                commands,
                net_room,
            }
        }
        TAG_ERROR => Message::Error {
            code: r.str(MAX_CODE_BYTES)?,
            message: r.str(MAX_MESSAGE_BYTES)?,
        },
        other => return Err(ProtoError::new(format!("unknown tag {other:#04x}"))),
    };
    r.done()?;
    Ok(msg)
}

/// Read one frame. `Ok(None)` on a clean end of stream before a frame.
pub fn read_frame(reader: &mut impl Read) -> Result<Option<Message>, ReadError> {
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(ReadError::Io(e)),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(ReadError::Proto(ProtoError::new(format!(
            "frame length {len} refused (max {MAX_FRAME})"
        ))));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).map_err(ReadError::Io)?;
    decode(&body).map(Some).map_err(ReadError::Proto)
}

pub fn write_frame(writer: &mut impl Write, msg: &Message) -> io::Result<()> {
    writer.write_all(&encode(msg))?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(msg: Message) {
        let bytes = encode(&msg);
        let mut cursor = std::io::Cursor::new(bytes);
        let back = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(back, msg);
        assert_eq!(cursor.position() as usize, cursor.get_ref().len());
    }

    #[test]
    fn every_message_round_trips() {
        let snap = Snapshot::from_entries(
            [("main.lua", b"x".to_vec()), ("gfx/a.png", vec![1, 2, 3])],
            SnapshotLimits::default(),
        )
        .unwrap();
        round_trip(Message::Load {
            snapshot: snap,
            saves: vec![(0, b"a".to_vec()), (7, vec![0; SLOT_BYTES])],
            net: NetEnv {
                permitted: true,
                invite: Some("endpointabc".into()),
            },
        });
        round_trip(Message::Load {
            snapshot: Snapshot::empty(),
            saves: Vec::new(),
            net: NetEnv::default(),
        });
        round_trip(Message::Step {
            input: FrameInput::new(0b101),
            events: Vec::new(),
        });
        round_trip(Message::Step {
            input: FrameInput::NONE,
            events: vec![
                Event::Hosting {
                    ticket: "t".repeat(MAX_TICKET),
                },
                Event::Connected { peer: 1 },
                Event::Message {
                    from: 1,
                    data: vec![0, 255, 7],
                },
                Event::Message {
                    from: 1,
                    data: vec![9; MAX_DATA],
                },
                Event::Disconnected {
                    reason: Reason::Lost,
                },
                Event::Failed {
                    code: FailCode::QueueFull,
                    detail: "full".into(),
                },
                Event::Permission { granted: false },
            ],
        });
        round_trip(Message::Stop);
        round_trip(Message::Ready {
            width: 320,
            height: 240,
        });
        let mut palette = [[0u8; 3]; PALETTE_SIZE];
        palette[3] = [1, 2, 3];
        let plain = |frame: OwnedFrame| Message::Frame {
            frame,
            saves: Vec::new(),
            commands: Vec::new(),
            net_room: 0,
        };
        round_trip(Message::Frame {
            frame: OwnedFrame {
                frame: 7,
                width: 2,
                height: 2,
                pixels: vec![1, 2, 3, 4],
                palette,
                profile: FrameProfile::default(),
                audio: vec![1, -2, 32767, -32768],
                log: vec!["a".into(), "".into()],
                state: ConsoleState::Running,
            },
            saves: vec![(3, vec![9; 10])],
            commands: vec![
                Command::Host,
                Command::Join {
                    ticket: "endpointx".into(),
                },
                Command::Send { data: vec![1, 2] },
                Command::Leave,
            ],
            net_room: MAX_BATCH as u32,
        });
        round_trip(plain(OwnedFrame {
            frame: 8,
            width: 1,
            height: 1,
            pixels: vec![0],
            palette,
            profile: FrameProfile::default(),
            log: vec![],
            audio: vec![],
            state: ConsoleState::Faulted(Fault::new("runtime_error", "main.lua", Some(3), "boom")),
        }));
        round_trip(plain(OwnedFrame {
            frame: 9,
            width: 1,
            height: 1,
            pixels: vec![0],
            palette,
            profile: FrameProfile::default(),
            log: vec![],
            audio: vec![],
            state: ConsoleState::Faulted(Fault::new("cart_read_error", "main.lua", None, "")),
        }));
        round_trip(Message::Error {
            code: "worker_error".into(),
            message: "no".into(),
        });
        // The profile travels with the frame.
        round_trip(plain(OwnedFrame {
            frame: 10,
            width: 1,
            height: 1,
            pixels: vec![0],
            palette,
            log: vec![],
            state: ConsoleState::Running,
            profile: FrameProfile {
                cycles: [1, 2, 3, 4, 5, 6, u64::MAX],
                budget: 279_620,
                lua_mem: 12345,
            },
            audio: vec![0; MAX_AUDIO_SAMPLES],
        }));
    }

    #[test]
    fn long_fault_strings_are_clipped_not_refused() {
        // A cart can raise any message; it must still arrive as a fault.
        let long = "ä".repeat(3000); // 6000 bytes, clipped on a char boundary
        let msg = Message::Frame {
            frame: OwnedFrame {
                frame: 1,
                width: 1,
                height: 1,
                pixels: vec![0],
                palette: [[0; 3]; PALETTE_SIZE],
                profile: FrameProfile::default(),
                log: vec![],
                audio: vec![],
                state: ConsoleState::Faulted(Fault::new(
                    "runtime_error",
                    &"f".repeat(300),
                    Some(3),
                    &long,
                )),
            },
            saves: Vec::new(),
            commands: Vec::new(),
            net_room: 0,
        };
        let bytes = encode(&msg);
        let back = decode(&bytes[4..]).unwrap();
        let Message::Frame { frame: f, .. } = back else {
            panic!("not a frame")
        };
        let ConsoleState::Faulted(fault) = f.state else {
            panic!("not faulted")
        };
        assert_eq!(fault.message.len(), MAX_MESSAGE_BYTES);
        assert_eq!(fault.file.len(), MAX_FILE_BYTES);
        assert!(long.starts_with(&fault.message));
        round_trip(Message::Error {
            code: "worker_error".into(),
            message: "x".repeat(MAX_MESSAGE_BYTES),
        });
    }

    #[test]
    fn a_maximal_snapshot_fits_in_one_frame() {
        let limits = SnapshotLimits::DEFAULT;
        let framing = 1 + 4 + limits.max_files * (8 + 255);
        assert!(limits.max_total_bytes + framing + SAVES_BYTES <= MAX_FRAME);
    }

    #[test]
    fn bad_saves_are_rejected() {
        // Nine entries, a repeated slot, a slot out of range, an oversized
        // slot: each is a protocol error, not a panic.
        let base = encode(&Message::Load {
            snapshot: Snapshot::empty(),
            saves: Vec::new(),
            net: NetEnv::default(),
        });
        // Drop the empty saves count and the net env (1 + 4 bytes).
        let with = |tail: &[u8]| {
            let mut body = base[4..base.len() - 4 - 5].to_vec();
            body.extend_from_slice(tail);
            body.push(0);
            body.extend_from_slice(&0u32.to_le_bytes());
            decode(&body)
        };
        let mut nine = 9u32.to_le_bytes().to_vec();
        for s in 0..9u8 {
            nine.push(s);
            nine.extend_from_slice(&0u32.to_le_bytes());
        }
        assert!(with(&nine).is_err());
        let mut twice = 2u32.to_le_bytes().to_vec();
        for _ in 0..2 {
            twice.push(1);
            twice.extend_from_slice(&0u32.to_le_bytes());
        }
        assert!(with(&twice).is_err());
        let mut high = 1u32.to_le_bytes().to_vec();
        high.push(8);
        high.extend_from_slice(&0u32.to_le_bytes());
        assert!(with(&high).is_err());
        let mut big = 1u32.to_le_bytes().to_vec();
        big.push(0);
        big.extend_from_slice(&(SLOT_BYTES as u32 + 1).to_le_bytes());
        big.extend(vec![0u8; SLOT_BYTES + 1]);
        assert!(with(&big).is_err());
    }

    #[test]
    fn bad_network_fields_are_rejected() {
        let step = |events: &[u8]| {
            let mut body = vec![TAG_STEP, 0];
            body.extend_from_slice(events);
            decode(&body)
        };
        // Too many events.
        assert!(step(&(MAX_BATCH as u32 + 1).to_le_bytes()).is_err());
        // Peer 0.
        let mut ev = 1u32.to_le_bytes().to_vec();
        ev.push(EV_CONNECTED);
        ev.extend_from_slice(&0u32.to_le_bytes());
        assert!(step(&ev).is_err());
        // Empty data.
        let mut ev = 1u32.to_le_bytes().to_vec();
        ev.push(EV_MESSAGE);
        ev.extend_from_slice(&1u32.to_le_bytes());
        ev.extend_from_slice(&0u32.to_le_bytes());
        assert!(step(&ev).is_err());
        // Oversized data is refused from the length alone.
        let mut ev = 1u32.to_le_bytes().to_vec();
        ev.push(EV_MESSAGE);
        ev.extend_from_slice(&1u32.to_le_bytes());
        ev.extend_from_slice(&(MAX_DATA as u32 + 1).to_le_bytes());
        ev.extend(vec![0u8; MAX_DATA + 1]);
        assert!(step(&ev).is_err());
        // An unknown failure code.
        let mut ev = 1u32.to_le_bytes().to_vec();
        ev.push(EV_FAILED);
        ev.extend_from_slice(&5u32.to_le_bytes());
        ev.extend_from_slice(b"net_x");
        ev.extend_from_slice(&0u32.to_le_bytes());
        assert!(step(&ev).is_err());
        // An unknown tag.
        let mut ev = 1u32.to_le_bytes().to_vec();
        ev.push(99);
        assert!(step(&ev).is_err());
        // A frame with too many commands or too much room.
        let base = encode(&Message::Frame {
            frame: OwnedFrame {
                frame: 1,
                width: 1,
                height: 1,
                pixels: vec![0],
                palette: [[0; 3]; PALETTE_SIZE],
                profile: FrameProfile::default(),
                log: vec![],
                audio: vec![],
                state: ConsoleState::Running,
            },
            saves: Vec::new(),
            commands: Vec::new(),
            net_room: 0,
        });
        let body = &base[4..];
        let mut many = body[..body.len() - 8].to_vec();
        many.extend_from_slice(&(MAX_COMMANDS as u32 + 1).to_le_bytes());
        many.extend_from_slice(&0u32.to_le_bytes());
        assert!(decode(&many).is_err());
        let mut room = body[..body.len() - 4].to_vec();
        room.extend_from_slice(&(MAX_BATCH as u32 + 1).to_le_bytes());
        assert!(decode(&room).is_err());
    }

    #[test]
    fn bad_frames_are_rejected() {
        let mut oversized = (MAX_FRAME as u32 + 1).to_le_bytes().to_vec();
        oversized.push(TAG_STOP);
        assert!(matches!(
            read_frame(&mut std::io::Cursor::new(oversized)),
            Err(ReadError::Proto(_))
        ));
        let zero = 0u32.to_le_bytes().to_vec();
        assert!(matches!(
            read_frame(&mut std::io::Cursor::new(zero)),
            Err(ReadError::Proto(_))
        ));
        assert!(decode(&[0x7f]).is_err(), "unknown tag");
        assert!(decode(&[TAG_STOP, 0]).is_err(), "trailing byte");
        assert!(decode(&[TAG_STEP]).is_err(), "short payload");
        assert!(decode(&[TAG_STEP, 0]).is_err(), "a step without its events");
        // A frame claiming a huge size fails before allocating pixels.
        let mut body = vec![TAG_FRAME];
        body.extend_from_slice(&1u64.to_le_bytes());
        body.extend_from_slice(&5000u32.to_le_bytes());
        body.extend_from_slice(&5000u32.to_le_bytes());
        assert!(decode(&body).unwrap_err().message.contains("refused"));
        // A truncated frame body is a pipe error, not a panic.
        let mut cut = encode(&Message::Ready {
            width: 1,
            height: 1,
        });
        cut.truncate(cut.len() - 2);
        assert!(matches!(
            read_frame(&mut std::io::Cursor::new(cut)),
            Err(ReadError::Io(_))
        ));
        assert!(read_frame(&mut std::io::Cursor::new(Vec::new()))
            .unwrap()
            .is_none());
    }
}
