//! Open Module Cues (docs/omq.md): a bank of a game's sound effects, played by the engine that
//! plays OMT. A tracked cue is a few OMT tracks played together, a plain cue one audio file played
//! as it is. The bank is read (`load`), a cue is played by a host (`CuePlayer`) or rendered
//! (`render`), and everything that sounds is OMT's: instruments, samples, tracks, cells and mixing
//! are the song loader's and the player's own.

mod audio;
mod build;
mod load;
mod play;

use std::sync::Arc;

use crate::song::{Diag, Instrument, Resampling, Sample, Song, Track};

pub use load::{load, load_sources, VERSION};
pub use play::{render, render_limited, CuePlayer};

/// The one profile of banks (section 11).
pub const PROFILE_KUULA: &str = "kuula";

/// The OMQ version this engine reads and writes, as (major, minor).
pub const READER: (u32, u32) = (0, 2);

/// Bounds of section 9 that OMT's don't give.
pub const MAX_CUES: usize = 4096;
pub const MAX_CUE_TRACKS: usize = 64;
pub const MAX_NAME: usize = 255;
pub const MAX_CUE_TICKS: i64 = 1 << 24;

/// A trigger (section 5): the transposition and the gain a cue is played with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trigger {
    /// In pitch units, −12288 to 12288.
    pub transpose: i32,
    /// 0 to 256, where 256 is unity.
    pub gain: i32,
}

impl Default for Trigger {
    /// The default trigger plays a cue as written: the one a cue's hash is of.
    fn default() -> Trigger {
        Trigger { transpose: 0, gain: 256 }
    }
}

impl Trigger {
    /// The trigger within its limits: a host's number outside them is held to them.
    pub fn clamped(self) -> Trigger {
        Trigger { transpose: self.transpose.clamp(-12288, 12288), gain: self.gain.clamp(0, 256) }
    }
}

/// A cue's `vary` (section 5): the ranges its variations are taken from, inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vary {
    pub transpose: (i32, i32),
    pub gain: (i32, i32),
}

impl Default for Vary {
    /// No variation: every number gives the default trigger.
    fn default() -> Vary {
        Vary { transpose: (0, 0), gain: (256, 256) }
    }
}

impl Vary {
    /// The trigger of variation number `u` (section 5): the low half of `u` picks the
    /// transposition from its range and the high half the gain, each as
    /// `lo + ((half × (hi − lo + 1)) >> 16)`.
    pub fn trigger(&self, u: u32) -> Trigger {
        let pick = |half: u32, (lo, hi): (i32, i32)| lo + ((half as u64 * (hi - lo + 1) as u64) >> 16) as i32;
        Trigger { transpose: pick(u & 0xffff, self.transpose), gain: pick(u >> 16, self.gain) }
    }
}

/// The audio of a plain cue as decoded: interleaved 16-bit frames of one or two channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sound {
    pub rate: u32,
    pub channels: u32,
    pub pcm: Arc<[i16]>,
    /// The audio entry's loop, `start..end` in frames, when it declares one; it fits the file (a
    /// loop that doesn't makes the file `bad-audio` and the cue play nothing, section 4).
    pub looping: Option<(u32, u32)>,
}

impl Sound {
    pub fn frames(&self) -> u32 {
        (self.pcm.len() / self.channels as usize) as u32
    }
}

/// What a cue plays.
#[derive(Debug, Clone)]
pub enum Content {
    /// A few tracks played together (section 3).
    Tracked {
        /// The track each channel plays, `None` for a reference to a track the bank doesn't have.
        tracks: Vec<Option<usize>>,
        /// Whether each channel follows a trigger's transposition.
        pitched: Vec<bool>,
        /// The cue's length in ticks.
        ticks: u64,
        looping: bool,
        /// The OMT song section 3 gives, with only the cue's own tracks, re-indexed.
        song: Arc<Song>,
    },
    /// One audio file played as it is (section 4).
    Plain {
        /// The `name` of the audio entry.
        audio: String,
        /// The file decoded, when the player has it and decodes it.
        sound: Option<Arc<Sound>>,
        /// The OMT song that plays it: a sampler instrument over a sample made of the file.
        song: Option<Arc<Song>>,
    },
}

#[derive(Debug, Clone)]
pub struct Cue {
    pub name: String,
    /// 0 to 64, and −256 to 256.
    pub volume: i32,
    pub pan: i32,
    pub vary: Vary,
    pub content: Content,
}

impl Cue {
    pub fn is_tracked(&self) -> bool {
        matches!(self.content, Content::Tracked { .. })
    }

    /// The channels the cue takes (section 2): one for each of a tracked cue's tracks, one for a
    /// plain cue.
    pub fn channels(&self) -> usize {
        match &self.content {
            Content::Tracked { tracks, .. } => tracks.len(),
            Content::Plain { .. } => 1,
        }
    }

    /// The trigger of variation number `u` (section 5).
    pub fn varied(&self, u: u32) -> Trigger {
        self.vary.trigger(u)
    }
}

/// A bank that could be read: its shared instruments, samples and tracks, and its cues.
#[derive(Debug, Clone)]
pub struct Bank {
    pub title: String,
    pub rate: u32,
    pub tick: (u64, u64),
    pub volume: i32,
    pub resampling: Resampling,
    /// Indexed by number, as a song's.
    pub instruments: Vec<Option<Arc<Instrument>>>,
    pub samples: Vec<Arc<Sample>>,
    pub tracks: Vec<Track>,
    pub cues: Vec<Cue>,
}

impl Bank {
    /// The index of the cue a game asks for by `name`.
    pub fn cue_named(&self, name: &str) -> Option<usize> {
        self.cues.iter().position(|c| c.name == name)
    }
}

/// What loading a payload found: the bank, when it could be read, and every diagnostic.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub bank: Option<Bank>,
    pub diags: Vec<Diag>,
    /// `exact` or `faithful` (section 7); none for a bank that can't be read.
    pub tier: Option<&'static str>,
    /// A player plays the bank with parts missing: an unknown engine, a missing resource, a sample
    /// or an audio file it can't decode or doesn't have.
    pub degraded: bool,
    /// The profiles the bank fits, whether or not it names one (section 11); none for a bank that
    /// can't be read.
    pub fits: Option<Vec<&'static str>>,
}

impl Loaded {
    pub fn errors(&self) -> impl Iterator<Item = &Diag> {
        self.diags.iter().filter(|d| d.error)
    }

    pub fn is_valid(&self) -> bool {
        self.bank.is_some() && self.errors().next().is_none()
    }
}

/// The validator rules of section 7, the mapping of section 3 and the plain cue of section 4,
/// worked by hand.
#[cfg(test)]
#[path = "omq_tests.rs"]
mod tests;

/// Playing a tracked cue as the OMT song it is, transposition, gain, plain cues and variation.
#[cfg(test)]
#[path = "omq_play_tests.rs"]
mod play_tests;
