//! The player: plays an arrangement of a song tick by tick into 16-bit stereo frames, exactly as
//! docs/omt.md sections 2, 4, 6, 7, 8, 9 and 10 have it. Everything in the render path is integer,
//! except the filter, which the specification leaves to the player (a faithful feature), so every
//! machine produces the same PCM for an exact song.

mod cells;
mod voice;

use std::sync::Arc;

use crate::cell::{Effect, Note};
use crate::song::{Action, Arrangement, Global, Instrument, Song, MAX_PITCH};
use crate::tables::{FREQ, SINE};

pub use voice::Voice;

/// Envelope level resolution: 24 bits.
const ENV_ONE: u32 = 1 << 24;
const NOISE_SEED: u16 = 0x2a7f;
/// The fade level of a voice that isn't fading.
const FADE_ONE: u32 = 65536;
/// A channel's background voices at most (section 6).
const MAX_BACKGROUND: usize = 64;

/// A voice event, for the event trace (section 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    On,
    Retarget,
    Release,
    Cut,
    Fade,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub frame: u64,
    pub channel: usize,
    pub kind: EventKind,
    pub instrument: u8,
    pub pitch: i32,
}

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self.kind {
            EventKind::On => "on",
            EventKind::Retarget => "retarget",
            EventKind::Release => "release",
            EventKind::Cut => "cut",
            EventKind::Fade => "fade",
            EventKind::End => "end",
        };
        write!(f, "{} {} {}", self.frame, self.channel, name)?;
        if matches!(self.kind, EventKind::On | EventKind::Retarget) {
            write!(f, " {} {}", self.instrument, self.pitch)?;
        }
        Ok(())
    }
}

/// The frequency of pitch `p` in 16.16 fixed-point Hz (section 3).
pub fn freq(p: i32) -> u64 {
    let p = p.clamp(0, MAX_PITCH) as u32;
    (FREQ[(p % 3072) as usize] >> (10 - p / 3072)) as u64
}

/// The oscillator's phase increment at pitch `p` and output rate `rate` (section 8).
pub fn increment(p: i32, rate: u32) -> u32 {
    (freq(p) * 65536 / rate as u64) as u32
}

/// `pitch_of(F)` (section 7): the largest pitch whose frequency is at most `f` (16.16 Hz), or 0
/// when `f` is below FREQ(0). FREQ never falls as the pitch rises, so a binary search finds it.
pub fn pitch_of(f: u64) -> i32 {
    if f < freq(0) {
        return 0;
    }
    let (mut lo, mut hi) = (0, MAX_PITCH); // freq(lo) <= f throughout
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if freq(mid) <= f { lo = mid } else { hi = mid - 1 }
    }
    lo
}

/// A vibrato waveform of `vibw` at phase `phi`, 0..=255 (section 7).
pub fn vibrato_wave(w: u8, phi: i32) -> i32 {
    match w {
        1 => {
            let v = if phi < 64 {
                512 * phi
            } else if phi < 192 {
                32767 - 512 * (phi - 64)
            } else {
                -32767 + 512 * (phi - 192)
            };
            v.clamp(-32767, 32767)
        }
        2 => 32767 - 256 * phi,
        3 => {
            if phi < 128 { 32767 } else { -32767 }
        }
        _ => SINE[phi as usize] as i32,
    }
}

/// The pitch of MIDI 99, whose increment is soft noise's `R` (section 8).
const SOFT_NOISE_PITCH: i32 = 25344;

/// A sampled voice's step in 32.32 frames per output frame (section 9).
pub fn sample_step(p: i32, root: i32, srate: u32, rate: u32) -> u64 {
    let p = p.clamp(0, MAX_PITCH) as u32;
    let r = root.clamp(0, MAX_PITCH) as u32;
    let num = (FREQ[(p % 3072) as usize] as u128 * srate as u128) << (32 + p / 3072);
    let den = (FREQ[(r % 3072) as usize] as u128 * rate as u128) << (r / 3072);
    (num / den).min(u64::MAX as u128) as u64
}

fn env_frames(ms: u32, rate: u32) -> u32 {
    (ms as u64 * rate as u64 / 1000) as u32
}

