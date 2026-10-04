//! The audio subsystem: a mixer rendered in lockstep with the frames.
//! Eight channels, 44.1 kHz stereo 16-bit, [`SAMPLES_PER_FRAME`] sample
//! frames per step, played from Open Module Track songs and PCM samples.
//! Every operation in the render path is integer arithmetic so the PCM is
//! bit-identical on every machine, and the mix clips rather than wraps.
//!
//! Audio work is not charged to the cart's cycle budget; the API calls
//! that reach it cost one cycle each in the guest.

pub mod sample;
pub mod songs;

mod api;
#[cfg(test)]
mod load_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod testsong;

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use omt_engine::player::{Mode, Player};
use omt_engine::song::Role;

use sample::{Sample, SampleBank, SampleVoice};
use songs::{LoadedSong, PlayableSong, SongBank};

/// Output rate in Hz.
pub const SAMPLE_RATE: u32 = 44_100;
/// Sample frames per 60 Hz frame.
pub const SAMPLES_PER_FRAME: usize = 735;
/// Channels of the output: left, then right.
pub const OUTPUT_CHANNELS: usize = 2;
/// `i16` values a frame's PCM holds, the left one of each pair first.
pub const VALUES_PER_FRAME: usize = SAMPLES_PER_FRAME * OUTPUT_CHANNELS;
/// Channels a cart may address: 0 to 7.
pub const CART_CHANNELS: usize = 8;
/// Gain of one channel or of the music's fade: 256 is unity.
pub const GAIN_ONE: u16 = 256;
/// Bytes of its reason a refused load keeps.
pub const REFUSAL_REASON: usize = 240;

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
    Song {
        path: String,
        why: String,
    },
    Sample {
        path: String,
        why: String,
    },
    /// The song has more channels than fit from the requested channel.
    NoRoom {
        path: String,
        channel: usize,
        channels: usize,
    },
}

impl AudioError {
    pub fn code(&self) -> &'static str {
        match self {
            AudioError::BadChannel { .. } => "audio_bad_channel",
            AudioError::AssetNotFound { .. } => "asset_not_found",
            AudioError::AssetInvalid { .. } => "asset_invalid",
            AudioError::Song { .. } => "song_error",
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
            AudioError::Song { path, why } => write!(f, "{path}: {why}"),
            AudioError::Sample { path, why } => write!(f, "{path}: {why}"),
            AudioError::NoRoom {
                path,
                channel,
                channels,
            } => write!(
                f,
                "{path} has {channels} channels, which do not fit from channel {channel}"
            ),
        }
    }
}

impl std::error::Error for AudioError {}

/// The step of a fade over `frames` frames: 256 / `frames`, at least 1.
fn fade_step(frames: u32) -> u16 {
    (GAIN_ONE as u32 / frames.max(1)).max(1) as u16
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fade {
    None,
    In(u16),
    Out(u16),
}

/// The song playing as music, on channels 0 upward.
struct Music {
    player: Player,
    /// Console channels it takes: one per channel of its song.
    channels: usize,
    /// The fade gain, 0..=256, constant within a frame.
    gain: u16,
    fade: Fade,
}

impl Music {
    fn role(&self, channel: usize) -> Role {
        self.player.song().channels[channel].role
    }
}

/// A song playing as an effect, holding `channels` channels from `base`.
struct Effect {
    player: Player,
    base: usize,
    channels: usize,
}

impl Effect {
    fn holds(&self, channel: usize) -> bool {
        (self.base..self.base + self.channels).contains(&channel)
    }
}

/// A sample playing once on one channel.
struct SampleSound {
    voice: SampleVoice,
    channel: usize,
}

/// The mixer: the playing songs and samples, the channel gains and the
/// cart's decoded audio assets. Owned by `DrawState`; the host owns the
/// master volume.
pub struct Mixer {
    master: u8,
    /// `volume(channel, v)` as 0..=256, kept through whatever plays.
    gains: [u16; CART_CHANNELS],
    out: Vec<i16>,
    /// The frame's sum before the master volume.
    sum: Vec<i32>,
    /// One player's frame.
    scratch: Vec<i16>,
    music: Option<Music>,
    effects: Vec<Effect>,
    voices: Vec<SampleSound>,
    songs: SongBank,
    samples: SampleBank,
    /// The files a load refused, by path, with the error each got.
    refused: HashMap<String, AudioError>,
    /// A frame of PCM rendered elsewhere, output by the next `render`.
    external: Vec<i16>,
    has_external: bool,
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
            .field("music", &self.music.is_some())
            .field("effects", &self.effects.len())
            .field("samples", &self.voices.len())
            .finish()
    }
}

