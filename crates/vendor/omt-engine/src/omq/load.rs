//! Reading and validating a bank's payload (docs/omq.md sections 1, 2, 7 and 9). The instruments,
//! samples, tracks and cells are read by the song loader, which reports OMT's codes at OMT's paths;
//! this reads the bank's own members and its cues.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{Map, Value};

use super::audio::{classify, sound, File};
use super::build::{default_ticks, plain_song, tracked_song, Shared};
use super::{Bank, Content, Cue, Loaded, Sound, Vary, MAX_CUES, MAX_CUE_TICKS, MAX_CUE_TRACKS, MAX_NAME, PROFILE_KUULA, READER};
use crate::omc::CueAudio;
use crate::song::{
    self, join, load_instruments, load_rate_tick, load_samples, load_tracks, load_volume_resampling, parse_ijson, parse_version,
    reads, tier_of, Ctx, Engine, Kuula, Resource, Source, MAX_PAYLOAD,
};

/// The OMQ version this engine reads (section 1).
pub const VERSION: &str = "0.2";

/// Reads and validates a payload (the plain JSON text) with the song entry's resources and, when
/// the file is at hand, its cue audio: a validator without the file checks no `audio` reference
/// (section 7).
pub fn load(payload: &[u8], resources: &[Resource], audio: Option<&[CueAudio]>) -> Loaded {
    let sources: Vec<Source> = resources.iter().map(|r| r.map_or(Source::Missing, Source::Bytes)).collect();
    load_sources(payload, &sources, audio)
}

/// As `load`, with resources that may come decoded.
pub fn load_sources(payload: &[u8], resources: &[Source], audio: Option<&[CueAudio]>) -> Loaded {
    let mut cx = Ctx { diags: Vec::new(), later: false };
    let (bank, tier, degraded, fits) = match load_bank(&mut cx, payload, resources, audio) {
        Some((bank, tier, degraded, fits)) => (Some(bank), Some(tier), degraded, Some(fits)),
        None => (None, None, false, None),
    };
    let mut diags = cx.diags;
    diags.sort();
    diags.dedup();
    Loaded { bank, diags, tier, degraded, fits }
}

/// A cue as read, before the songs it plays as are made.
struct Raw {
    name: String,
    volume: i32,
    pan: i32,
    vary: Vary,
    kind: Kind,
}

enum Kind {
    Tracked { refs: Vec<Option<usize>>, pitched: Vec<bool>, ticks: Option<u64>, looping: bool },
    Plain { audio: String, sound: Option<Arc<Sound>> },
}

/// What reading the cues found beside the cues.
struct Cues {
    /// The cues, or `None` when the bank can't be read for them (section 7).
    list: Option<Vec<Raw>>,
    /// A plain cue names a file that isn't of one of the two kinds of section 4.
    faithful: bool,
    /// A plain cue plays nothing: no entry, a lost or damaged one, or a file not decoded.
    degraded: bool,
    used_tracks: Vec<bool>,
    /// The frames and channels of each whole file of one of the two kinds that a plain cue names,
    /// once for each audio entry; empty without the file (section 11).
    files: Vec<(u32, u32)>,
}

/// What a cue's `audio` names, resolved once for every cue that names it.
#[derive(Clone)]
enum Res {
    Sound(Arc<Sound>),
    Damaged,
    Unknown,
    Lost,
    Absent,
}