fn env_step(frames: u32, span: u32) -> u32 {
    if frames == 0 { ENV_ONE } else { (span / frames).max(1) }
}

/// Integer division rounding towards minus infinity.
fn floor_div(a: i64, b: i64) -> i64 {
    a.div_euclid(b) - if b < 0 && a.rem_euclid(b) != 0 { 1 } else { 0 }
}

/// Where a track is at local tick `tau` of its order row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrackPos {
    Row(usize, u32),
    /// The track ends on this tick: its channel is released.
    EndsNow,
    Ended,
}

fn track_pos(rows: u32, speed: u32, loop_row: Option<u32>, tau: u64) -> TrackPos {
    let speed64 = speed as u64;
    let len = rows as u64 * speed64;
    if tau < len {
        return TrackPos::Row((tau / speed64) as usize, (tau % speed64) as u32);
    }
    match loop_row {
        Some(l) => {
            let period = (rows - l) as u64 * speed64;
            let t = l as u64 * speed64 + (tau - len) % period;
            TrackPos::Row((t / speed64) as usize, (t % speed64) as u32)
        }
        None if tau == len => TrackPos::EndsNow,
        None => TrackPos::Ended,
    }
}

#[derive(Debug, Clone, Default)]
struct Chan {
    voice: Option<Voice>,
    background: Vec<Voice>,
    /// The row being played: its track and row, when it has a cell.
    cell: Option<(usize, usize)>,
    /// The tick within the row.
    k: u32,
    /// The foreground voice's base pitch and note volume on the tick of the row's events: where
    /// `glide`, `fglide` and `vglide` start their lines.
    line_pitch: i32,
    line_vol: i32,
    /// Where a line whose tick d is the first after its row arrives as the row ends: the base
    /// pitch and the note volume the next row starts at.
    arrive_pitch: Option<i32>,
    arrive_vol: Option<i32>,
}

/// What the player is doing with the arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Not reading the song; voices and keyboard notes still sound.
    Stopped,
    /// Following the arrangement, loops included.
    Song,
    /// Playing one order row over and over.
    Order,
}

/// A note played from outside the song, as the tracker's keyboard does.
#[derive(Debug, Clone)]
struct Jam {
    key: i32,
    voice: Voice,
    volume: i32,
}

/// A segment of time (section 2): its tick, its groove, and the frame its first tick starts at.
#[derive(Debug, Clone)]
struct Segment {
    num: u64,
    den: u64,
    weights: Vec<u32>,
    /// prefix[i] is the sum of the first i weights; the last is W.
    prefix: Vec<u64>,
    base: u64,
}

impl Segment {
    fn new(num: u64, den: u64, weights: Vec<u32>, base: u64) -> Segment {
        let mut prefix = vec![0u64];
        for w in &weights {
            prefix.push(prefix.last().unwrap() + *w as u64);
        }
        Segment { num, den, weights, prefix, base }
    }

    /// The frame tick `j` of the segment starts at.
    fn frame(&self, j: u64, rate: u32) -> u64 {
        let k = self.weights.len() as u64;
        let w = *self.prefix.last().unwrap();
        let p = (j / k) as u128 * w as u128 + self.prefix[(j % k) as usize] as u128;
        let top = p.saturating_mul(k as u128).saturating_mul(self.num as u128).saturating_mul(rate as u128);
        let bottom = w as u128 * self.den as u128;
        self.base.saturating_add((top / bottom).min(u64::MAX as u128) as u64)
    }
}

pub struct Player {
    song: Arc<Song>,
    arrangement: usize,
    mode: Mode,
    /// A rendering: going back to an order row already played ends the note data.
    once: bool,
    order: usize,
    /// The local tick of the next tick in the current order row.
    order_tick: u64,
    visited: Vec<bool>,
    data_ended: bool,
    channels: Vec<Chan>,
    mute: Vec<bool>,
    /// A host's gain for each channel, 0..=256 with 256 as unity: not the song's, so a rendering
    /// leaves it alone.
    gain: Vec<i64>,
    /// A host's transposition for each channel, in pitch units: added wherever the instrument's
    /// `transpose` is used for the channel's notes, and 0 unless a host sets it (OMQ section 5).
    transpose: Vec<i32>,
    jams: Vec<Jam>,
    /// Time: the segment, the ticks started in it, the frame reached and where the tick ends.
    segment: Segment,
    tick: u64,
    frame: u64,
    tick_end: u64,
    global_volume: i32,
    /// The order row and local tick of the tick now sounding, for the editor.
    now: (usize, u64),
    trace: Option<Vec<Event>>,
    /// A rendering: the frame each order row first started at, and the loop found.
    first_start: Vec<Option<u64>>,
    loop_frames: Option<(u64, u64)>,
}