impl Mixer {
    pub fn new() -> Mixer {
        Mixer {
            master: 100,
            gains: [GAIN_ONE; CART_CHANNELS],
            out: vec![0; VALUES_PER_FRAME],
            sum: vec![0; VALUES_PER_FRAME],
            scratch: vec![0; VALUES_PER_FRAME],
            music: None,
            effects: Vec::new(),
            voices: Vec::new(),
            songs: SongBank::default(),
            samples: SampleBank::default(),
            refused: HashMap::new(),
            external: vec![0; VALUES_PER_FRAME],
            has_external: false,
        }
    }

    /// Hand the mixer a frame of PCM rendered in another process. The
    /// next `render` outputs it at the master volume instead of mixing
    /// the channels; it applies to one frame only, so a frame that brings
    /// none mixes as usual. Short input is padded with silence.
    pub fn set_external(&mut self, samples: &[i16]) {
        let n = samples.len().min(VALUES_PER_FRAME);
        self.external[..n].copy_from_slice(&samples[..n]);
        self.external[n..].fill(0);
        self.has_external = true;
    }

    /// Master volume, 0..=100, applied after the mix. The host sets it.
    pub fn set_master(&mut self, v: u8) {
        self.master = v.min(100);
    }

    pub fn master(&self) -> u8 {
        self.master
    }

    /// The last rendered frame: [`VALUES_PER_FRAME`] values, left first.
    pub fn output(&self) -> &[i16] {
        &self.out
    }

    /// Silence everything and drop every song and sample voice. Loaded
    /// assets stay cached and the channel gains stay.
    pub fn stop_all(&mut self) {
        self.music = None;
        self.effects.clear();
        self.voices.clear();
    }

    /// Whether something sounds on `channel`: an effect or sample voice
    /// holding it, or the music's voice there when no effect holds it.
    pub fn is_playing(&self, channel: usize) -> bool {
        if channel >= CART_CHANNELS {
            return false;
        }
        if let Some(e) = self.effects.iter().find(|e| e.holds(channel)) {
            return !e.player.channel_silent(channel - e.base);
        }
        if self.voices.iter().any(|v| v.channel == channel) {
            return self
                .voices
                .iter()
                .any(|v| v.channel == channel && !v.voice.is_off());
        }
        self.music
            .as_ref()
            .is_some_and(|m| channel < m.channels && !m.player.channel_silent(channel))
    }

    pub fn music_playing(&self) -> bool {
        self.music.is_some()
    }

    // ----- assets -------------------------------------------------------

    pub fn cached_song(&self, path: &str) -> Option<PlayableSong> {
        self.songs.get(path)
    }

    /// Song bytes still free in the song budget.
    pub fn song_room(&self) -> usize {
        self.songs.room()
    }

    /// Keep a song [`songs::load`] accepted, charging both budgets.
    pub fn add_song(&mut self, path: &str, song: &LoadedSong) -> PlayableSong {
        self.samples.charge(song.sample_bytes);
        self.songs.insert(path, song)
    }

    pub fn songs(&self) -> &SongBank {
        &self.songs
    }

    pub fn samples(&self) -> &SampleBank {
        &self.samples
    }

    pub fn samples_mut(&mut self) -> &mut SampleBank {
        &mut self.samples
    }

    /// The error the file at `path` was refused with, if a load refused it.
    pub fn refused(&self, path: &str) -> Option<AudioError> {
        self.refused.get(path).cloned()
    }

