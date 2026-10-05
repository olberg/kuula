//! The song: the payload of docs/omt.md read into a model the player plays, with the
//! validator's diagnostics (section 13) found on the way.

mod ijson;
mod instruments;
mod load;
mod profile;

use std::fmt;
use std::sync::Arc;

use crate::cell::{Effect, Note};

pub(crate) use ijson::parse_ijson;
pub use ijson::MAX_DEPTH;
// What the bank loader (OMQ, `omq.rs`) reads instruments, samples, tracks and cells with.
pub(crate) use instruments::{load_instruments, load_samples};
pub(crate) use load::{join, load_rate_tick, load_tracks, load_volume_resampling, Ctx};
#[cfg(test)]
pub(crate) use load::load_as;
pub use load::{load, load_sources, parse_global, Resource, Source};
pub use profile::{fits, profile_violations, tier, PROFILES};
pub(crate) use profile::{tier_of, Kuula};

/// The OMT version this engine reads and writes, major.minor: the one constant the reader's version
/// comes from.
pub const VERSION: &str = "0.3";
/// `VERSION` as (major, minor).
pub const READER: (u32, u32) = match parse_version(VERSION) {
    Some(v) => v,
    None => panic!("VERSION is major.minor"),
};

/// A version `digits.digits` as (major, minor), each without leading zeros; `None` otherwise.
pub const fn parse_version(s: &str) -> Option<(u32, u32)> {
    let b = s.as_bytes();
    let mut parts = [0u32; 2];
    let (mut part, mut digits, mut i) = (0, 0, 0);
    while i < b.len() {
        let c = b[i];
        if c == b'.' && part == 0 && digits > 0 {
            (part, digits) = (1, 0);
        } else if c.is_ascii_digit() && digits < 9 && !(digits == 1 && parts[part] == 0) {
            parts[part] = parts[part] * 10 + (c - b'0') as u32;
            digits += 1;
        } else {
            return None;
        }
        i += 1;
    }
    if part == 1 && digits > 0 { Some((parts[0], parts[1])) } else { None }
}

/// Whether a reader of version `reader` reads a song of version `song` (section 1): while the
/// major version is 0, only its own; from 1.0 on, any of its own major version. `Some(true)` for a
/// song of a later minor version, whose additions the reader reads as unknown.
pub fn reads(reader: (u32, u32), song: (u32, u32)) -> Option<bool> {
    if song.0 != reader.0 || (reader.0 == 0 && song.1 != reader.1) {
        return None;
    }
    Some(song.1 > reader.1)
}
/// Bounds of section 15.
pub const MAX_PAYLOAD: usize = 16 << 20;
pub const MAX_CHANNELS: usize = 64;
pub const MAX_INSTRUMENT: u8 = 99;
pub const MAX_SAMPLES: usize = 256;
pub const MAX_TRACKS: usize = 65536;
pub const MAX_ROWS: u32 = 1024;
pub const MAX_ORDERS: usize = 4096;
pub const MAX_ARRANGEMENTS: usize = 256;
pub const MAX_GLOBAL: usize = 4096;
pub const MAX_ZONES: usize = 120;
pub const MAX_POINTS: usize = 32;
pub const MAX_FRAMES: u32 = 1 << 24;
pub const MAX_PITCH: i32 = 33791;

