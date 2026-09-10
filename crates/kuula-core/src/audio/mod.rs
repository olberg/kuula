//! The audio subsystem: a mixer rendered in
//! lockstep with the frames. Eight cart channels plus one reserved shell
//! channel, 44.1 kHz mono 16-bit, [`SAMPLES_PER_FRAME`] samples per
//! step. Every operation in the render path is integer or fixed-point so
//! the PCM is bit-identical on every machine, and the mix clips rather
//! than wraps.
//!
//! Audio work is not charged to the cart's cycle budget; the API calls
//! that reach it cost one cycle each in the guest.

pub mod sample;
pub mod synth;
pub mod tables;
pub mod tracker;

mod api;

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use sample::{Sample, SampleBank, SampleVoice};
use synth::{increment, Patch, SynthVoice};
use tracker::{Cell, Cursor, Instrument, Note, Step, Track, GAIN_ONE};

/// Output rate in Hz.
pub const SAMPLE_RATE: u32 = 44_100;
/// Samples per 60 Hz frame.
pub const SAMPLES_PER_FRAME: usize = 735;
/// Channels a cart may address: 0 to 7.
pub const CART_CHANNELS: usize = 8;
/// The shell's reserved channel; a cart cannot name it.
pub const SHELL_CHANNEL: usize = 8;
/// Every channel, cart and shell.
pub const CHANNELS: usize = CART_CHANNELS + 1;
/// Most simultaneous sfx cursors; older ones are dropped first.
const MAX_SFX_CURSORS: usize = CHANNELS;
/// Middle C plays a sample at its native rate.
const SAMPLE_ROOT_NOTE: u8 = 48;

/// Errors from the audio API, with stable codes like `GfxError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    BadChannel {
        channel: i64,
    },
    AssetNotFound {
        path: String,
    },
    AssetInvalid {
        path: String,
        why: String,
    },
    Track {
        path: String,
        line: u32,
        why: String,
    },
    Sample {
        path: String,
        why: String,
    },
    /// The track has more columns than fit from the requested channel.
    NoRoom {
        path: String,
        channel: usize,
        columns: usize,
    },
}

impl AudioError {
    pub fn code(&self) -> &'static str {
        match self {
            AudioError::BadChannel { .. } => "audio_bad_channel",
            AudioError::AssetNotFound { .. } => "asset_not_found",
            AudioError::AssetInvalid { .. } => "asset_invalid",
            AudioError::Track { .. } => "track_error",
            AudioError::Sample { .. } => "sample_error",
            AudioError::NoRoom { .. } => "audio_no_room",
        }
    }
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            AudioError::BadChannel { channel } => {
                write!(f, "channel {channel} is not 0..{CART_CHANNELS}")
            }
            AudioError::AssetNotFound { path } => write!(f, "{path} not found in the cart"),
            AudioError::AssetInvalid { path, why } => write!(f, "{path}: {why}"),
            AudioError::Track { path, line, why } if *line > 0 => {
                write!(f, "{path}:{line}: {why}")
            }
            AudioError::Track { path, why, .. } => write!(f, "{path}: {why}"),
            AudioError::Sample { path, why } => write!(f, "{path}: {why}"),
            AudioError::NoRoom {
                path,
                channel,
                columns,
            } => write!(
                f,
                "{path} has {columns} channels, which do not fit from channel {channel}"
            ),
        }
    }
}

impl std::error::Error for AudioError {}

/// Scale a sample by an 8-bit volume where 255 is exactly unity.
fn scale(s: i32, v: u8) -> i32 {
    if v == 255 {
        s
    } else {
        (s * v as i32) >> 8
    }
}

#[derive(Debug, Clone)]
enum Voice {
    Synth(SynthVoice),
    Sample(SampleVoice),
}

impl Voice {
    fn next_sample(&mut self) -> Option<i32> {
        match self {
            Voice::Synth(v) => v.next_sample(),
            Voice::Sample(v) => v.next_sample(),
        }
    }

    fn is_off(&self) -> bool {
        match self {
            Voice::Synth(v) => v.is_off(),
            Voice::Sample(v) => v.is_off(),
        }
    }
}

#[derive(Debug, Clone)]
struct Channel {
    voice: Option<Voice>,
    /// `volume(channel, v)`, 0..=255.
    gain: u8,
    /// The last `vN` the track set, 0..=255.
    vol: u8,
    /// The owning cursor's fade gain, 0..=256.
    track_gain: i32,
    last_inst: u8,
}