impl Player {
    pub fn new(song: Arc<Song>) -> Player {
        let n = song.channels.len();
        let segment = Segment::new(song.tick.0, song.tick.1, vec![1], 0);
        Player {
            song,
            arrangement: 0,
            mode: Mode::Stopped,
            once: false,
            order: 0,
            order_tick: 0,
            visited: Vec::new(),
            data_ended: false,
            channels: vec![Chan::default(); n],
            mute: vec![false; n],
            gain: vec![256; n],
            transpose: vec![0; n],
            jams: Vec::new(),
            segment,
            tick: 0,
            frame: 0,
            tick_end: 0,
            global_volume: 256,
            now: (0, 0),
            trace: None,
            first_start: Vec::new(),
            loop_frames: None,
        }
    }

    pub fn song(&self) -> &Arc<Song> {
        &self.song
    }

    /// Starts a new segment at the next tick boundary.
    fn new_segment(&mut self, num: u64, den: u64, weights: Vec<u32>) {
        self.segment = Segment::new(num, den, weights, self.tick_end);
        self.tick = 0;
    }

    /// Swaps the song, keeping the position and every sounding voice (which keep the instrument
    /// they started with). The position is clamped to the new song.
    pub fn set_song(&mut self, song: Arc<Song>) {
        if song.rate != self.song.rate || song.tick != self.song.tick {
            self.new_segment(song.tick.0, song.tick.1, vec![1]);
        }
        let n = song.channels.len();
        self.channels.resize(n, Chan::default());
        self.mute.resize(n, false);
        self.gain.resize(n, 256);
        self.transpose.resize(n, 0);
        self.song = song;
        // The row being played keeps its curves if the song still has it.
        let tracks = &self.song.tracks;
        for ch in &mut self.channels {
            if let Some((t, row)) = ch.cell {
                if tracks.get(t).is_none_or(|tr| tr.cell(row).is_none()) {
                    ch.cell = None;
                }
            }
        }
        if self.arrangement >= self.song.arrangements.len() {
            self.arrangement = 0;
            self.mode = Mode::Stopped;
        }
        let orders = self.song.arrangements[self.arrangement].orders.len();
        if self.order >= orders {
            self.order = orders - 1;
            self.order_tick = 0;
        }
        self.visited.resize(orders, false);
        self.first_start.resize(orders, None);
    }

    fn arrangement(&self) -> &Arrangement {
        &self.song.arrangements[self.arrangement]
    }

    /// Starts arrangement `a` at order row `order`, local tick `tick`, cutting every voice. The
    /// global lane's events before the start, in the order of the rows, are applied first.
    pub fn play(&mut self, a: usize, order: usize, tick: u64, mode: Mode) {
        self.stop();
        if a >= self.song.arrangements.len() || order >= self.song.arrangements[a].orders.len() {
            return;
        }
        self.arrangement = a;
        self.mode = mode;
        self.order = order;
        self.order_tick = tick;
        self.visited = vec![false; self.arrangement().orders.len()];
        self.visited[order] = true;
        self.first_start = vec![None; self.visited.len()];
        self.first_start[order] = Some(self.frame);
        self.loop_frames = None;
        self.data_ended = false;
        self.tick_end = self.frame;
        self.global_volume = 256;
        let song = self.song.clone();
        let (mut num, mut den) = song.tick;
        let mut weights = vec![1];
        for e in &song.arrangements[a].global {
            if (e.order, e.tick) >= (order, tick) {
                break;
            }
            match &e.event {
                Global::Tick(n, d) => (num, den) = (*n, *d),
                Global::Groove(w) => weights = w.clone(),
                Global::Volume(g) => self.global_volume = *g,
            }
        }
        self.new_segment(num, den, weights);
        self.now = (order, tick);
    }