/// A diagnostic: `code@where`, an error or a warning.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diag {
    pub code: String,
    pub path: String,
    pub error: bool,
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.code)
        } else {
            write!(f, "{}@{}", self.code, self.path)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resampling {
    Nearest,
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Music,
    Reserved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub role: Role,
    pub volume: i32,
    pub pan: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Envelope {
    pub attack: u32,
    pub decay: u32,
    pub sustain: u32,
    pub release: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Pulse,
    Triangle,
    Saw,
    Sine,
    Noise,
    SoftNoise,
    /// A table of `tables`, by index.
    Table(usize),
}

/// The names of `waveform` (section 8); `table` plays the first table.
pub const WAVEFORMS: [(&str, Waveform); 7] = [
    ("pulse", Waveform::Pulse),
    ("triangle", Waveform::Triangle),
    ("saw", Waveform::Saw),
    ("sine", Waveform::Sine),
    ("noise", Waveform::Noise),
    ("softnoise", Waveform::SoftNoise),
    ("table", Waveform::Table(0)),
];

/// The waveform a `waveform` sequence's step selects: 0–5 the built-in ones, 6 + i table i.
pub fn sequence_waveform(step: i32) -> Waveform {
    match step {
        0 => Waveform::Pulse,
        1 => Waveform::Triangle,
        2 => Waveform::Saw,
        3 => Waveform::Sine,
        4 => Waveform::Noise,
        5 => Waveform::SoftNoise,
        s => Waveform::Table((s - 6).max(0) as usize),
    }
}

/// The most tables a `wave` instrument has, and the most steps a sequence has.
pub const MAX_TABLES: usize = 16;
pub const MAX_STEPS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sequence {
    pub steps: Vec<i32>,
    pub loop_step: Option<usize>,
    pub release: Option<usize>,
    /// Ticks a step, 1..=255.
    pub speed: u32,
}

impl Default for Sequence {
    fn default() -> Sequence {
        Sequence { steps: Vec::new(), loop_step: None, release: None, speed: 1 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sequences {
    pub volume: Option<Sequence>,
    pub arpeggio: Option<Sequence>,
    pub pitch: Option<Sequence>,
    pub duty: Option<Sequence>,
    pub waveform: Option<Sequence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wave {
    pub waveform: Waveform,
    pub duty: i32,
    pub tables: Vec<Vec<i16>>,
    /// The second oscillator: its transpose in pitch units and its level, 0..=64.
    pub second: Option<(i32, i32)>,
    pub nna: Action,
    pub sequences: Sequences,
}

/// A new-note action, and a duplicate's action (section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Cut,
    Continue,
    Release,
    Fade,
}

pub const ACTIONS: [(&str, Action); 4] =
    [("cut", Action::Cut), ("continue", Action::Continue), ("release", Action::Release), ("fade", Action::Fade)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duplicate {
    Off,
    Note,
    Sample,
    Instrument,
}

/// A key zone: the notes `low` to `high` play `sample` with `transpose` added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zone {
    pub low: i32,
    pub high: i32,
    pub sample: usize,
    pub transpose: i32,
}

/// A point envelope (section 9): ticks and values, with a loop and a sustain loop between points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointEnvelope {
    pub points: Vec<(u32, i32)>,
    pub looping: Option<(usize, usize)>,
    pub sustain: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VibratoWave {
    Sine,
    Square,
    Ramp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoVibrato {
    pub waveform: VibratoWave,
    pub depth: i32,
    pub rate: u32,
    pub sweep: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sampler {
    /// The sample every note plays, or the key map's zones.
    pub sample: Option<usize>,
    pub zones: Vec<Zone>,
    pub fadeout: Option<u32>,
    pub volume_envelope: Option<PointEnvelope>,
    pub pan_envelope: Option<PointEnvelope>,
    pub pitch_envelope: Option<PointEnvelope>,
    pub autovibrato: Option<AutoVibrato>,
    pub nna: Action,
    pub dct: Duplicate,
    pub dca: Action,
    /// Cutoff and resonance, 0..=127.
    pub filter: Option<(u8, u8)>,
}

impl Sampler {
    /// The sample a note (as written, before transposes) plays, and its zone's transpose.
    pub fn sample_for(&self, note: i32) -> Option<(usize, i32)> {
        if let Some(s) = self.sample {
            return Some((s, 0));
        }
        self.zones.iter().find(|z| note >= z.low && note <= z.high).map(|z| (z.sample, z.transpose))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Engine {
    Wave(Wave),
    Sampler(Sampler),
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instrument {
    pub number: u8,
    /// Its place in the payload's `instruments`, for diagnostics.
    pub index: usize,
    pub name: String,
    pub volume: i32,
    pub transpose: i32,
    /// Where the instrument's voices start, instead of their channel's pan.
    pub pan: Option<i32>,
    pub envelope: Envelope,
    pub engine: Engine,
    /// Whether its sampler has a filter that is an object, valid or not: the song is then
    /// faithful (section 13).
    pub filter: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    Forward,
    PingPong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loop {
    pub mode: LoopMode,
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Pcm16,
    Flac,
    Opus,
}

/// A sample record and, when its resource was there and matched, its data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    pub name: String,
    pub resource: usize,
    pub encoding: Encoding,
    pub rate: u32,
    pub channels: u32,
    pub frames: u32,
    pub root: i32,
    /// 0..=64.
    pub gain: i32,
    pub looping: Option<Loop>,
    /// Interleaved 16-bit frames, or `None` when the resource is missing, doesn't match, or is in
    /// an encoding this engine doesn't decode.
    pub data: Option<Arc<[i16]>>,
}

/// A cell with the track's state resolved (section 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub note: Note,
    /// The instrument a note plays: the cell's own, or the last one named earlier in the track.
    pub ins: u8,
    /// The instrument written in the cell, or 0.
    pub written_ins: u8,
    /// The volume written in the cell.
    pub vol: Option<u8>,
    /// The volume a note starts with: the cell's, the track's last, or the instrument's default.
    pub note_vol: u8,
    pub effects: Vec<Effect>,
    /// The cell as written.
    pub text: String,
}

impl Cell {
    pub fn delay(&self) -> u32 {
        self.effects
            .iter()
            .find_map(|e| match e {
                Effect::Delay(d) => Some(*d as u32),
                _ => None,
            })
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub name: String,
    /// The rows the player plays: 1 when the payload's count is out of range.
    pub rows: u32,
    /// The rows as written, when an integer: the `kuula` profile's `rows` rule reads them.
    pub written_rows: i64,
    pub speed: u32,
    pub loop_row: Option<u32>,
    /// The rows that have a cell, in row order: a track stores only those (section 5), so that a
    /// payload of many long, empty tracks doesn't cost a cell's memory for every row.
    pub cells: Vec<(u32, Cell)>,
}

impl Track {
    /// The cell on row `row`, if it has one.
    pub fn cell(&self, row: usize) -> Option<&Cell> {
        let i = self.cells.binary_search_by_key(&row, |(r, _)| *r as usize).ok()?;
        Some(&self.cells[i].1)
    }

    /// Ticks from the track's start to its end, without looping.
    pub fn ticks(&self) -> u64 {
        self.rows as u64 * self.speed as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Following,
    Order(usize),
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    pub tracks: Vec<Option<usize>>,
    pub ticks: u64,
    pub next: Next,
}

/// An event of the global lane (section 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Global {
    Tick(u64, u64),
    Groove(Vec<u32>),
    Volume(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalEvent {
    pub order: usize,
    pub tick: u64,
    pub event: Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrangement {
    pub name: String,
    pub orders: Vec<Order>,
    /// Sorted by order row and tick.
    pub global: Vec<GlobalEvent>,
    /// The entries `global` holds as written, good or bad (the `pico8` profile's `global` rule).
    pub global_entries: usize,
}

impl Arrangement {
    /// The global events at a tick of an order row, in file order.
    pub fn global_at(&self, order: usize, tick: u64) -> impl Iterator<Item = &GlobalEvent> {
        let start = self.global.partition_point(|e| (e.order, e.tick) < (order, tick));
        self.global[start..].iter().take_while(move |e| e.order == order && e.tick == tick)
    }
}

impl Arrangement {
    /// The order row that follows `o`, or `None` at the end of the note data.
    pub fn after(&self, o: usize) -> Option<usize> {
        match self.orders.get(o)?.next {
            Next::Following => (o + 1 < self.orders.len()).then_some(o + 1),
            Next::Order(i) => (i < self.orders.len()).then_some(i),
            Next::Stop => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Song {
    pub title: String,
    pub profile: Option<String>,
    pub rate: u32,
    pub tick: (u64, u64),
    pub ticks_per_beat: Option<u64>,
    pub volume: i32,
    pub resampling: Resampling,
    pub channels: Vec<Channel>,
    /// Indexed by number; slot 0 is always empty.
    pub instruments: Vec<Option<Arc<Instrument>>>,
    pub samples: Vec<Arc<Sample>>,
    pub tracks: Vec<Track>,
    pub arrangements: Vec<Arrangement>,
}

impl Song {
    pub fn instrument(&self, n: u8) -> Option<&Arc<Instrument>> {
        self.instruments.get(n as usize)?.as_ref()
    }

    /// The frame at which tick `n` starts: ⌊n × N × rate / D⌋ (section 2). N is at most 2^53, so
    /// the product fits 128 bits for any tick within 2^57, far beyond an hour; it saturates past.
    pub fn tick_frame(&self, n: u64) -> u64 {
        let (num, den) = self.tick;
        let product = (n as u128).saturating_mul(num as u128).saturating_mul(self.rate as u128);
        (product / den as u128).min(u64::MAX as u128) as u64
    }
}

/// What loading a payload found: the song, when it could be read, and every diagnostic.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub song: Option<Song>,
    pub diags: Vec<Diag>,
}

impl Loaded {
    pub fn errors(&self) -> impl Iterator<Item = &Diag> {
        self.diags.iter().filter(|d| d.error)
    }

    pub fn is_valid(&self) -> bool {
        self.song.is_some() && self.errors().next().is_none()
    }
}

/// The validator rules of section 13 and the version rule of section 1, worked by hand.
#[cfg(test)]
#[path = "song_tests.rs"]
mod tests;
