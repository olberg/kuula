//! Playing and rendering a cue (docs/omq.md sections 5 and 6). A tracked cue is OMT's player over
//! the cue's song with the trigger's two host controls set; a plain cue is the same player over a
//! song of one sampled voice.

use std::sync::Arc;

use super::{Bank, Content, Sound, Trigger};
use crate::player::{render_with, sample_step, Event, EventKind, Mode, Player, Rendering};
use crate::song::{Song, MAX_PITCH};

/// A plain cue's voice sounds at its root, 15360, plus the trigger's transposition (section 4).
const ROOT: i32 = 15360;

/// Sets a trigger on a player of a cue's song (section 5): the gain on every channel, and the
/// transposition on the pitched ones.
fn apply(p: &mut Player, pitched: &[bool], t: Trigger) {
    for (c, &follows) in pitched.iter().enumerate() {
        p.set_gain(c, t.gain);
        if follows {
            p.set_transpose(c, t.transpose);
        }
    }
}

/// A cue being played: what a host drives. It is started by a trigger, gives frames as it is asked
/// for them, says when it has ended, and can be stopped. A cue that loops plays until it is stopped.
/// The host also has a gain for each channel the cue takes and can ask whether a channel is silent:
/// its own controls, which no bank carries and no bank's meaning depends on.
pub struct CuePlayer {
    /// `None` for a cue that plays nothing: a plain cue whose file the player doesn't have.
    player: Option<Player>,
    stopped: bool,
    /// The trigger's gain (section 5), held to its limits.
    trigger_gain: i32,
    /// The host's gain for each channel the cue takes, 0 to 256: 256, unity, until the host sets one.
    host_gain: Vec<i32>,
}

impl CuePlayer {
    /// Starts cue number `cue` of `bank` with `trigger` (held to its limits); `None` when the bank
    /// has no such cue. The cue sounds from the first frame rendered.
    pub fn new(bank: &Bank, cue: usize, trigger: Trigger) -> Option<CuePlayer> {
        let c = bank.cues.get(cue)?;
        let t = trigger.clamped();
        let player = match &c.content {
            Content::Tracked { pitched, song, .. } => {
                let mut p = Player::new(song.clone());
                apply(&mut p, pitched, t);
                p.play(0, 0, 0, Mode::Song);
                Some(p)
            }
            Content::Plain { song: Some(song), .. } => Some(plain_player(song, t)),
            Content::Plain { song: None, .. } => None,
        };
        Some(CuePlayer { player, stopped: false, trigger_gain: t.gain, host_gain: vec![256; c.channels()] })
    }

    /// How many channels the cue takes: one for each of a tracked cue's tracks, one for a plain cue
    /// (section 2), whether or not the player has a file to play in it. The channels a host's
    /// `set_gain` and `channel_silent` ask about are numbered from 0 in the order of the cue's `tracks`;
    /// a plain cue's voice is on channel 0.
    pub fn channels(&self) -> usize {
        self.host_gain.len()
    }

    /// The host's gain for one of the cue's channels, 0 to 256 (held to them), where 256 is unity and
    /// what every channel has until it is set. It composes with the trigger's gain as one number,
    /// `(t × h) >> 8`, which is the gain of the channel's voices (section 5: each side of each voice
    /// becomes (x × gain) >> 8 before the voices are summed), so a host's 256 changes nothing. It
    /// applies from the next frame rendered, and to a plain cue's one voice as to a tracked cue's.
    /// A channel the cue doesn't take is ignored.
    pub fn set_gain(&mut self, channel: usize, gain: i32) {
        let Some(h) = self.host_gain.get_mut(channel) else { return };
        *h = gain.clamp(0, 256);
        if let Some(p) = &mut self.player {
            p.set_gain(channel, (self.trigger_gain * *h) >> 8);
        }
    }

    /// Whether no voice of channel `channel` is sounding: not yet, while a tracked cue has not
    /// started its notes on it (a tracked cue's first voices start with the first frames rendered; a
    /// plain cue's voice is sounding from the start), and once it is over. A channel the cue doesn't
    /// take is silent.
    pub fn channel_silent(&self, channel: usize) -> bool {
        channel >= self.channels() || self.player.as_ref().is_none_or(|p| p.channel_silent(channel))
    }