    /// Plays the notes of the rows that start at local tick `tick` of order row `order`, as if the
    /// song reached them, without going on. Notes still sounding from the song are released first.
    pub fn play_row(&mut self, a: usize, order: usize, tick: u64) {
        if self.mode != Mode::Stopped {
            return;
        }
        let song = self.song.clone();
        let Some(row) = song.arrangements.get(a).and_then(|arr| arr.orders.get(order)) else { return };
        for c in 0..self.channels.len() {
            if let Some(v) = &mut self.channels[c].voice {
                v.release();
            }
            let Some(t) = row.tracks.get(c).copied().flatten() else { continue };
            let track = &song.tracks[t];
            if let TrackPos::Row(r, 0) = track_pos(track.rows, track.speed, track.loop_row, tick) {
                if let Some(cell) = track.cell(r) {
                    self.cell_events(c, cell);
                }
            }
        }
        self.tick_voices();
    }

    /// Stops reading the song and cuts every voice of the song's channels.
    pub fn stop(&mut self) {
        self.mode = Mode::Stopped;
        for ch in &mut self.channels {
            ch.voice = None;
            ch.background.clear();
            ch.cell = None;
            (ch.arrive_pitch, ch.arrive_vol) = (None, None);
        }
    }

    /// Stops reading the song and releases its voices, as the end of the note data does. A line
    /// that arrives as its row ends arrives first: the release tail plays at its target (section 7).
    pub fn finish(&mut self) {
        self.mode = Mode::Stopped;
        self.data_ended = true;
        self.arrive();
        for c in 0..self.channels.len() {
            self.release(c);
            // Background voices are released too, each with its event (section 11).
            for i in 0..self.channels[c].background.len() {
                self.channels[c].background[i].release();
                self.event(c, EventKind::Release, 0, 0);
            }
            self.channels[c].cell = None;
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The order row and the local tick of the tick sounding now.
    pub fn position(&self) -> (usize, u64) {
        self.now
    }

    pub fn arrangement_index(&self) -> usize {
        self.arrangement
    }

    pub fn set_mute(&mut self, channel: usize, mute: bool) {
        if let Some(m) = self.mute.get_mut(channel) {
            *m = mute;
        }
    }

    /// A host's gain for a channel's voices, 0 to 256, where 256 (the start's) is unity: each side
    /// of a voice's frame becomes (x × gain) >> 8 before the voices are summed (section 10). A
    /// player's own control, as `set_mute` is: the song doesn't carry it.
    pub fn set_gain(&mut self, channel: usize, gain: i32) {
        if let Some(g) = self.gain.get_mut(channel) {
            *g = gain.clamp(0, 256) as i64;
        }
    }

    /// A host's transposition for a channel's notes, in pitch units, 0 (the start's) being none: it
    /// is added to the `transpose` of every instrument wherever that is used for the channel's
    /// notes: the pitch a new voice starts at, a retarget's target, a retriggered voice's, the
    /// duplicate check's and the note soft noise takes its gain from, but not the key-map lookup,
    /// which reads the note as written. The sum isn't held to `transpose`'s range; pitches are
    /// clamped where they always are. A player's own control, as `set_gain` is (OMQ section 5).
    pub fn set_transpose(&mut self, channel: usize, transpose: i32) {
        if let Some(t) = self.transpose.get_mut(channel) {
            *t = transpose;
        }
    }

    /// True when no voice of the channel is sounding, foreground or background.
    pub fn channel_silent(&self, channel: usize) -> bool {
        self.channels.get(channel).is_none_or(|c| c.voice.is_none() && c.background.is_empty())
    }

    /// True when no voice is sounding, in the song's channels or from the keyboard.
    pub fn silent(&self) -> bool {
        self.channels.iter().all(|c| c.voice.is_none() && c.background.is_empty()) && self.jams.is_empty()
    }

    /// The event trace so far, in the order the events happen (section 11): for each tick the `end`
    /// lines of the voices gone after the tick before, then the tick's events, then the `end` lines
    /// of the voices that end during its frames, each in channel order.
    pub fn trace(&mut self) -> Option<Vec<Event>> {
        self.trace.take()
    }

    pub fn record_trace(&mut self) {
        self.trace = Some(Vec::new());
    }

    /// A note from the keyboard: `ins` of `song` for the MIDI pitch `note` at `pitch` (transpose
    /// included by the caller), at `pan`, with the channel volume `volume`. `key` identifies it for
    /// `jam_off`.
    #[allow(clippy::too_many_arguments)]
    pub fn jam_on(&mut self, key: i32, song: &Song, ins: &Arc<Instrument>, note: i32, pitch: i32, vol: i32, pan: i32, volume: i32) {
        self.jams.retain(|j| j.key != key);
        if let Some(mut voice) = Voice::new(song, ins, note, pitch, vol, pan, 0) {
            voice.tick(self.song.rate);
            self.jams.push(Jam { key, voice, volume });
            if self.jams.len() > 32 {
                self.jams.remove(0);
            }
        }
    }

    /// Starts a note on a channel outside the song's note data, as a host does for a cue (OMQ
    /// section 4): `ins` for the pitch `note` as written, sounding at `pitch`, with the note volume
    /// `vol` and the channel's pan. The voice it replaces is cut. No event is recorded.
    pub(crate) fn start_voice(&mut self, channel: usize, ins: u8, note: i32, pitch: i32, vol: i32) {
        let song = self.song.clone();
        let Some(instrument) = song.instrument(ins) else { return };
        let Some(pan) = song.channels.get(channel).map(|c| c.pan) else { return };
        self.channels[channel].voice = Voice::new(&song, instrument, note, pitch, vol, pan, 0);
    }

    pub fn jam_off(&mut self, key: i32) {
        for j in &mut self.jams {
            if j.key == key {
                j.voice.release();
                j.key = i32::MIN;
            }
        }
    }

    /// Silences every keyboard note of instrument `number`.
    pub fn jam_cut(&mut self, number: u8) {
        self.jams.retain(|j| j.voice.ins.number != number);
    }

    pub fn jam_cut_all(&mut self) {
        self.jams.clear();
    }

    fn event(&mut self, channel: usize, kind: EventKind, instrument: u8, pitch: i32) {
        let frame = self.tick_start_frame();
        if let Some(t) = &mut self.trace {
            t.push(Event { frame, channel, kind, instrument, pitch });
        }
    }

    fn tick_start_frame(&self) -> u64 {
        self.segment.frame(self.tick, self.song.rate)
    }

    fn release(&mut self, c: usize) {
        if let Some(v) = &mut self.channels[c].voice {
            v.release();
            self.event(c, EventKind::Release, 0, 0);
        }
    }

    fn cut(&mut self, c: usize) {
        if self.channels[c].voice.take().is_some() {
            self.event(c, EventKind::Cut, 0, 0);
        }
    }

    fn fade(&mut self, c: usize) {
        if let Some(v) = &mut self.channels[c].voice {
            let kind = v.fade();
            self.event(c, kind, 0, 0);
        }
    }

    /// Applies an action to a voice, recording it; the voice, unless it ended.
    fn act(&mut self, c: usize, voice: Voice, action: Action) -> Option<Voice> {
        let mut kind = EventKind::End;
        let kept = voice.act(action, &mut kind);
        if kind != EventKind::End {
            self.event(c, kind, 0, 0);
        }
        kept
    }

    /// Everything that happens at the start of a tick (section 2), after what ends with the tick
    /// before.
    fn start_tick(&mut self) {
        self.after_tick();
        if self.mode != Mode::Stopped {
            self.sequence();
        }
        self.tick_voices();
    }

    /// What happens after a tick's frames, before the next tick's events: fades fall, and the
    /// voices a fade or a volume envelope ends are gone (section 9). Their `end` is at the next
    /// tick's first frame, the first they give nothing on.
    fn after_tick(&mut self) {
        let frame = self.tick_start_frame();
        let mut ended: Vec<usize> = Vec::new();
        for (c, ch) in self.channels.iter_mut().enumerate() {
            let before = ch.background.len();
            ch.background.retain_mut(Voice::after_tick);
            for _ in ch.background.len()..before {
                ended.push(c);
            }
            if let Some(v) = &mut ch.voice {
                if !v.after_tick() {
                    ch.voice = None;
                    ended.push(c);
                }
            }
        }
        self.jams.retain_mut(|j| j.voice.after_tick());
        if let Some(t) = &mut self.trace {
            for channel in ended {
                t.push(Event { frame, channel, kind: EventKind::End, instrument: 0, pitch: 0 });
            }
        }
    }

    /// Every voice's sequences and envelopes, and the pitch it sounds at this tick.
    fn tick_voices(&mut self) {
        let rate = self.song.rate;
        for ch in &mut self.channels {
            if let Some(v) = &mut ch.voice {
                v.tick(rate);
            }
            for v in &mut ch.background {
                v.tick(rate);
            }
        }
        for j in &mut self.jams {
            j.voice.tick(rate);
        }
    }

    /// Lines that arrive as their row ends (section 7): the base pitch and note volume the next
    /// row, or the release tail after the note data, starts at.
    fn arrive(&mut self) {
        for ch in &mut self.channels {
            let (pitch, vol) = (ch.arrive_pitch.take(), ch.arrive_vol.take());
            if let Some(v) = &mut ch.voice {
                if let Some(p) = pitch {
                    v.base = p;
                }
                if let Some(x) = vol {
                    v.vol = x;
                }
            }
        }
    }

    fn sequence(&mut self) {
        let song = self.song.clone();
        let arr = &song.arrangements[self.arrangement];
        if self.order_tick >= arr.orders[self.order].ticks {
            let next = if self.mode == Mode::Order { Some(self.order) } else { arr.after(self.order) };
            if let (true, Some(o)) = (self.once && self.mode == Mode::Song, next) {
                if self.visited[o] {
                    self.loop_frames = self.first_start[o].map(|s| (s, self.tick_start_frame()));
                } else {
                    self.first_start[o] = Some(self.tick_start_frame());
                }
            }
            match next {
                Some(o) if !(self.once && self.visited[o] && self.mode == Mode::Song) => {
                    self.order = o;
                    self.order_tick = 0;
                    self.visited[o] = true;
                    for ch in &mut self.channels {
                        ch.cell = None;
                    }
                }
                _ => {
                    self.finish();
                    return;
                }
            }
        }
        // Lines that arrive as their row ended.
        self.arrive();
        let tau = self.order_tick;
        self.now = (self.order, tau);
        // The global lane first.
        let events: Vec<Global> = arr.global_at(self.order, tau).map(|e| e.event.clone()).collect();
        if !events.is_empty() {
            let (mut num, mut den, mut weights) = (self.segment.num, self.segment.den, self.segment.weights.clone());
            let mut changes_time = false;
            for e in events {
                match e {
                    Global::Tick(n, d) => {
                        (num, den) = (n, d);
                        changes_time = true;
                    }
                    Global::Groove(w) => {
                        weights = w;
                        changes_time = true;
                    }
                    Global::Volume(g) => self.global_volume = g,
                }
            }
            if changes_time {
                // The segment starts at this tick: the frame it starts at in the old one.
                self.segment = Segment::new(num, den, weights, self.tick_start_frame());
                self.tick = 0;
            }
        }
        let order = &arr.orders[self.order];
        // Events, in channel order.
        for c in 0..self.channels.len() {
            let Some(t) = order.tracks.get(c).copied().flatten() else {
                self.channels[c].cell = None;
                continue;
            };
            let track = &song.tracks[t];
            match track_pos(track.rows, track.speed, track.loop_row, tau) {
                TrackPos::Row(row, k) => {
                    if k == 0 {
                        self.channels[c].cell = track.cell(row).is_some().then_some((t, row));
                    }
                    self.channels[c].k = k;
                }
                TrackPos::EndsNow => {
                    self.channels[c].cell = None;
                    self.release(c);
                    continue;
                }
                TrackPos::Ended => {
                    self.channels[c].cell = None;
                    continue;
                }
            }
            let Some((t, row)) = self.channels[c].cell else { continue };
            let cell = song.tracks[t].cell(row).unwrap();
            let k = self.channels[c].k;
            let ev = cell.delay();
            if k == ev {
                self.cell_events(c, cell);
            }
            if k >= ev {
                // Restarts and cuts, in the order the cell writes them (section 7). A restart
                // starts again at the cell's offset, which only a cell with a note has.
                let offset = match cell.note {
                    Note::On(_) => cell.effects.iter().find_map(|e| if let Effect::Offset(f) = *e { Some(f as u32) } else { None }).unwrap_or(0),
                    _ => 0,
                };
                let k = k - ev;
                for e in &cell.effects {
                    match *e {
                        Effect::Retrig(r) if k > 0 && k % r as u32 == 0 => self.retrigger(c, None, offset),
                        Effect::Retrigv(r, x) if k > 0 && k % r as u32 == 0 => self.retrigger(c, Some(x), offset),
                        Effect::Cut(t) if k == t as u32 => self.cut(c),
                        _ => {}
                    }
                }
            }
        }
        // Curves, in channel order.
        for c in 0..self.channels.len() {
            let Some((t, row)) = self.channels[c].cell else { continue };
            let cell = song.tracks[t].cell(row).unwrap();
            let (k, ev) = (self.channels[c].k, cell.delay());
            if k >= ev {
                let last = k + 1 == song.tracks[t].speed;
                self.curves(c, cell, k - ev, last);
            }
        }
        self.order_tick += 1;
    }

    /// Renders `out.len() / 2` stereo frames, interleaved.
    pub fn render(&mut self, out: &mut [i16]) {
        let frames = out.len() / 2;
        let mut done = 0;
        while done < frames {
            if self.frame >= self.tick_end {
                self.start_tick();
                self.tick += 1;
                self.tick_end = self.tick_start_frame();
            }
            let n = ((self.tick_end - self.frame) as usize).min(frames - done);
            self.mix(&mut out[done * 2..(done + n) * 2]);
            done += n;
            self.frame += n as u64;
        }
    }

    fn mix(&mut self, out: &mut [i16]) {
        let resampling = self.song.resampling;
        let volume = self.song.volume as i64;
        let global = self.global_volume as i64;
        let first_frame = self.frame;
        let mut ended: Vec<(u64, usize)> = Vec::new();
        for (f, pair) in out.chunks_exact_mut(2).enumerate() {
            let (mut sl, mut sr) = (0i64, 0i64);
            for (c, ch) in self.channels.iter_mut().enumerate() {
                let channel_volume = self.song.channels[c].volume as i64;
                let muted = self.mute[c];
                let gain = self.gain[c];
                if let Some(v) = &mut ch.voice {
                    match v.frame(resampling, channel_volume) {
                        Some((l, r)) if !muted => {
                            sl += (l * gain) >> 8;
                            sr += (r * gain) >> 8;
                        }
                        Some(_) => {}
                        None => {
                            ch.voice = None;
                            ended.push((first_frame + f as u64, c));
                        }
                    }
                }
                let before = ch.background.len();
                ch.background.retain_mut(|v| match v.frame(resampling, channel_volume) {
                    Some((l, r)) => {
                        if !muted {
                            sl += (l * gain) >> 8;
                            sr += (r * gain) >> 8;
                        }
                        true
                    }
                    None => false,
                });
                for _ in ch.background.len()..before {
                    ended.push((first_frame + f as u64, c));
                }
            }
            self.jams.retain_mut(|j| match j.voice.frame(resampling, j.volume as i64) {
                Some((l, r)) => {
                    sl += l;
                    sr += r;
                    true
                }
                None => false,
            });
            pair[0] = ((((sl * volume) >> 8) * global) >> 8).clamp(-32768, 32767) as i16;
            pair[1] = ((((sr * volume) >> 8) * global) >> 8).clamp(-32768, 32767) as i16;
        }
        if let Some(t) = &mut self.trace {
            for (frame, channel) in ended {
                t.push(Event { frame, channel, kind: EventKind::End, instrument: 0, pitch: 0 });
            }
        }
    }
}

/// A rendering of an arrangement (section 11).
pub struct Rendering {
    /// Interleaved stereo frames.
    pub pcm: Vec<i16>,
    pub trace: Vec<Event>,
    /// Cut off at one hour.
    pub too_long: bool,
    /// Where the arrangement loops: the frame its loop starts at, and the frame it went back from.
    pub loop_frames: Option<(u64, u64)>,
}

impl Rendering {
    pub fn frames(&self) -> usize {
        self.pcm.len() / 2
    }

    /// The SHA-256 of the frames as 16-bit little-endian stereo.
    pub fn hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for s in &self.pcm {
            h.update(s.to_le_bytes());
        }
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Whether another rendering agrees within the faithful tolerance (section 13): the same length,
    /// and the difference at least 40 dB below this one.
    pub fn agrees(&self, other: &Rendering) -> bool {
        if self.pcm.len() != other.pcm.len() {
            return false;
        }
        let (mut signal, mut noise) = (0f64, 0f64);
        for (&x, &y) in self.pcm.iter().zip(&other.pcm) {
            signal += (x as f64) * (x as f64);
            noise += (x as f64 - y as f64) * (x as f64 - y as f64);
        }
        if signal == 0.0 {
            return noise == 0.0;
        }
        noise == 0.0 || 10.0 * (signal / noise).log10() >= 40.0
    }
}

/// Renders arrangement `a` from its start until the note data has ended and every voice is silent.
pub fn render(song: Arc<Song>, a: usize) -> Rendering {
    let limit = song.rate as u64 * 3600;
    render_limited(song, a, limit)
}

/// As `render`, stopping after `limit` frames instead of one hour (`too_long` then set): for a
/// program that wants less than the hour, such as a fuzzer.
pub fn render_limited(song: Arc<Song>, a: usize, limit: u64) -> Rendering {
    render_with(song, a, limit, |_| {})
}

/// As `render_limited`, with `setup` run on the player before it starts: for a host's controls, as
/// a cue's trigger sets them (OMQ section 6).
pub fn render_with(song: Arc<Song>, a: usize, limit: u64, setup: impl FnOnce(&mut Player)) -> Rendering {
    let mut p = Player::new(song);
    setup(&mut p);
    p.record_trace();
    p.once = true;
    p.play(a, 0, 0, Mode::Song);
    let mut pcm = Vec::new();
    // Too long when, with the events before the limit's frame, a voice is sounding or note data
    // is left at it (section 11). The end of the note data is itself an event: until it has
    // happened, before that frame, note data is left. A tick starting at that frame doesn't happen,
    // so the end of the note data there counts as left; but a voice gone after the tick before is
    // gone at that frame.
    let over = |p: &Player| !(p.data_ended && p.silent() && p.mode == Mode::Stopped);
    let too_long;
    loop {
        if p.frame >= limit {
            p.after_tick();
            too_long = over(&p);
            break;
        }
        // One tick at a time, so that the rendering ends on a tick boundary.
        p.start_tick();
        if !over(&p) {
            too_long = false;
            break;
        }
        p.tick += 1;
        p.tick_end = p.tick_start_frame();
        // A tick can outlast the hour (a tick of N/D seconds allows N up to 2^53 - 1): the
        // rendering stops at the hour, inside the tick.
        let n = (p.tick_end.min(limit) - p.frame) as usize;
        let at = pcm.len();
        pcm.resize(at + n * 2, 0);
        p.mix(&mut pcm[at..]);
        p.frame += n as u64;
        if p.frame >= limit && p.frame < p.tick_end {
            // Inside a tick: the end of the note data, if it hasn't happened, is still to come.
            too_long = over(&p);
            break;
        }
    }
    // Events at or after the limit's frame aren't part of the rendering.
    let mut trace = p.trace().unwrap_or_default();
    trace.retain(|e| e.frame < limit);
    Rendering { pcm, trace, too_long, loop_frames: p.loop_frames }
}

#[cfg(test)]
#[path = "player_tests.rs"]
mod tests;

/// A curve test for every named effect; it reads the voices' state, so it lives in this module.
#[cfg(test)]
#[path = "effect_tests.rs"]
mod effect_tests;
