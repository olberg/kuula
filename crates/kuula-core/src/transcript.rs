//! Replay transcripts (format version 1).
//!
//! A transcript is everything a cart could have observed: the initial
//! environment (cart content digest, runtime version, seed, the save
//! slots as they were) and, per frame, the [`FrameInput`] the cart saw.
//! Nothing else reaches a cart today: it has no clock, its loads come
//! from the immutable snapshot, and its saves go to a store whose results
//! do not depend on the host ([`crate::save::WriteThroughStore`]). The
//! per-frame messages, connection changes and I/O outcomes the
//! architecture reserves are keys later versions may add to a record;
//! version 1 writes none and refuses a record that carries any.
//!
//! The file (`.kr`) is line-oriented so it streams and stays within the
//! canonical codec's per-document limits:
//!
//! ```text
//! kuula-transcript 1
//! { cart = "0x...", frames = N, runtime = "0.0.1", seed = 0, version = 1 }
//! { data = blob"...", part = 1, parts = 3, slot = 0 }      -- save chunks
//! { buttons = 8, frames = 30 }                              -- input runs
//! ```
//!
//! Every line after the first is one canonical document. Save slots are
//! split into chunks so a full slot fits a document; inputs are
//! run-length encoded, one record per run of identical inputs. The
//! decoder bounds the file, every line, the frame count and the chunks
//! before it allocates.

use std::cell::RefCell;
use std::rc::Rc;

use crate::codec::{self, Key, Table, Value};
use crate::console::{FactoryFn, Guest};
use crate::draw::DrawState;
use crate::fault::Fault;
use crate::input::{FrameInput, CART_BUTTONS};
use crate::save::{check_size, check_slot, SLOT_COUNT};
use crate::snapshot::Snapshot;

pub const FORMAT_VERSION: i64 = 1;
pub const MAGIC: &str = "kuula-transcript";
/// The file extension a transcript is written with.
pub const EXTENSION: &str = "kr";
/// Most frames one transcript may hold: the input-script limit.
pub const MAX_FRAMES: u64 = 1_000_000;
/// Longest line, the codec's document limit.
pub const MAX_LINE_BYTES: usize = codec::MAX_ENCODED;
/// Largest file the decoder reads.
pub const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
/// Raw bytes per save chunk: base64 of this is under the line limit.
const SAVE_CHUNK: usize = 96 * 1024;

/// The initial environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The runtime that recorded, `CARGO_PKG_VERSION` of the core.
    pub runtime: String,
    /// [`cart_digest`] of the snapshot.
    pub cart: String,
    /// The guest's fixed RNG seed.
    pub seed: i64,
    /// The save slots as the cart found them, in slot order.
    pub saves: Vec<(u8, Vec<u8>)>,
}

