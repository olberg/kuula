//! The audio subsystem: a mixer rendered in lockstep with the frames.
//! Eight channels, 44.1 kHz stereo 16-bit, [`SAMPLES_PER_FRAME`] sample
//! frames per step, played from Open Module Track songs, the cues of Open
//! Module Cues banks and PCM samples.
//! Every operation in the render path is integer arithmetic so the PCM is
//! bit-identical on every machine, and the mix clips rather than wraps.
//!
//! Audio work is not charged to the cart's cycle budget; the API calls
//! that reach it cost one cycle each in the guest.

pub mod cues;
pub mod sample;
pub mod songs;

mod api;
#[cfg(test)]
mod cue_tests;
#[cfg(test)]
mod load_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod testsong;
#[cfg(test)]
mod version_tests;

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

pub use api::Version;
pub use omt_engine::omq::Trigger;
use omt_engine::omq::{Bank, CuePlayer};
use omt_engine::player::{Mode, Player};
use omt_engine::song::Role;

use cues::LoadedBank;
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
/// Where the console's variation numbers start.
pub const VARIATION_SEED: u32 = 0x2545_F491;
/// Bytes of a cue's name an error quotes: the longest name a bank can
/// give a cue.
const CUE_NAME: usize = 255;

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
    /// No channel was named, and every run of the sound's length has a
    /// channel the music keeps.
    NoFreeRun {
        path: String,
        channels: usize,
    },
    /// The bank has no cue of that name.
    CueNotFound {
        path: String,
        name: String,
    },
    /// The song has no arrangement of that name or number.
    VersionNotFound {
        path: String,
        /// The version as the cart gave it: a quoted name, or a number.
        version: String,
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
            AudioError::NoRoom { .. } | AudioError::NoFreeRun { .. } => "audio_no_room",
            AudioError::CueNotFound { .. } => "cue_not_found",
            AudioError::VersionNotFound { .. } => "version_not_found",
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
            AudioError::NoFreeRun { path, channels } => write!(
                f,
                "{path} has {channels} channels, and every run of that many has a channel the music keeps; music_channels(n) makes it keep fewer"
            ),
            AudioError::CueNotFound { path, name } => write!(f, "{path} has no cue {name:?}"),
            AudioError::VersionNotFound { path, version } => {
                write!(f, "{path} has no version {version}")
            }
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
    /// Nothing of it has been rendered since it started, or since it
    /// switched version: the tick it is at has not sounded yet.
    fresh: bool,
    /// The version it was switched from, its voices released and ringing
    /// out under the version that plays; gone once they are silent.
    tail: Option<Player>,
}

impl Music {
    fn role(&self, channel: usize) -> Role {
        self.player.song().channels[channel].role
    }

    /// Before a frame of `player`, the music or its tail: a channel held
    /// by an effect, a cue or a sample, or reserved for them by the song,
    /// is silent, and each channel has the console's gain.
    fn prepare(
        player: &mut Player,
        channels: usize,
        held: &[Option<Hold>; CART_CHANNELS],
        gains: &[u16; CART_CHANNELS],
    ) {
        for c in 0..channels {
            let reserved = player.song().channels[c].role == Role::Reserved;
            player.set_mute(c, held[c].is_some() || reserved);
            player.set_gain(c, gains[c] as i32);
        }
    }
}

/// What a sound that holds channels is to the sounds that come after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hold {
    /// Its number among the sounds started, the first being 1.
    born: u64,
    /// The cart named its channel: it keeps its channels until it ends.
    kept: bool,
}

/// A song playing as an effect, holding `channels` channels from `base`.
struct Effect {
    player: Player,
    base: usize,
    channels: usize,
    hold: Hold,
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
    hold: Hold,
}

/// A cue of a bank playing, holding `channels` channels from `base`: one
/// for each track of a tracked cue, one for a plain cue.
struct Cue {
    player: CuePlayer,
    base: usize,
    channels: usize,
    hold: Hold,
}

impl Cue {
    fn holds(&self, channel: usize) -> bool {
        (self.base..self.base + self.channels).contains(&channel)
    }
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
    cues: Vec<Cue>,
    voices: Vec<SampleSound>,
    /// The last variation number given to a cue.
    variation: u32,
    /// How many of its channels, from 0 up, the music keeps to itself.
    music_keeps: usize,
    /// Effects, cues and samples started so far.
    started: u64,
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
            .field("cues", &self.cues.len())
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
            cues: Vec::new(),
            voices: Vec::new(),
            variation: VARIATION_SEED,
            music_keeps: CART_CHANNELS,
            started: 0,
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

    /// Silence everything and drop every song, cue and sample voice.
    /// Loaded assets stay cached and the channel gains stay.
    pub fn stop_all(&mut self) {
        self.music = None;
        self.effects.clear();
        self.cues.clear();
        self.voices.clear();
    }