fn load_bank(
    cx: &mut Ctx,
    payload: &[u8],
    resources: &[Source],
    audio: Option<&[CueAudio]>,
) -> Option<(Bank, &'static str, bool, Vec<&'static str>)> {
    if payload.len() > MAX_PAYLOAD {
        cx.error("too-large", "");
        return None;
    }
    let Some(Value::Object(root)) = parse_ijson(payload) else {
        cx.error("not-json", "");
        return None;
    };
    // `omq` is read as OMT's `omt` is (OMT section 1): after it is missing or unknown, nothing else.
    match root.get("omq") {
        None => {
            cx.error("missing-member", "omq");
            return None;
        }
        Some(v) => match v.as_str().and_then(parse_version).and_then(|bank| reads(READER, bank)) {
            Some(later) => cx.later = later,
            None => {
                cx.error("unknown-version", "omq");
                return None;
            }
        },
    }
    cx.members(&root, &["omq", "title", "profile", "rate", "tick", "volume", "resampling", "instruments", "samples", "tracks", "cues"], "");
    let title = cx.string(&root, "title", "");
    let profile = match root.get("profile") {
        None => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => {
            cx.error("bad-value", "profile");
            None
        }
    };
    let (rate, tick) = load_rate_tick(cx, &root);
    let (volume, resampling) = load_volume_resampling(cx, &root);

    let samples = load_samples(cx, &root, resources);
    let (instruments, index, used_samples) = load_instruments(cx, &root, samples.len());
    let tracks = load_tracks(cx, &root, &instruments);
    let cues = load_cues(cx, &root, tracks.len(), audio);

    let (Some(rate), Some(tick), Some(raws)) = (rate, tick, cues.list) else { return None };
    let shared = Shared { rate: rate as u32, tick, volume, resampling, instruments: &instruments, samples: &samples, tracks: &tracks };
    let built: Vec<Cue> = raws.into_iter().map(|r| build(&shared, r)).collect();

    // Whole-bank checks (section 7): what nothing uses, and the tier.
    for (t, used) in cues.used_tracks.iter().enumerate() {
        if !used {
            cx.warn("unused", format!("tracks[{t}]"));
        }
    }
    let mut used_ins = [false; song::MAX_INSTRUMENT as usize + 1];
    for track in &tracks {
        for (_, cell) in &track.cells {
            used_ins[cell.written_ins as usize] = true;
        }
    }
    for (i, ins) in instruments.iter().enumerate() {
        if let Some(ins) = ins
            && !used_ins[i]
        {
            cx.warn("unused", format!("instruments[{}]", index[ins.number as usize]));
        }
    }
    for (s, used) in used_samples.iter().enumerate() {
        if !used {
            cx.warn("unused", format!("samples[{s}]"));
        }
    }
    let tier = if cues.faithful || tier_of(&instruments, &samples, &tracks) == "faithful" { "faithful" } else { "exact" };
    // Degraded (section 7): parts a player plays nothing of for want of them. A resource the file
    // doesn't hold intact, or in an encoding this engine doesn't decode, is one; a resource that is
    // damaged, doesn't match its record or isn't there to name is an error (OMT section 9), not a
    // degradation.
    let missing = |s: &song::Sample| {
        s.data.is_none()
            && (s.encoding == song::Encoding::Opus || matches!(resources.get(s.resource), Some(Source::Missing)))
    };
    let degraded = cues.degraded
        || instruments.iter().flatten().any(|i| matches!(i.engine, Engine::Unknown(_)))
        || samples.iter().any(|s| missing(s));
    // Profiles (section 11): the `kuula` rules, which the bank fits or not whether or not it names the
    // profile, and the promise its `profile` makes.
    let widest = built.iter().map(|c| if let Content::Tracked { tracks, .. } = &c.content { tracks.len() } else { 0 }).max().unwrap_or(0);
    let mut broken = Kuula {
        rate: rate as u32,
        resampling,
        channels: widest,
        instruments: &instruments,
        samples: &samples,
        tracks: &tracks,
        extra_bytes: cues.files.iter().map(|&(frames, channels)| frames as u64 * channels as u64 * 2).sum(),
        exact: tier == "exact",
    }
    .violations();
    if cues.files.iter().any(|&(_, channels)| channels != 1) {
        broken.push("stereo-audio");
    }
    match profile {
        Some(PROFILE_KUULA) => {
            for rule in &broken {
                cx.error(&format!("profile:{rule}"), "");
            }
        }
        Some(_) => cx.warn("unknown-profile", "profile"),
        None => {}
    }
    let fits = if broken.is_empty() { vec![PROFILE_KUULA] } else { Vec::new() };
    let bank = Bank { title, rate: rate as u32, tick, volume, resampling, instruments, samples, tracks, cues: built };
    Some((bank, tier, degraded, fits))
}

/// A cue with the songs it plays as (sections 3 and 4).
fn build(b: &Shared, r: Raw) -> Cue {
    let content = match r.kind {
        Kind::Tracked { refs, pitched, ticks, looping } => {
            let ticks = ticks.unwrap_or_else(|| default_ticks(b.tracks, &refs));
            let song = Arc::new(tracked_song(b, &refs, ticks, looping, r.volume, r.pan));
            Content::Tracked { tracks: refs, pitched, ticks, looping, song }
        }
        Kind::Plain { audio, sound } => {
            let song = sound.as_ref().map(|s| Arc::new(plain_song(b, s, r.volume, r.pan)));
            Content::Plain { audio, sound, song }
        }
    };
    Cue { name: r.name, volume: r.volume, pan: r.pan, vary: r.vary, content }
}