impl Header {
    pub fn new(snapshot: &Snapshot, seed: i64, saves: Vec<(u8, Vec<u8>)>) -> Header {
        Header {
            runtime: env!("CARGO_PKG_VERSION").to_string(),
            cart: cart_digest(snapshot),
            seed,
            saves,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub header: Header,
    pub inputs: Vec<FrameInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptError {
    pub code: &'static str,
    pub message: String,
}

impl TranscriptError {
    /// Not a transcript, or a malformed or truncated one.
    pub const FORMAT: &'static str = "transcript_format";
    /// A format version this runtime does not read.
    pub const VERSION: &'static str = "transcript_version";
    /// Over a bound.
    pub const SIZE: &'static str = "transcript_size";
    /// The cart being replayed is not the one recorded.
    pub const CART: &'static str = "transcript_cart_mismatch";

    fn new(code: &'static str, message: impl Into<String>) -> TranscriptError {
        TranscriptError {
            code,
            message: message.into(),
        }
    }

    fn format(message: impl Into<String>) -> TranscriptError {
        TranscriptError::new(TranscriptError::FORMAT, message)
    }
}

impl std::fmt::Display for TranscriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TranscriptError {}

/// FNV-1a 64 over the snapshot's entries in name order: each name, a
/// zero byte, the length as 8 little-endian bytes, then the bytes. `0x`
/// and 16 hex digits, like the conformance hash.
pub fn cart_digest(snapshot: &Snapshot) -> String {
    let mut entries: Vec<(&str, &[u8])> = snapshot.entries().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let mut h: u64 = 0xcbf29ce484222325;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    for (name, data) in entries {
        feed(name.as_bytes());
        feed(&[0]);
        feed(&(data.len() as u64).to_le_bytes());
        feed(data);
    }
    format!("{h:#018x}")
}

// ----- encoding -----------------------------------------------------------

fn table(entries: Vec<(&str, Value)>) -> Result<Value, TranscriptError> {
    let mut t = Table::new();
    for (k, v) in entries {
        t.insert(Key::str(k), v)
            .map_err(|e| TranscriptError::new(TranscriptError::SIZE, e.to_string()))?;
    }
    Ok(Value::Table(t))
}

fn line(value: &Value) -> Result<String, TranscriptError> {
    codec::encode(value).map_err(|e| TranscriptError::new(TranscriptError::SIZE, e.to_string()))
}

impl Transcript {
    pub fn encode(&self) -> Result<String, TranscriptError> {
        if self.inputs.len() as u64 > MAX_FRAMES {
            return Err(TranscriptError::new(
                TranscriptError::SIZE,
                format!("more than {MAX_FRAMES} frames"),
            ));
        }
        let mut out = format!("{MAGIC} {FORMAT_VERSION}\n");
        let header = table(vec![
            ("cart", Value::str(&self.header.cart)),
            ("frames", Value::Int(self.inputs.len() as i64)),
            ("runtime", Value::str(&self.header.runtime)),
            ("seed", Value::Int(self.header.seed)),
            ("version", Value::Int(FORMAT_VERSION)),
        ])?;
        out.push_str(&line(&header)?);
        out.push('\n');
        for (slot, bytes) in &self.header.saves {
            check_slot(*slot)
                .and_then(|()| check_size(bytes.len()))
                .map_err(|e| TranscriptError::new(TranscriptError::SIZE, e.to_string()))?;
            let parts = bytes.len().div_ceil(SAVE_CHUNK).max(1);
            for (i, chunk) in bytes
                .chunks(SAVE_CHUNK)
                .chain(std::iter::once(&[][..]).take(usize::from(bytes.is_empty())))
                .enumerate()
            {
                let record = table(vec![
                    ("data", Value::Blob(chunk.to_vec())),
                    ("part", Value::Int(i as i64 + 1)),
                    ("parts", Value::Int(parts as i64)),
                    ("slot", Value::Int(*slot as i64)),
                ])?;
                out.push_str(&line(&record)?);
                out.push('\n');
            }
        }
        let mut i = 0;
        while i < self.inputs.len() {
            let buttons = self.inputs[i].buttons;
            let mut n = 1;
            while i + n < self.inputs.len() && self.inputs[i + n].buttons == buttons {
                n += 1;
            }
            let record = table(vec![
                ("buttons", Value::Int(buttons as i64)),
                ("frames", Value::Int(n as i64)),
            ])?;
            out.push_str(&line(&record)?);
            out.push('\n');
            i += n;
        }
        Ok(out)
    }

    // ----- decoding -------------------------------------------------------

    pub fn decode(text: &str) -> Result<Transcript, TranscriptError> {
        if text.len() > MAX_FILE_BYTES {
            return Err(TranscriptError::new(
                TranscriptError::SIZE,
                format!("transcript is over {MAX_FILE_BYTES} bytes"),
            ));
        }
        let mut lines = text.lines().enumerate();
        let (_, first) = lines
            .next()
            .ok_or_else(|| TranscriptError::format("empty file"))?;
        let mut words = first.split(' ');
        if words.next() != Some(MAGIC) {
            return Err(TranscriptError::format(format!(
                "not a transcript: first line is not `{MAGIC} <version>`"
            )));
        }
        let version: i64 = words
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| TranscriptError::format("missing format version"))?;
        if version != FORMAT_VERSION {
            return Err(TranscriptError::new(
                TranscriptError::VERSION,
                format!("format version {version}; this runtime reads {FORMAT_VERSION}"),
            ));
        }
        let (n, header_line) = lines
            .next()
            .ok_or_else(|| TranscriptError::format("missing header"))?;
        let header = parse_line(n, header_line)?;
        let frames = get_int(&header, "frames", n)?;
        if !(0..=MAX_FRAMES as i64).contains(&frames) {
            return Err(TranscriptError::new(
                TranscriptError::SIZE,
                format!("header claims {frames} frames; the limit is {MAX_FRAMES}"),
            ));
        }
        if get_int(&header, "version", n)? != FORMAT_VERSION {
            return Err(TranscriptError::new(
                TranscriptError::VERSION,
                "header version disagrees with the first line",
            ));
        }
        let mut out = Transcript {
            header: Header {
                runtime: get_str(&header, "runtime", n)?,
                cart: get_str(&header, "cart", n)?,
                seed: get_int(&header, "seed", n)?,
                saves: Vec::new(),
            },
            inputs: Vec::with_capacity((frames as usize).min(65_536)),
        };
        // Save chunks in progress: slot, expected part count, bytes so far.
        let mut chunking: Option<(u8, i64, i64, Vec<u8>)> = None;
        for (n, text) in lines {
            if text.is_empty() {
                continue;
            }
            let record = parse_line(n, text)?;
            if let Some(v) = record.get(&Key::str("slot")) {
                let slot = int_of(v, "slot", n)?;
                let part = get_int(&record, "part", n)?;
                let parts = get_int(&record, "parts", n)?;
                let data = match record.get(&Key::str("data")) {
                    Some(Value::Blob(b)) => b.clone(),
                    _ => {
                        return Err(TranscriptError::format(format!(
                            "line {}: save chunk without blob data",
                            n + 1
                        )))
                    }
                };
                check_slot(u8::try_from(slot).unwrap_or(u8::MAX))
                    .map_err(|e| TranscriptError::format(format!("line {}: {e}", n + 1)))?;
                let slot = slot as u8;
                if parts < 1 || part < 1 || part > parts {
                    return Err(TranscriptError::format(format!(
                        "line {}: chunk {part} of {parts}",
                        n + 1
                    )));
                }
                let (cur_slot, cur_parts, cur_part, mut bytes) = match chunking.take() {
                    Some(c) => c,
                    None => (slot, parts, 0, Vec::new()),
                };
                if cur_slot != slot || cur_parts != parts || cur_part + 1 != part {
                    return Err(TranscriptError::format(format!(
                        "line {}: save chunks out of order",
                        n + 1
                    )));
                }
                bytes.extend_from_slice(&data);
                check_size(bytes.len())
                    .map_err(|e| TranscriptError::new(TranscriptError::SIZE, e.to_string()))?;
                if part == parts {
                    if out.header.saves.iter().any(|(s, _)| *s == slot) {
                        return Err(TranscriptError::format(format!(
                            "line {}: slot {slot} twice",
                            n + 1
                        )));
                    }
                    if !out.inputs.is_empty() {
                        return Err(TranscriptError::format(format!(
                            "line {}: save chunk after inputs",
                            n + 1
                        )));
                    }
                    out.header.saves.push((slot, bytes));
                } else {
                    chunking = Some((slot, parts, part, bytes));
                }
                continue;
            }
            if chunking.is_some() {
                return Err(TranscriptError::format(format!(
                    "line {}: save chunk sequence interrupted",
                    n + 1
                )));
            }
            let buttons = get_int(&record, "buttons", n)?;
            let run = get_int(&record, "frames", n)?;
            if !(0..=CART_BUTTONS as i64).contains(&buttons) {
                return Err(TranscriptError::format(format!(
                    "line {}: buttons {buttons} out of range",
                    n + 1
                )));
            }
            if run < 1 || out.inputs.len() as i64 + run > frames {
                return Err(TranscriptError::format(format!(
                    "line {}: run of {run} frames exceeds the {frames} the header declares",
                    n + 1
                )));
            }
            out.inputs.extend(std::iter::repeat_n(
                FrameInput::new(buttons as u8),
                run as usize,
            ));
        }
        if chunking.is_some() {
            return Err(TranscriptError::format(
                "truncated: an unfinished save chunk sequence",
            ));
        }
        if out.inputs.len() as i64 != frames {
            return Err(TranscriptError::format(format!(
                "truncated: {} of the {frames} frames the header declares",
                out.inputs.len()
            )));
        }
        if out.header.saves.len() > SLOT_COUNT as usize {
            return Err(TranscriptError::format("more save slots than exist"));
        }
        Ok(out)
    }

    /// Refuse a replay against a cart other than the recorded one.
    pub fn check_cart(&self, snapshot: &Snapshot) -> Result<(), TranscriptError> {
        let digest = cart_digest(snapshot);
        if digest == self.header.cart {
            Ok(())
        } else {
            Err(TranscriptError::new(
                TranscriptError::CART,
                format!(
                    "the transcript was recorded from cart {}, this cart is {digest}",
                    self.header.cart
                ),
            ))
        }
    }
}

fn parse_line(n: usize, text: &str) -> Result<Table, TranscriptError> {
    if text.len() > MAX_LINE_BYTES {
        return Err(TranscriptError::new(
            TranscriptError::SIZE,
            format!("line {} is over {MAX_LINE_BYTES} bytes", n + 1),
        ));
    }
    match codec::decode(text) {
        Ok(Value::Table(t)) => {
            for reserved in ["messages", "io", "connections"] {
                if let Some(v) = t.get(&Key::str(reserved)) {
                    let empty = matches!(v, Value::Table(t) if t.is_empty());
                    if !empty {
                        return Err(TranscriptError::new(
                            TranscriptError::VERSION,
                            format!(
                                "line {}: `{reserved}` is not supported in format version 1",
                                n + 1
                            ),
                        ));
                    }
                }
            }
            Ok(t)
        }
        Ok(_) => Err(TranscriptError::format(format!(
            "line {}: not a table",
            n + 1
        ))),
        Err(e) => Err(TranscriptError::format(format!("line {}: {e}", n + 1))),
    }
}

fn int_of(v: &Value, key: &str, n: usize) -> Result<i64, TranscriptError> {
    match v {
        Value::Int(i) => Ok(*i),
        _ => Err(TranscriptError::format(format!(
            "line {}: `{key}` is not an integer",
            n + 1
        ))),
    }
}

fn get_int(t: &Table, key: &str, n: usize) -> Result<i64, TranscriptError> {
    t.get(&Key::str(key))
        .ok_or_else(|| TranscriptError::format(format!("line {}: missing `{key}`", n + 1)))
        .and_then(|v| int_of(v, key, n))
}

fn get_str(t: &Table, key: &str, n: usize) -> Result<String, TranscriptError> {
    match t.get(&Key::str(key)) {
        Some(Value::Str(s)) => String::from_utf8(s.clone())
            .map_err(|_| TranscriptError::format(format!("line {}: `{key}` is not UTF-8", n + 1))),
        _ => Err(TranscriptError::format(format!(
            "line {}: missing string `{key}`",
            n + 1
        ))),
    }
}

// ----- recording ----------------------------------------------------------

/// Collects the inputs a cart is stepped with, up to [`MAX_FRAMES`].
#[derive(Debug, Clone)]
pub struct Recorder {
    header: Header,
    inputs: Vec<FrameInput>,
    overflowed: bool,
}

pub type SharedRecorder = Rc<RefCell<Recorder>>;

impl Recorder {
    pub fn new(header: Header) -> Recorder {
        Recorder {
            header,
            inputs: Vec::new(),
            overflowed: false,
        }
    }

    pub fn shared(header: Header) -> SharedRecorder {
        Rc::new(RefCell::new(Recorder::new(header)))
    }

    /// One frame the cart saw. Past the limit the frame is dropped and
    /// the recorder remembers that it is no longer complete.
    pub fn record(&mut self, input: FrameInput) {
        if self.inputs.len() as u64 >= MAX_FRAMES {
            self.overflowed = true;
        } else {
            self.inputs.push(input);
        }
    }

    pub fn frames(&self) -> u64 {
        self.inputs.len() as u64
    }

    /// Whether frames were dropped because the run outgrew a transcript.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    pub fn transcript(&self) -> Transcript {
        Transcript {
            header: self.header.clone(),
            inputs: self.inputs.clone(),
        }
    }
}

/// A guest wrapper that records every input the inner guest is stepped
/// with, after the console's own masking, so the transcript holds exactly
/// what the cart saw.
pub struct RecordingGuest {
    inner: Box<dyn Guest>,
    recorder: SharedRecorder,
}

impl RecordingGuest {
    pub fn new(inner: Box<dyn Guest>, recorder: SharedRecorder) -> RecordingGuest {
        RecordingGuest { inner, recorder }
    }