impl Default for Channel {
    fn default() -> Channel {
        Channel {
            voice: None,
            gain: 255,
            vol: 255,
            track_gain: GAIN_ONE,
            last_inst: 1,
        }
    }
}

impl Channel {
    fn is_idle(&self) -> bool {
        self.voice.as_ref().is_none_or(|v| v.is_off())
    }

    fn release(&mut self) {
        if let Some(Voice::Synth(v)) = &mut self.voice {
            v.release();
        } else {
            self.voice = None;
        }
    }
}

/// The mixer: channels, playing cursors, and the cart's decoded audio
/// assets. Owned by `DrawState`; the host owns the master volume.
pub struct Mixer {
    channels: Vec<Channel>,
    master: u8,
    out: Vec<i16>,
    music: Option<Cursor>,
    sfx: Vec<Cursor>,
    tracks: HashMap<String, Rc<Track>>,
    samples: SampleBank,
    /// A frame of PCM rendered elsewhere, output by the next `render`.
    external: Option<Vec<i16>>,
}

impl Default for Mixer {
    fn default() -> Mixer {
        Mixer::new()
    }
}

impl fmt::Debug for Mixer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mixer")
            .field("master", &self.master)
            .field("music", &self.music.as_ref().map(|c| c.row()))
            .field("sfx", &self.sfx.len())
            .finish()
    }
}

impl Mixer {
    pub fn new() -> Mixer {
        Mixer {
            channels: vec![Channel::default(); CHANNELS],
            master: 100,
            out: vec![0; SAMPLES_PER_FRAME],
            music: None,
            sfx: Vec::new(),
            tracks: HashMap::new(),
            samples: SampleBank::default(),
            external: None,
        }
    }

    /// Hand the mixer a frame of PCM rendered in another process. The
    /// next `render` outputs it at the master volume instead of mixing
    /// the channels; it applies to one frame only, so a frame that brings
    /// none mixes as usual. Short input is padded with silence.
    pub fn set_external(&mut self, samples: &[i16]) {
        let mut ext = vec![0; SAMPLES_PER_FRAME];
        let n = samples.len().min(SAMPLES_PER_FRAME);
        ext[..n].copy_from_slice(&samples[..n]);
        self.external = Some(ext);
    }

    /// Master volume, 0..=100, applied after the mix. The host sets it.
    pub fn set_master(&mut self, v: u8) {
        self.master = v.min(100);
    }

    pub fn master(&self) -> u8 {
        self.master
    }

    /// The last rendered frame.
    pub fn output(&self) -> &[i16] {
        &self.out
    }

    /// Silence every channel and drop every cursor. Assets stay cached.
    pub fn stop_all(&mut self) {
        for ch in &mut self.channels {
            ch.voice = None;
        }
        self.music = None;
        self.sfx.clear();
    }

    pub fn is_playing(&self, channel: usize) -> bool {
        self.channels.get(channel).is_some_and(|c| !c.is_idle())
    }

    pub fn music_playing(&self) -> bool {
        self.music.is_some()
    }

    // ----- assets -------------------------------------------------------

    pub fn cached_track(&self, path: &str) -> Option<Rc<Track>> {
        self.tracks.get(path).cloned()
    }

    pub fn cache_track(&mut self, path: &str, track: Track) -> Rc<Track> {
        let rc = Rc::new(track);
        self.tracks.insert(path.to_string(), rc.clone());
        rc
    }

    pub fn samples(&self) -> &SampleBank {
        &self.samples
    }

    pub fn samples_mut(&mut self) -> &mut SampleBank {
        &mut self.samples
    }

    // ----- playing ------------------------------------------------------

    fn check_channel(channel: i64) -> Result<usize, AudioError> {
        if (0..CART_CHANNELS as i64).contains(&channel) {
            Ok(channel as usize)
        } else {
            Err(AudioError::BadChannel { channel })
        }
    }

    /// The highest run of `columns` idle, unowned cart channels, or the
    /// top run regardless when every channel is busy.
    fn pick_channel(&self, columns: usize) -> usize {
        let columns = columns.clamp(1, CART_CHANNELS);
        (0..=CART_CHANNELS - columns)
            .rev()
            .find(|&base| {
                (base..base + columns).all(|i| self.channels[i].is_idle() && !self.cursor_owns(i))
            })
            .unwrap_or(CART_CHANNELS - columns)
    }