    /// Whether something sounds on `channel`: an effect, a cue or a sample
    /// voice holding it, or the music's voice there when nothing holds it.
    pub fn is_playing(&self, channel: usize) -> bool {
        if channel >= CART_CHANNELS {
            return false;
        }
        if let Some(e) = self.effects.iter().find(|e| e.holds(channel)) {
            return !e.player.channel_silent(channel - e.base);
        }
        if let Some(c) = self.cues.iter().find(|c| c.holds(channel)) {
            return !c.player.channel_silent(channel - c.base);
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

    pub fn cached_cues(&self, path: &str) -> Option<Arc<Bank>> {
        self.songs.cues(path)
    }

    /// Keep a bank [`cues::load`] accepted, charging both budgets.
    pub fn add_cues(&mut self, path: &str, loaded: &LoadedBank) -> Arc<Bank> {
        self.samples.charge(loaded.sample_bytes);
        self.songs
            .insert_cues(path, &loaded.bank, loaded.song_bytes);
        loaded.bank.clone()
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

    /// `music_channels(n)`: the music keeps its first `n` channels to
    /// itself, held to 0..=8; `None` is all of them, as a cart starts.
    pub fn set_music_channels(&mut self, n: Option<i64>) {
        self.music_keeps = n.map_or(CART_CHANNELS, |n| n.clamp(0, CART_CHANNELS as i64) as usize);
    }

    /// Which channels an effect, a cue or a sample holds.
    #[cfg(test)]
    fn busy(&self) -> [bool; CART_CHANNELS] {
        self.held().map(|h| h.is_some())
    }

    /// What holds each channel: an effect, a cue or a sample.
    fn held(&self) -> [Option<Hold>; CART_CHANNELS] {
        let mut held = [None; CART_CHANNELS];
        for e in &self.effects {
            held[e.base..e.base + e.channels].fill(Some(e.hold));
        }
        for c in &self.cues {
            held[c.base..c.base + c.channels].fill(Some(c.hold));
        }
        for v in &self.voices {
            held[v.channel] = Some(v.hold);
        }
        held
    }

    /// The music's voice on `channel`, if its song plays there.
    fn music_on(&self, channel: usize) -> Option<&Music> {
        self.music
            .as_ref()
            .filter(|m| channel < m.channels && m.role(channel) == Role::Music)
    }

    /// Whether `channel` is one the music keeps to itself.
    fn music_keeps_channel(&self, channel: usize) -> bool {
        channel < self.music_keeps && self.music_on(channel).is_some()
    }

    /// Whether some run of `channels` has no channel the music keeps: a
    /// sound of that length is then kept out only by sounds placed by name.
    fn music_leaves_a_run(&self, channels: usize) -> bool {
        (0..=CART_CHANNELS - channels)
            .any(|base| !(base..base + channels).any(|c| self.music_keeps_channel(c)))
    }

    /// The first channel of the run of `channels` that a sound without a
    /// named channel takes, if any run is open to it. A run with a channel
    /// the music keeps, or one held by a sound placed by name, is not. Of
    /// the others the lowest class is taken, a run's class being its worst
    /// channel's: the highest run of a class below 3, and of class 3 the
    /// run whose newest sound is the oldest, the highest when two are as
    /// old.
    fn pick_run(&self, channels: usize) -> Option<usize> {
        let held = self.held();
        // A channel's class and, for class 3, the number of the sound
        // holding it; `None` for a channel that is kept.
        let class = |c: usize| -> Option<(u8, u64)> {
            if self.music_keeps_channel(c) {
                return None;
            }
            match (held[c], self.music_on(c)) {
                (Some(hold), _) if hold.kept => None,
                (Some(hold), _) => Some((3, hold.born)),
                (None, None) => Some((0, 0)),
                (None, Some(m)) if m.player.channel_silent(c) => Some((1, 0)),
                (None, Some(_)) => Some((2, 0)),
            }
        };
        let mut best: Option<((u8, u64), usize)> = None;
        for base in (0..=CART_CHANNELS - channels).rev() {
            // The worst class, and with it the newest of the sounds held.
            let worst = (base..base + channels)
                .try_fold((0, 0), |worst: (u8, u64), c| class(c).map(|k| worst.max(k)));
            let Some(rank) = worst else {
                continue;
            };
            if best.is_none_or(|(least, _)| rank < least) {
                best = Some((rank, base));
            }
        }
        best.map(|(_, base)| base)
    }

    /// Cut every effect, cue and sample holding a channel of `range`:
    /// nothing of them is rendered again.
    fn cut(&mut self, range: std::ops::Range<usize>) {
        self.effects.retain(|e| !range.clone().any(|c| e.holds(c)));
        self.cues.retain(|c| !range.clone().any(|ch| c.holds(ch)));
        self.voices.retain(|v| !range.contains(&v.channel));
    }

    /// The first channel of a sound of `channels` channels, named by the
    /// cart or picked, with what the sound is to later ones; or the error
    /// for one that does not fit. `None` is a sound that named no channel
    /// and is not played: the music leaves it a run, and sounds placed by
    /// name hold a channel of every one. Nothing is counted for a sound
    /// that is refused or not played.
    fn place(
        &mut self,
        what: &str,
        channels: usize,
        channel: Option<i64>,
    ) -> Result<Option<(usize, Hold)>, AudioError> {
        let base = match channel {
            Some(c) => {
                let base = Mixer::check_channel(c)?;
                if base + channels > CART_CHANNELS {
                    return Err(AudioError::NoRoom {
                        path: what.to_string(),
                        channel: base,
                        channels,
                    });
                }
                base
            }
            None => match self.pick_run(channels) {
                Some(base) => base,
                None if self.music_leaves_a_run(channels) => return Ok(None),
                None => {
                    return Err(AudioError::NoFreeRun {
                        path: what.to_string(),
                        channels,
                    })
                }
            },
        };
        self.started += 1;
        let hold = Hold {
            born: self.started,
            kept: channel.is_some(),
        };
        Ok(Some((base, hold)))
    }

    fn player(song: &PlayableSong) -> Player {
        let mut player = Player::new(song.song.clone());
        player.play(song.arrangement, 0, 0, Mode::Song);
        player
    }

    /// Start `song` as an effect from `channel`, or from the best free run.
    /// Effects and samples holding a channel of the run are cut. Returns the
    /// first channel taken, or `None` for an effect that named no channel
    /// and is not played, sounds placed by name being in its way.
    pub fn play_effect(
        &mut self,
        path: &str,
        song: &PlayableSong,
        channel: Option<i64>,
    ) -> Result<Option<usize>, AudioError> {
        let channels = song.song.channels.len().min(CART_CHANNELS);
        let Some((base, hold)) = self.place(path, channels, channel)? else {
            return Ok(None);
        };
        self.cut(base..base + channels);
        self.effects.push(Effect {
            player: Mixer::player(song),
            base,
            channels,
            hold,
        });
        Ok(Some(base))
    }

    /// A name a cart gave, as an error quotes it: at most the longest name
    /// a cue can have.
    fn quoted(name: &str) -> &str {
        let mut end = name.len().min(CUE_NAME);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        &name[..end]
    }

    /// The error for a cue name `bank` does not have.
    pub fn no_cue(path: &str, name: &str) -> AudioError {
        AudioError::CueNotFound {
            path: path.to_string(),
            name: Mixer::quoted(name).to_string(),
        }
    }

    /// The error for a version the song at `path` does not have.
    pub fn no_version(path: &str, version: &Version) -> AudioError {
        AudioError::VersionNotFound {
            path: path.to_string(),
            version: match version {
                Version::Name(name) => format!("{:?}", Mixer::quoted(name)),
                Version::Number(n) => n.to_string(),
            },
        }
    }

    /// The next variation number: a 32-bit xorshift from
    /// [`VARIATION_SEED`], so every run of a cart varies its cues alike.
    fn next_variation(&mut self) -> u32 {
        let mut x = self.variation;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.variation = x;
        x
    }

    /// Trigger cue number `cue` of `bank` from `channel`, or from the best
    /// free run: one channel for each track of a tracked cue, one for a
    /// plain cue. `trigger` is the transposition and the gain the cart
    /// gave; without one the cue is varied by the console's next variation
    /// number. Effects, cues and samples holding a channel of the run are
    /// cut. Returns the first channel taken, or `None` as
    /// [`Mixer::play_effect`] does.
    pub fn play_cue(
        &mut self,
        path: &str,
        bank: &Arc<Bank>,
        cue: usize,
        trigger: Option<Trigger>,
        channel: Option<i64>,
    ) -> Result<Option<usize>, AudioError> {
        let c = &bank.cues[cue];
        let channels = c.channels();
        let what = format!("{path}: cue {:?}", c.name);
        let Some((base, hold)) = self.place(&what, channels, channel)? else {
            return Ok(None);
        };
        // Nothing can fail from here on, so a call that is refused, or
        // whose cue is not played, steps nothing.
        let trigger = match trigger {
            Some(t) => t,
            None => c.varied(self.next_variation()),
        };
        self.cut(base..base + channels);
        if let Some(player) = CuePlayer::new(bank, cue, trigger) {
            self.cues.push(Cue {
                player,
                base,
                channels,
                hold,
            });
        }
        Ok(Some(base))
    }

    /// End what holds `channel`. A cue or an effect ends as its own note
    /// data would end it: its voices are released, their tails play out and
    /// it holds its channels until they are silent. A plain cue has no
    /// release and a sample has none, so they end at once. With `cut`,
    /// whatever holds the channel is cut. The music holds no channel.
    pub fn stop(&mut self, channel: i64, cut: bool) -> Result<(), AudioError> {
        let ch = Mixer::check_channel(channel)?;
        if cut {
            self.cut(ch..ch + 1);
            return Ok(());
        }
        for e in self.effects.iter_mut().filter(|e| e.holds(ch)) {
            e.player.finish();
        }
        for c in self.cues.iter_mut().filter(|c| c.holds(ch)) {
            c.player.stop();
        }
        self.voices.retain(|v| v.channel != ch);
        Ok(())
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
            fresh: true,
            tail: None,
        });
    }

    /// Play arrangement `version` of `song` as the music. When `song` is
    /// the music now, has not ended and is not fading out, the music
    /// switches to that version where it is: it goes on at the tick after
    /// the one that was sounding, in the same order row, its fade as it
    /// was, and `fade` is not read. The voices of the version it leaves
    /// are released and ring out under it. A position the version does
    /// not have starts it at its beginning, and the version that is
    /// playing goes on untouched. Otherwise the song starts, as
    /// [`Mixer::play_music`] starts it.
    pub fn play_music_version(&mut self, song: &PlayableSong, version: usize, fade: u32) {
        if let Some(m) = &mut self.music {
            if Arc::ptr_eq(m.player.song(), &song.song)
                && m.player.mode() != Mode::Stopped
                && !matches!(m.fade, Fade::Out(_))
            {
                if m.player.arrangement_index() != version {
                    // The engine's position is the tick that is sounding;
                    // before anything is rendered, the one that will.
                    let (order, sounding) = m.player.position();
                    let tick = if m.fresh { sounding } else { sounding + 1 };
                    // A row's length is a place too: the engine goes on
                    // from it to the row after.
                    let there = song.song.arrangements[version]
                        .orders
                        .get(order)
                        .is_some_and(|o| tick <= o.ticks);
                    let (order, tick) = if there { (order, tick) } else { (0, 0) };
                    let mut next = Player::new(song.song.clone());
                    next.play(version, order, tick, Mode::Song);
                    // The version it leaves ends as its note data would
                    // end it. A tail still ringing from a switch before
                    // this one is cut: one version rings out at a time.
                    let mut left = std::mem::replace(&mut m.player, next);
                    left.finish();
                    m.tail = Some(left);
                    m.fresh = true;
                }
                return;
            }
        }
        self.play_music(
            &PlayableSong {
                song: song.song.clone(),
                arrangement: version,
            },
            fade,
        );
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
    /// is cut. Returns the channel, or `None` as [`Mixer::play_effect`]
    /// does.
    pub fn play_sample(
        &mut self,
        path: &str,
        sample: Rc<Sample>,
        channel: Option<i64>,
        pitch: u32,
    ) -> Result<Option<usize>, AudioError> {
        let Some((ch, hold)) = self.place(path, 1, channel)? else {
            return Ok(None);
        };
        self.cut(ch..ch + 1);
        self.voices.push(SampleSound {
            voice: SampleVoice::new(sample, pitch),
            channel: ch,
            hold,
        });
        Ok(Some(ch))
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
            Music::prepare(&mut m.player, m.channels, &held, &self.gains);
            m.player.render(&mut self.scratch);
            m.fresh = false;
            let g = m.gain as i32;
            for (acc, &s) in self.sum.iter_mut().zip(self.scratch.iter()) {
                *acc += (s as i32 * g) >> 8;
            }
            // The version it was switched from, ringing out on the same
            // channels at the same gains.
            if let Some(tail) = &mut m.tail {
                Music::prepare(tail, m.channels, &held, &self.gains);
                tail.render(&mut self.scratch);
                for (acc, &s) in self.sum.iter_mut().zip(self.scratch.iter()) {
                    *acc += (s as i32 * g) >> 8;
                }
                if tail.silent() {
                    m.tail = None;
                }
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

        // The cues: the console's gain for each channel a cue takes is the
        // host's gain of that channel of the cue, which the engine makes
        // one number with the trigger's.
        for c in &mut self.cues {
            for i in 0..c.channels {
                c.player.set_gain(i, self.gains[c.base + i] as i32);
            }
            c.player.render(&mut self.scratch);
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
        self.cues.retain(|c| !c.player.ended());
        self.voices.retain(|v| !v.voice.is_off());
        &self.out
    }
}