    /// Remember that the file at `path` was refused, and return the error
    /// as it is kept. The budgets only fill, so a refusal stands for the
    /// cart's life, and a cart asking again must not make the console
    /// read the file again. One entry per file of the cart at most, each
    /// with a reason of at most [`REFUSAL_REASON`] bytes.
    pub fn refuse(&mut self, path: &str, mut error: AudioError) -> AudioError {
        if let AudioError::Song { why, .. } | AudioError::Sample { why, .. } = &mut error {
            let mut end = why.len().min(REFUSAL_REASON);
            while !why.is_char_boundary(end) {
                end -= 1;
            }
            why.truncate(end);
        }
        self.refused.insert(path.to_string(), error.clone());
        error
    }

    // ----- playing ------------------------------------------------------

    fn check_channel(channel: i64) -> Result<usize, AudioError> {
        if (0..CART_CHANNELS as i64).contains(&channel) {
            Ok(channel as usize)
        } else {
            Err(AudioError::BadChannel { channel })
        }
    }

    /// Which channels an effect or a sample holds.
    fn held(&self) -> [bool; CART_CHANNELS] {
        let mut held = [false; CART_CHANNELS];
        for e in &self.effects {
            held[e.base..e.base + e.channels].fill(true);
        }
        for v in &self.voices {
            held[v.channel] = true;
        }
        held
    }

    /// The first channel of the run of `channels` that an effect without a
    /// named channel takes: the highest run of the lowest class, where a
    /// run's class is its worst channel's.
    fn pick_run(&self, channels: usize) -> usize {
        let held = self.held();
        let class = |c: usize| -> u8 {
            if held[c] {
                return 3;
            }
            match &self.music {
                Some(m) if c < m.channels && m.role(c) == Role::Music => {
                    if m.player.channel_silent(c) {
                        1
                    } else {
                        2
                    }
                }
                _ => 0,
            }
        };
        let mut best = (u8::MAX, 0);
        for base in (0..=CART_CHANNELS - channels).rev() {
            let worst = (base..base + channels).map(class).max().unwrap_or(0);
            if worst < best.0 {
                best = (worst, base);
            }
        }
        best.1
    }

    /// Cut every effect and sample holding a channel of `range`: nothing of
    /// them is rendered again.
    fn cut(&mut self, range: std::ops::Range<usize>) {
        self.effects.retain(|e| !range.clone().any(|c| e.holds(c)));
        self.voices.retain(|v| !range.contains(&v.channel));
    }

    fn player(song: &PlayableSong) -> Player {
        let mut player = Player::new(song.song.clone());
        player.play(song.arrangement, 0, 0, Mode::Song);
        player
    }

    /// Start `song` as an effect from `channel`, or from the best free run.
    /// Effects and samples holding a channel of the run are cut. Returns the
    /// first channel taken.
    pub fn play_effect(
        &mut self,
        path: &str,
        song: &PlayableSong,
        channel: Option<i64>,
    ) -> Result<usize, AudioError> {
        let channels = song.song.channels.len().min(CART_CHANNELS);
        let base = match channel {
            Some(c) => Mixer::check_channel(c)?,
            None => self.pick_run(channels),
        };
        if base + channels > CART_CHANNELS {
            return Err(AudioError::NoRoom {
                path: path.to_string(),
                channel: base,
                channels,
            });
        }
        self.cut(base..base + channels);
        self.effects.push(Effect {
            player: Mixer::player(song),
            base,
            channels,
        });
        Ok(base)
    }

    /// Start `song` as music on channels 0 upward, fading in over `fade`
    /// frames. The previous music is cut at once; effects and samples keep
    /// their channels.
    pub fn play_music(&mut self, song: &PlayableSong, fade: u32) {
        let (gain, fade) = if fade > 0 {
            (0, Fade::In(fade_step(fade)))
        } else {
            (GAIN_ONE, Fade::None)
        };
        self.music = Some(Music {
            player: Mixer::player(song),
            channels: song.song.channels.len().min(CART_CHANNELS),
            gain,
            fade,
        });
    }