    fn cursor_owns(&self, channel: usize) -> bool {
        self.music
            .iter()
            .chain(self.sfx.iter())
            .any(|c| c.channels().contains(&channel))
    }

    /// Drop sfx cursors overlapping `range` and release their notes.
    fn evict_sfx(&mut self, range: std::ops::Range<usize>) {
        let (drop, keep): (Vec<Cursor>, Vec<Cursor>) = std::mem::take(&mut self.sfx)
            .into_iter()
            .partition(|c| c.channels().any(|ch| range.contains(&ch)));
        self.sfx = keep;
        for c in drop {
            for ch in c.channels() {
                self.channels[ch].release();
            }
        }
    }

    /// Start `track` as a sound effect from `channel`, or from a free
    /// channel. Any sfx already on those channels is dropped; music is
    /// left alone but its notes there are cut.
    pub fn play_sfx(
        &mut self,
        path: &str,
        track: Rc<Track>,
        channel: Option<i64>,
    ) -> Result<usize, AudioError> {
        let base = match channel {
            Some(c) => Mixer::check_channel(c)?,
            None => self.pick_channel(track.columns),
        };
        if base + track.columns > CART_CHANNELS {
            return Err(AudioError::NoRoom {
                path: path.to_string(),
                channel: base,
                columns: track.columns,
            });
        }
        let range = base..base + track.columns;
        self.evict_sfx(range.clone());
        for ch in range {
            self.channels[ch].voice = None;
            self.channels[ch].track_gain = GAIN_ONE;
            // Instrument 1 until the track names one.
            self.channels[ch].last_inst = 1;
        }
        if self.sfx.len() >= MAX_SFX_CURSORS {
            self.sfx.remove(0);
        }
        self.sfx.push(Cursor::new(track, base, 0));
        Ok(base)
    }

    /// Start `track` as music on channels 0 to its column count, fading
    /// in over `fade_in` frames. The previous music stops at once.
    pub fn play_music(&mut self, track: Rc<Track>, fade_in: u32) {
        self.stop_music(0);
        let range = 0..track.columns;
        self.evict_sfx(range.clone());
        for ch in range {
            self.channels[ch].voice = None;
            self.channels[ch].last_inst = 1;
        }
        self.music = Some(Cursor::new(track, 0, fade_in));
    }

    /// Fade the music out over `fade` frames (0 stops it now).
    pub fn stop_music(&mut self, fade: u32) {
        match &mut self.music {
            Some(cursor) if fade > 0 => cursor.fade_out(fade),
            Some(cursor) => {
                for ch in cursor.channels() {
                    self.channels[ch].release();
                    self.channels[ch].track_gain = GAIN_ONE;
                }
                self.music = None;
            }
            None => {}
        }
    }

    /// Play a sample once on `channel` (or a free one) at `pitch` (16.16;
    /// `1 << 16` is the file's own rate).
    pub fn play_sample(
        &mut self,
        sample: Rc<Sample>,
        channel: Option<i64>,
        pitch: u32,
    ) -> Result<usize, AudioError> {
        let ch = match channel {
            Some(c) => Mixer::check_channel(c)?,
            None => self.pick_channel(1),
        };
        self.evict_sfx(ch..ch + 1);
        let c = &mut self.channels[ch];
        c.voice = Some(Voice::Sample(SampleVoice::new(sample, pitch)));
        c.vol = 255;
        c.track_gain = GAIN_ONE;
        Ok(ch)
    }

    /// A synth note outside any track, e.g. the shell's UI sounds on
    /// [`SHELL_CHANNEL`]. Any channel index is accepted here; the cart
    /// API checks its own range before calling.
    pub fn play_note(&mut self, channel: usize, patch: Patch, note: u8) {
        if let Some(c) = self.channels.get_mut(channel) {
            c.voice = Some(Voice::Synth(SynthVoice::new(patch, note)));
            c.vol = 255;
            c.track_gain = GAIN_ONE;
        }
    }

    /// Per-channel volume, 0..=255.
    pub fn set_volume(&mut self, channel: i64, gain: u8) -> Result<(), AudioError> {
        let ch = Mixer::check_channel(channel)?;
        self.channels[ch].gain = gain;
        Ok(())
    }

    // ----- rendering ----------------------------------------------------