    /// A guest factory that builds with `inner` and, when `recorder` is
    /// given, wraps the guest so every input it sees is recorded.
    pub fn factory<F: FactoryFn>(inner: F, recorder: Option<SharedRecorder>) -> impl FactoryFn {
        move |text: &str, name: &str| {
            let guest = inner(text, name)?;
            Ok(match &recorder {
                Some(r) => Box::new(RecordingGuest::new(guest, r.clone())) as Box<dyn Guest>,
                None => guest,
            })
        }
    }
}

impl Guest for RecordingGuest {
    fn step(&mut self, state: &mut DrawState, input: FrameInput, frame: u64) -> Result<(), Fault> {
        self.recorder.borrow_mut().record(input);
        self.inner.step(state, input, frame)
    }

    fn state(&mut self, names: &[String]) -> Result<String, Fault> {
        self.inner.state(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::SnapshotLimits;

    fn snap(entries: &[(&str, &[u8])]) -> Snapshot {
        Snapshot::from_entries(
            entries.iter().map(|(k, v)| (*k, v.to_vec())),
            SnapshotLimits::default(),
        )
        .unwrap()
    }

    fn sample() -> Transcript {
        let s = snap(&[("main.lua", b"x=1"), ("gfx/a.png", &[1, 2, 3])]);
        let mut big = vec![7u8; SAVE_CHUNK * 2 + 5];
        big[0] = 1;
        let header = Header::new(
            &s,
            0,
            vec![(0, b"hello".to_vec()), (3, big), (5, Vec::new())],
        );
        let mut inputs = vec![FrameInput::new(8); 30];
        inputs.extend(vec![FrameInput::NONE; 2]);
        inputs.push(FrameInput::new(63));
        Transcript { header, inputs }
    }

    #[test]
    fn round_trip_with_saves_and_runs() {
        let t = sample();
        let text = t.encode().unwrap();
        assert!(text.starts_with("kuula-transcript 1\n{ cart = \"0x"));
        assert!(text.contains("{ buttons = 8, frames = 30 }\n"));
        assert!(text.contains("part = 3, parts = 3, slot = 3"));
        assert!(text.contains("part = 1, parts = 1, slot = 5"));
        let back = Transcript::decode(&text).unwrap();
        assert_eq!(back, t);
        assert_eq!(text.lines().count(), 2 + 1 + 3 + 1 + 3);
    }

    #[test]
    fn digest_depends_on_content_and_names_not_order() {
        let a = snap(&[("main.lua", b"x=1"), ("b", b"2")]);
        let b = snap(&[("b", b"2"), ("main.lua", b"x=1")]);
        let c = snap(&[("main.lua", b"x=2"), ("b", b"2")]);
        assert_eq!(cart_digest(&a), cart_digest(&b));
        assert_ne!(cart_digest(&a), cart_digest(&c));
        let t = sample();
        assert!(t
            .check_cart(&snap(&[("main.lua", b"x=1"), ("gfx/a.png", &[1, 2, 3])]))
            .is_ok());
        assert_eq!(t.check_cart(&c).unwrap_err().code, TranscriptError::CART);
    }

    #[test]
    fn bad_files_are_refused_with_codes() {
        let t = sample();
        let text = t.encode().unwrap();
        let code = |s: &str| Transcript::decode(s).unwrap_err().code;
        assert_eq!(code(""), TranscriptError::FORMAT);
        assert_eq!(code("kuula-transcript 2\n{}"), TranscriptError::VERSION);
        assert_eq!(code("nope 1\n{}"), TranscriptError::FORMAT);
        // Truncated: drop the last input run.
        let cut = text.trim_end_matches('\n').rsplit_once('\n').unwrap().0;
        assert_eq!(code(cut), TranscriptError::FORMAT);
        // A save chunk cut off mid-sequence.
        let mut lines: Vec<&str> = text.lines().collect();
        let chunk2 = lines.iter().position(|l| l.contains("part = 2")).unwrap();
        lines.remove(chunk2);
        assert_eq!(code(&lines.join("\n")), TranscriptError::FORMAT);
        // Reserved fields with content are a version error.
        let with = text.replace(
            "{ buttons = 8, frames = 30 }",
            "{ buttons = 8, frames = 30, messages = { [1] = \"hi\" } }",
        );
        assert_eq!(code(&with), TranscriptError::VERSION);
        let with = text.replace(
            "{ buttons = 8, frames = 30 }",
            "{ buttons = 8, frames = 30, messages = {}, io = {} }",
        );
        assert!(
            Transcript::decode(&with).is_ok(),
            "empty reserved fields pass"
        );
        // Out-of-range buttons and over-long runs.
        assert_eq!(
            code(&text.replace("buttons = 63", "buttons = 64")),
            TranscriptError::FORMAT
        );
        assert_eq!(
            code(&text.replace("frames = 30 }", "frames = 31 }")),
            TranscriptError::FORMAT
        );
        assert_eq!(
            code(&text.replace("frames = 33,", "frames = 2000000,")),
            TranscriptError::SIZE
        );
    }

    #[test]
    fn recorder_caps_and_the_wrapper_records_what_the_cart_saw() {
        struct Stub(Vec<u8>);
        impl Guest for Stub {
            fn step(&mut self, _: &mut DrawState, input: FrameInput, _: u64) -> Result<(), Fault> {
                self.0.push(input.buttons);
                Ok(())
            }
        }
        let s = snap(&[("main.lua", b"")]);
        let rec = Recorder::shared(Header::new(&s, 0, Vec::new()));
        let mut g = RecordingGuest::new(Box::new(Stub(Vec::new())), rec.clone());
        let mut state = DrawState::placeholder();
        g.step(&mut state, FrameInput::new(3), 1).unwrap();
        g.step(&mut state, FrameInput::new(0), 2).unwrap();
        let t = rec.borrow().transcript();
        assert_eq!(t.inputs, [FrameInput::new(3), FrameInput::NONE]);
        assert!(!rec.borrow().overflowed());
        let mut r = Recorder::new(Header::new(&s, 0, Vec::new()));
        r.inputs = vec![FrameInput::NONE; MAX_FRAMES as usize];
        r.record(FrameInput::NONE);
        assert!(r.overflowed());
        assert_eq!(r.frames(), MAX_FRAMES);
    }
}