    /// Renders `out.len() / 2` stereo frames, interleaved. After the cue has ended they are silence.
    pub fn render(&mut self, out: &mut [i16]) {
        match &mut self.player {
            Some(p) => p.render(out),
            None => out.fill(0),
        }
    }

    /// Whether the cue has ended: its note data has ended and no voice sounds (a tracked cue), its
    /// file has run out (a plain one), or it was stopped.
    pub fn ended(&self) -> bool {
        self.stopped || self.player.as_ref().is_none_or(|p| p.mode() == Mode::Stopped && p.silent())
    }

    /// Stops the cue as the end of its note data does: its voices are released, and their tails
    /// play out. A voice of a plain cue has no release, so it ends at once.
    pub fn stop(&mut self) {
        if let Some(p) = &mut self.player {
            p.finish();
        }
    }

    /// Stops the cue and silences it at once.
    pub fn cut(&mut self) {
        if let Some(p) = &mut self.player {
            p.stop();
        }
        self.stopped = true;
    }
}

/// A player of a plain cue's song with its voice started.
fn plain_player(song: &Arc<Song>, t: Trigger) -> Player {
    let mut p = Player::new(song.clone());
    p.set_gain(0, t.gain);
    p.start_voice(0, 1, ROOT, (ROOT + t.transpose).clamp(0, MAX_PITCH), 64);
    p
}

/// Renders cue number `cue` of `bank` with `trigger` (section 6), cut at an hour of output. A tracked
/// cue is OMT's rendering of its song with the trigger applied; a plain cue's is its voice's frames
/// from the first, with the two lines of its trace. A cue the bank doesn't have plays nothing.
pub fn render(bank: &Bank, cue: usize, trigger: Trigger) -> Rendering {
    render_limited(bank, cue, trigger, bank.rate as u64 * 3600)
}

/// As `render`, stopping after `limit` frames instead of one hour (`too_long` then set): for a
/// program that wants less than the hour, such as a test.
pub fn render_limited(bank: &Bank, cue: usize, trigger: Trigger, limit: u64) -> Rendering {
    let t = trigger.clamped();
    let nothing = || Rendering { pcm: Vec::new(), trace: Vec::new(), too_long: false, loop_frames: None };
    let Some(c) = bank.cues.get(cue) else { return nothing() };
    match &c.content {
        Content::Tracked { pitched, song, .. } => render_with(song.clone(), 0, limit, |p| apply(p, pitched, t)),
        Content::Plain { sound: Some(sound), song: Some(song), .. } => render_plain(bank.rate, sound, song, t, limit),
        Content::Plain { .. } => nothing(),
    }
}

/// A plain cue's rendering (section 6): `N = ⌈(E << 32) ÷ step⌉` frames, `E` the file's length or
/// its loop's end, and the trace `0 0 on 0 <pitch>` and, for a file without a loop that wasn't cut,
/// `<N> 0 end`. The frames come from the player's own voice, whose position first reaches `E`
/// after exactly that many.
fn render_plain(rate: u32, sound: &Sound, song: &Arc<Song>, t: Trigger, limit: u64) -> Rendering {
    let pitch = (ROOT + t.transpose).clamp(0, MAX_PITCH);
    let step = sample_step(pitch, ROOT, sound.rate, rate).max(1) as u128;
    let end = sound.looping.map_or(sound.frames(), |(_, e)| e) as u128;
    let n = (end << 32).div_ceil(step);
    // The voice is still sounding at the hour when its last frame is at or after it.
    let too_long = n >= limit as u128;
    let frames = if too_long { limit } else { n as u64 };
    let mut p = plain_player(song, t);
    let mut pcm = vec![0i16; frames as usize * 2];
    p.render(&mut pcm);
    let mut trace = vec![Event { frame: 0, channel: 0, kind: EventKind::On, instrument: 0, pitch }];
    if !too_long && sound.looping.is_none() {
        trace.push(Event { frame: frames, channel: 0, kind: EventKind::End, instrument: 0, pitch: 0 });
    }
    Rendering { pcm, trace, too_long, loop_frames: None }
}