/// The cues, each checked on its own (section 7): `list` is `None` when the bank can't be read for
/// them, but every member is checked either way.
fn load_cues(cx: &mut Ctx, root: &Map<String, Value>, tracks: usize, audio: Option<&[CueAudio]>) -> Cues {
    let mut out = Cues { list: None, faithful: false, degraded: false, used_tracks: vec![false; tracks], files: Vec::new() };
    let Some(list) = cx.array(root, "cues", "", true) else { return out };
    let mut readable = true;
    if list.is_empty() || list.len() > MAX_CUES {
        cx.error("bad-value", "cues");
        readable = false;
    }
    let mut raws = Vec::new();
    let mut names = HashSet::new();
    let mut resolved: HashMap<String, Res> = HashMap::new();
    for (i, v) in list.iter().enumerate() {
        let path = format!("cues[{i}]");
        let Some(o) = cx.object(v, &path) else {
            readable = false;
            continue;
        };
        match load_cue(cx, o, &path, &mut out, &mut names, audio, &mut resolved) {
            Some(raw) => raws.push(raw),
            None => readable = false,
        }
    }
    out.list = readable.then_some(raws);
    out.files = resolved
        .values()
        .filter_map(|r| if let Res::Sound(s) = r { Some((s.frames(), s.channels)) } else { None })
        .collect();
    out
}

/// One cue, or `None` when the bank can't be read for it: no good `name`, neither or both of
/// `tracks` and `audio`, a `tracks` that isn't 1 to 64 entries or an `audio` that isn't a string.
fn load_cue(
    cx: &mut Ctx,
    o: &Map<String, Value>,
    path: &str,
    found: &mut Cues,
    names: &mut HashSet<String>,
    audio: Option<&[CueAudio]>,
    resolved: &mut HashMap<String, Res>,
) -> Option<Raw> {
    cx.members(o, &["name", "tracks", "audio", "ticks", "loop", "volume", "pan", "pitched", "vary"], path);
    let mut readable = true;
    let name = match o.get("name") {
        None => {
            cx.error("missing-member", join(path, "name"));
            readable = false;
            String::new()
        }
        Some(Value::String(s)) if !s.is_empty() && s.len() <= MAX_NAME => {
            if !names.insert(s.clone()) {
                cx.error("duplicate-name", join(path, "name"));
            }
            s.clone()
        }
        Some(_) => {
            cx.error("bad-value", join(path, "name"));
            readable = false;
            String::new()
        }
    };
    // A member that is bad reads as its default (section 7).
    let volume = cx.int(o, "volume", path, 0, 64, Some(64)).unwrap_or(64) as i32;
    let pan = cx.int(o, "pan", path, -256, 256, Some(0)).unwrap_or(0) as i32;
    let vary = load_vary(cx, o, path);
    let (tracks, audio_member) = (o.get("tracks"), o.get("audio"));
    let kind = match (tracks, audio_member) {
        (None, None) => {
            cx.error("missing-member", join(path, "tracks"));
            readable = false;
            load_tracked(cx, o, path, found)
        }
        (Some(_), Some(_)) => {
            cx.error("bad-value", join(path, "audio"));
            readable = false;
            load_tracked(cx, o, path, found)
        }
        (Some(_), None) => load_tracked(cx, o, path, found),
        (None, Some(a)) => {
            // `ticks`, `loop` and `pitched` are a tracked cue's.
            for key in ["ticks", "loop", "pitched"] {
                if o.contains_key(key) {
                    cx.error("bad-value", join(path, key));
                }
            }
            match a {
                Value::String(a) => {
                    let sound = load_audio(cx, a, path, found, audio, resolved);
                    Some(Kind::Plain { audio: a.clone(), sound })
                }
                _ => {
                    cx.error("bad-value", join(path, "audio"));
                    None
                }
            }
        }
    };
    let kind = kind?;
    readable.then_some(Raw { name, volume, pan, vary, kind })
}

