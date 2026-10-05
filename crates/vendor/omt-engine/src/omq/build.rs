//! The OMT songs a cue plays as (docs/omq.md sections 3 and 4), so that the player and the
//! rendering that play OMT play a bank unchanged.

use std::sync::Arc;

use super::Sound;
use crate::song::{
    Action, Arrangement, Channel, Duplicate, Encoding, Engine, Envelope, Instrument, Loop, LoopMode, Next, Order, Resampling,
    Role, Sample, Sampler, Song, Track, MAX_INSTRUMENT,
};

/// What a bank gives every song of its cues.
pub(super) struct Shared<'a> {
    pub rate: u32,
    pub tick: (u64, u64),
    pub volume: i32,
    pub resampling: Resampling,
    pub instruments: &'a [Option<Arc<Instrument>>],
    pub samples: &'a [Arc<Sample>],
    pub tracks: &'a [Track],
}

/// A tracked cue's length when it has no `ticks` (section 3): its longest track, the greatest
/// `rows` × `speed` among the tracks it names, a looping track counted once through. 1 when it
/// names none that exists.
pub(super) fn default_ticks(tracks: &[Track], refs: &[Option<usize>]) -> u64 {
    refs.iter().flatten().map(|&t| tracks[t].ticks()).max().unwrap_or(1)
}

/// The OMT song of a tracked cue (section 3): the bank's rate, tick, volume, resampling,
/// instruments and samples; one channel for each track the cue names, with the cue's `volume` and
/// `pan`; and arrangement 0 of one order row, its tracks the cue's, its ticks the cue's and its
/// `next` 0 when the cue loops and `stop` when it doesn't. Only the tracks the cue names are in
/// the song, so that its track indexes aren't the bank's: a track named twice is there once.
pub(super) fn tracked_song(b: &Shared, refs: &[Option<usize>], ticks: u64, looping: bool, volume: i32, pan: i32) -> Song {
    let mut tracks: Vec<Track> = Vec::new();
    let mut taken: Vec<usize> = Vec::new();
    let order: Vec<Option<usize>> = refs
        .iter()
        .map(|r| {
            r.map(|t| match taken.iter().position(|&x| x == t) {
                Some(i) => i,
                None => {
                    taken.push(t);
                    tracks.push(b.tracks[t].clone());
                    tracks.len() - 1
                }
            })
        })
        .collect();
    let channels = refs.iter().map(|_| Channel { name: String::new(), role: Role::Music, volume, pan }).collect();
    Song {
        title: String::new(),
        profile: None,
        rate: b.rate,
        tick: b.tick,
        ticks_per_beat: None,
        volume: b.volume,
        resampling: b.resampling,
        channels,
        instruments: b.instruments.to_vec(),
        samples: b.samples.to_vec(),
        tracks,
        arrangements: vec![arrangement(order, ticks, if looping { Next::Order(0) } else { Next::Stop })],
    }
}

fn arrangement(tracks: Vec<Option<usize>>, ticks: u64, next: Next) -> Arrangement {
    Arrangement { name: String::new(), orders: vec![Order { tracks, ticks, next }], global: Vec::new(), global_entries: 0 }
}

/// The OMT song of a plain cue (section 4): one channel with the cue's `volume` and `pan`, and one
/// sampler instrument, number 1 and otherwise OMT's defaults, over one sample made of the file:
/// its `rate` and `channels` the file's, its `frames` the file's length, its `root` 15360 and its
/// `gain` 64, with a forward loop when the audio entry declares one. A voice of it, started at
/// pitch 15360 plus the trigger's transposition, is the voice section 4 describes.
pub(super) fn plain_song(b: &Shared, sound: &Sound, volume: i32, pan: i32) -> Song {
    let sample = Sample {
        name: String::new(),
        resource: 0,
        encoding: Encoding::Pcm16,
        rate: sound.rate,
        channels: sound.channels,
        frames: sound.frames(),
        root: 15360,
        gain: 64,
        looping: sound.looping.map(|(start, end)| Loop { mode: LoopMode::Forward, start, end }),
        data: Some(sound.pcm.clone()),
    };
    let sampler = Sampler {
        sample: Some(0),
        zones: Vec::new(),
        fadeout: None,
        volume_envelope: None,
        pan_envelope: None,
        pitch_envelope: None,
        autovibrato: None,
        nna: Action::Cut,
        dct: Duplicate::Off,
        dca: Action::Cut,
        filter: None,
    };
    let instrument = Instrument {
        number: 1,
        index: 0,
        name: String::new(),
        volume: 64,
        transpose: 0,
        pan: None,
        envelope: Envelope { attack: 0, decay: 0, sustain: 64, release: 0 },
        engine: Engine::Sampler(sampler),
        filter: false,
    };
    let mut instruments = vec![None; MAX_INSTRUMENT as usize + 1];
    instruments[1] = Some(Arc::new(instrument));
    Song {
        title: String::new(),
        profile: None,
        rate: b.rate,
        tick: b.tick,
        ticks_per_beat: None,
        volume: b.volume,
        resampling: b.resampling,
        channels: vec![Channel { name: String::new(), role: Role::Music, volume, pan }],
        instruments,
        samples: vec![Arc::new(sample)],
        tracks: Vec::new(),
        arrangements: vec![arrangement(vec![None], 1, Next::Stop)],
    }
}