    /// Fade the music out over `fade` frames (0 stops it now).
    pub fn stop_music(&mut self, fade: u32) {
        if fade == 0 {
            self.music = None;
        } else if let Some(m) = &mut self.music {
            m.fade = Fade::Out(fade_step(fade));
        }
    }

    /// Play a sample once on `channel` (or the best free one) at `pitch`
    /// (16.16; `1 << 16` is the file's own rate). Whatever held the channel
    /// is cut.
    pub fn play_sample(
        &mut self,
        sample: Rc<Sample>,
        channel: Option<i64>,
        pitch: u32,
    ) -> Result<usize, AudioError> {
        let ch = match channel {
            Some(c) => Mixer::check_channel(c)?,
            None => self.pick_run(1),
        };
        self.cut(ch..ch + 1);
        self.voices.push(SampleSound {
            voice: SampleVoice::new(sample, pitch),
            channel: ch,
        });
        Ok(ch)
    }

    /// Per-channel gain, 0..=256 with 256 as unity.
    pub fn set_volume(&mut self, channel: i64, gain: u16) -> Result<(), AudioError> {
        let ch = Mixer::check_channel(channel)?;
        self.gains[ch] = gain.min(GAIN_ONE);
        Ok(())
    }

    // ----- rendering ----------------------------------------------------

    /// Advance everything one frame and mix it.
    pub fn render(&mut self) -> &[i16] {
        if self.has_external {
            self.has_external = false;
            let master = self.master as i32;
            for (o, &s) in self.out.iter_mut().zip(self.external.iter()) {
                *o = (s as i32 * master / 100).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }
            return &self.out;
        }
        self.sum.fill(0);

        // The music: its fade step, then its voices, every channel held by
        // an effect or a sample or reserved for effects left out.
        let held = self.held();
        if let Some(m) = &mut self.music {
            match m.fade {
                Fade::In(step) => {
                    m.gain = (m.gain + step).min(GAIN_ONE);
                    if m.gain == GAIN_ONE {
                        m.fade = Fade::None;
                    }
                }
                Fade::Out(step) => m.gain = m.gain.saturating_sub(step),
                Fade::None => {}
            }
        }
        if self
            .music
            .as_ref()
            .is_some_and(|m| m.gain == 0 && matches!(m.fade, Fade::Out(_)))
        {
            self.music = None;
        }
        if let Some(m) = &mut self.music {
            for (c, (&held, &gain)) in held.iter().zip(&self.gains).enumerate() {
                if c < m.channels {
                    m.player.set_mute(c, held || m.role(c) == Role::Reserved);
                    m.player.set_gain(c, gain as i32);
                }
            }
            m.player.render(&mut self.scratch);
            let g = m.gain as i32;
            for (acc, &s) in self.sum.iter_mut().zip(self.scratch.iter()) {
                *acc += (s as i32 * g) >> 8;
            }
        }

        // The effects: every channel of the song sounds.
        for e in &mut self.effects {
            for i in 0..e.channels {
                e.player.set_gain(i, self.gains[e.base + i] as i32);
            }
            e.player.render(&mut self.scratch);
            for (acc, &s) in self.sum.iter_mut().zip(self.scratch.iter()) {
                *acc += s as i32;
            }
        }

        // The samples, in the centre.
        for v in &mut self.voices {
            let g = self.gains[v.channel] as i32;
            for pair in self.sum.as_chunks_mut::<OUTPUT_CHANNELS>().0 {
                let Some(s) = v.voice.next_sample() else {
                    break;
                };
                let s = (s * g) >> 8;
                pair[0] += s;
                pair[1] += s;
            }
        }

        let master = self.master as i32;
        for (o, &s) in self.out.iter_mut().zip(self.sum.iter()) {
            *o = (s * master / 100).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        }

        // What ended this frame frees its channels from the next one.
        let finished = |p: &Player| p.mode() == Mode::Stopped && p.silent();
        if self.music.as_ref().is_some_and(|m| finished(&m.player)) {
            self.music = None;
        }
        self.effects.retain(|e| !finished(&e.player));
        self.voices.retain(|v| !v.voice.is_off());
        &self.out
    }
}