    fn apply_cell(&mut self, ch: usize, cell: &Cell, track: &Track) {
        if let Some(v) = cell.vol {
            self.channels[ch].vol = v;
        }
        if let Some(i) = cell.inst {
            self.channels[ch].last_inst = i;
        }
        match cell.note {
            Note::None => {}
            Note::Off => self.channels[ch].release(),
            Note::On(note) => {
                let inst = track.instrument(self.channels[ch].last_inst);
                let voice = match inst {
                    Some(Instrument::Synth(patch)) => {
                        Some(Voice::Synth(SynthVoice::new(*patch, note)))
                    }
                    Some(Instrument::Sample(name)) => {
                        self.samples.get(&api::sample_path(name)).map(|s| {
                            let pitch = ((increment(note as i32 * 16) as u64) << 16)
                                / increment(SAMPLE_ROOT_NOTE as i32 * 16) as u64;
                            Voice::Sample(SampleVoice::new(s, pitch as u32))
                        })
                    }
                    None => None,
                };
                self.channels[ch].voice = voice;
            }
        }
    }

    fn step_cursor(&mut self, mut cursor: Cursor) -> Option<Cursor> {
        let step = cursor.tick();
        for ch in cursor.channels() {
            self.channels[ch].track_gain = cursor.gain;
        }
        match step {
            Step::Hold => Some(cursor),
            Step::Row(r) => {
                let track = cursor.track.clone();
                let base = cursor.base;
                for (col, cell) in track.rows[r].iter().enumerate() {
                    self.apply_cell(base + col, cell, &track);
                }
                Some(cursor)
            }
            Step::End => {
                for ch in cursor.channels() {
                    self.channels[ch].release();
                    self.channels[ch].track_gain = GAIN_ONE;
                }
                None
            }
        }
    }