/// A tracked cue's `tracks`, `ticks`, `loop` and `pitched`; `None` when `tracks` isn't an array of
/// 1 to 64 entries.
fn load_tracked(cx: &mut Ctx, o: &Map<String, Value>, path: &str, found: &mut Cues) -> Option<Kind> {
    let tp = join(path, "tracks");
    let mut refs: Vec<Option<usize>> = Vec::new();
    let mut listed = None;
    match o.get("tracks") {
        Some(Value::Array(a)) => {
            if !(1..=MAX_CUE_TRACKS).contains(&a.len()) {
                cx.error("bad-value", &tp);
            } else {
                listed = Some(a.len());
            }
            for (k, x) in a.iter().enumerate() {
                match x.as_i64() {
                    Some(i) if i >= 0 && (i as usize) < found.used_tracks.len() => {
                        found.used_tracks[i as usize] = true;
                        refs.push(Some(i as usize));
                    }
                    // A track the bank doesn't have; anything but an index is a bad value.
                    Some(i) if i >= 0 => {
                        cx.error("bad-reference", format!("{tp}[{k}]"));
                        refs.push(None);
                    }
                    _ => {
                        cx.error("bad-value", format!("{tp}[{k}]"));
                        refs.push(None);
                    }
                }
            }
        }
        Some(_) => cx.error("bad-value", &tp),
        None => {}
    }
    let ticks = if o.contains_key("ticks") { cx.int(o, "ticks", path, 1, MAX_CUE_TICKS, None).map(|n| n as u64) } else { None };
    let looping = match o.get("loop") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            cx.error("bad-value", join(path, "loop"));
            false
        }
    };
    let pitched = match o.get("pitched") {
        None => vec![true; refs.len()],
        Some(Value::Array(a)) if a.iter().all(Value::is_boolean) && listed.is_none_or(|n| a.len() == n) => {
            a.iter().map(|b| b.as_bool().unwrap_or(true)).collect()
        }
        Some(_) => {
            cx.error("bad-value", join(path, "pitched"));
            vec![true; refs.len()]
        }
    };
    listed?;
    Some(Kind::Tracked { refs, pitched, ticks, looping })
}

/// A plain cue's file (section 4): what its `audio` names among the file's cue audio, which is not
/// checked without the file. The sound when it plays; the diagnostics and the tier's and the
/// degradation's marks otherwise.
fn load_audio(cx: &mut Ctx, name: &str, path: &str, found: &mut Cues, files: Option<&[CueAudio]>, resolved: &mut HashMap<String, Res>) -> Option<Arc<Sound>> {
    let files = files?;
    let res = resolved
        .entry(name.to_string())
        .or_insert_with(|| match files.iter().find(|f| f.name == name) {
            None => Res::Absent,
            Some(f) => match f.data.map(|d| classify(d, f.loop_start, f.loop_end)) {
                None => Res::Lost,
                Some(File::Damaged) => Res::Damaged,
                Some(File::Unknown) => Res::Unknown,
                Some(File::Sound { rate, channels, pcm }) => Res::Sound(Arc::new(sound(rate, channels, pcm, f.loop_start, f.loop_end))),
            },
        })
        .clone();
    let ap = join(path, "audio");
    match res {
        Res::Sound(s) => return Some(s),
        Res::Absent => cx.error("bad-reference", ap),
        // A file of the kind, damaged, is an error and the cue plays nothing; as OMT's damaged
        // resource it is not a degradation (section 7: that is a chunk damaged or lost).
        Res::Damaged => {
            cx.error("bad-audio", ap);
            return None;
        }
        Res::Unknown => {
            cx.warn("unknown-audio", ap);
            found.faithful = true;
        }
        Res::Lost => {}
    }
    found.degraded = true;
    None
}

/// A cue's `vary` (section 5): each range two integers in order within its limits, else
/// `bad-value` at the member, and the default.
fn load_vary(cx: &mut Ctx, o: &Map<String, Value>, path: &str) -> Vary {
    let default = Vary::default();
    let Some(v) = o.get("vary") else { return default };
    let vp = join(path, "vary");
    let Some(obj) = cx.object(v, &vp) else { return default };
    cx.members(obj, &["transpose", "gain"], &vp);
    let range = |cx: &mut Ctx, key: &str, limit: (i64, i64), default: (i32, i32)| match obj.get(key) {
        None => default,
        Some(x) => match x.as_array().map(|a| a.as_slice()) {
            Some([lo, hi]) => match (lo.as_i64(), hi.as_i64()) {
                (Some(lo), Some(hi)) if limit.0 <= lo && lo <= hi && hi <= limit.1 => (lo as i32, hi as i32),
                _ => {
                    cx.error("bad-value", join(&vp, key));
                    default
                }
            },
            _ => {
                cx.error("bad-value", join(&vp, key));
                default
            }
        },
    };
    Vary { transpose: range(cx, "transpose", (-12288, 12288), default.transpose), gain: range(cx, "gain", (0, 256), default.gain) }
}