    /// Advance every cursor and voice one frame and mix it.
    pub fn render(&mut self) -> &[i16] {
        if let Some(ext) = self.external.take() {
            let master = self.master as i32;
            for (o, &s) in self.out.iter_mut().zip(ext.iter()) {
                *o = (s as i32 * master / 100).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }
            return &self.out;
        }
        if let Some(music) = self.music.take() {
            self.music = self.step_cursor(music);
        }
        let sfx = std::mem::take(&mut self.sfx);
        for cursor in sfx {
            if let Some(c) = self.step_cursor(cursor) {
                self.sfx.push(c);
            }
        }
        for ch in &mut self.channels {
            if let Some(Voice::Synth(v)) = &mut ch.voice {
                v.tick_frame();
            }
        }
        let master = self.master as i32;
        for i in 0..SAMPLES_PER_FRAME {
            let mut sum: i32 = 0;
            for ch in &mut self.channels {
                let Some(voice) = &mut ch.voice else { continue };
                match voice.next_sample() {
                    Some(s) => {
                        let s = scale(scale(s, ch.vol), ch.gain);
                        sum += (s * ch.track_gain) >> 8;
                    }
                    None => ch.voice = None,
                }
            }
            let v = sum * master / 100;
            self.out[i] = v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        }
        &self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(src: &str) -> Rc<Track> {
        Rc::new(tracker::parse("sfx/t.trk", src.as_bytes()).unwrap())
    }

    #[test]
    fn silence_renders_zeros() {
        let mut m = Mixer::new();
        let out = m.render();
        assert_eq!(out.len(), SAMPLES_PER_FRAME);
        assert!(out.iter().all(|&s| s == 0));
        assert!(m.output().iter().all(|&s| s == 0));
    }

    /// The pinned waveform: a square at A-4, full volume. The phase
    /// increment is 42 852 281 per sample, so the half period at 440 Hz
    /// is 50.1 samples: 51 high, then low until the phase wraps at 101.
    #[test]
    fn a_pulse_note_renders_the_pinned_samples() {
        let mut m = Mixer::new();
        m.play_note(0, Patch::default(), 57);
        let out = m.render().to_vec();
        assert_eq!(&out[..4], &[32767, 32767, 32767, 32767]);
        assert_eq!(&out[49..53], &[32767, 32767, -32767, -32767]);
        assert_eq!(&out[99..103], &[-32767, -32767, 32767, 32767]);
        assert_eq!(out[734], 32767);
        assert_eq!(out.iter().filter(|&&s| s > 0).count(), 385);
        // Half master volume scales the mix last.
        m.set_master(50);
        let out = m.render().to_vec();
        assert_eq!(out[0], 16383);
        assert_eq!(m.master(), 50);
    }

    #[test]
    fn the_mix_clips_rather_than_wrapping() {
        let mut m = Mixer::new();
        for ch in 0..CART_CHANNELS {
            m.play_note(ch, Patch::default(), 57);
        }
        let out = m.render().to_vec();
        assert_eq!(out[0], 32767);
        assert_eq!(out[60], -32768);
        m.set_master(10);
        let out = m.render().to_vec();
        assert_eq!(out[0], 26213, "8 * 32767 / 10");
    }

    #[test]
    fn tracker_rows_advance_at_the_tempo() {
        let t = track("tempo 3\ninst 1 pulse\nC-4 1\n===\nC-5\n");
        let mut m = Mixer::new();
        m.play_sfx("sfx/t.trk", t, Some(2)).unwrap();
        assert!(!m.is_playing(2));
        // Frames 1-3: C-4 sounds; 4-6: off (release 0 means at once);
        // 7-9: C-5; then the track ends and the channel is released.
        let sounding: Vec<bool> = (0..10)
            .map(|_| {
                m.render();
                m.is_playing(2)
            })
            .collect();
        assert_eq!(
            sounding,
            [true, true, true, false, false, false, true, true, true, false]
        );
        assert!(m.sfx.is_empty());
    }

    #[test]
    fn channels_are_checked_and_the_shell_channel_is_out_of_reach() {
        let t = track("C-4\n");
        let mut m = Mixer::new();
        assert_eq!(
            m.play_sfx("sfx/t.trk", t.clone(), Some(8))
                .unwrap_err()
                .code(),
            "audio_bad_channel"
        );
        assert_eq!(m.set_volume(-1, 3).unwrap_err().code(), "audio_bad_channel");
        let wide = track("inst 1 pulse\nC-4 1|C-4 1|C-4 1\n");
        assert_eq!(
            m.play_sfx("sfx/t.trk", wide.clone(), Some(6))
                .unwrap_err()
                .code(),
            "audio_no_room"
        );
        assert_eq!(m.play_sfx("sfx/t.trk", wide, None).unwrap(), 7 - 2);
        assert_eq!(m.play_sfx("sfx/t.trk", t.clone(), None).unwrap(), 4);
        m.play_note(SHELL_CHANNEL, Patch::default(), 60);
        m.render();
        assert!(m.is_playing(SHELL_CHANNEL));
        m.stop_all();
        m.render();
        assert!((0..CHANNELS).all(|c| !m.is_playing(c)));
    }

    #[test]
    fn music_takes_the_low_channels_and_fades() {
        let t = track("tempo 1\ninst 1 pulse\nloop 0\nC-4 1 | E-4 1\n");
        let mut m = Mixer::new();
        m.play_music(t.clone(), 0);
        m.render();
        assert!(m.is_playing(0) && m.is_playing(1) && !m.is_playing(2));
        assert!(m.music_playing());
        let loud = m.render()[10];
        m.stop_music(4);
        m.render();
        let quieter = m.render()[10];
        assert!(quieter.abs() < loud.abs(), "{quieter} vs {loud}");
        m.render();
        m.render();
        assert!(!m.music_playing());
        // Volume is per channel and survives new notes.
        m.set_volume(0, 0).unwrap();
        m.play_music(t, 0);
        let out = m.render().to_vec();
        m.set_volume(1, 0).unwrap();
        let quiet = m.render().to_vec();
        assert!(out.iter().any(|&s| s != 0));
        assert!(quiet.iter().all(|&s| s == 0));
    }

    #[test]
    fn a_new_track_starts_from_instrument_one() {
        // `a` leaves the channel on instrument 3; `b` has no instrument 3
        // and names none in its cell, so it must play its instrument 1.
        let a = track("inst 1 pulse\ninst 3 pulse\nC-4 3\n");
        let b = track("inst 1 pulse\nC-4\n");
        let mut m = Mixer::new();
        m.play_sfx("sfx/a.trk", a.clone(), Some(2)).unwrap();
        m.render();
        assert!(m.is_playing(2));
        m.play_sfx("sfx/b.trk", b.clone(), Some(2)).unwrap();
        m.render();
        assert!(m.is_playing(2), "sfx after a track on instrument 3");
        m.play_sfx("sfx/a.trk", a, Some(0)).unwrap();
        m.render();
        m.play_music(b, 0);
        m.render();
        assert!(m.is_playing(0), "music after a track on instrument 3");
    }
}
