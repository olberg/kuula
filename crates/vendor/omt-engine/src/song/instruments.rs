//! Reading and validating the instruments and the samples of a payload (docs/omt.md sections 8
//! and 9).

use std::sync::Arc;

use serde_json::{Map, Value};

use super::load::{join, Ctx, Source};
use super::*;

/// A step sequence whose steps are integers in [lo, hi] (section 8), or `None` when its steps
/// can't be played. Each member is checked on its own (section 13): `loop` and `release` against
/// the number of steps, or against the most a sequence has when the steps aren't a list of 1 to
/// 1024. `later` is a song of a later minor version, where a `waveform` step out of the range is
/// a waveform this reader doesn't know: `Err` then, the instrument playing nothing (section 1).
fn load_sequence(cx: &mut Ctx, v: &Value, path: &str, lo: i64, hi: i64, later: bool) -> Result<Option<Sequence>, ()> {
    let Some(o) = cx.object(v, path) else { return Ok(None) };
    cx.members(o, &["steps", "loop", "release", "speed"], path);
    let mut unknown = false;
    let (steps, count): (Option<Vec<i32>>, usize) = match o.get("steps") {
        Some(Value::Array(a)) if !a.is_empty() && a.len() <= MAX_STEPS => {
            let ints: Option<Vec<i64>> = a.iter().map(Value::as_i64).collect();
            let in_range = |s: &[i64]| s.iter().all(|&n| n >= lo && n <= hi);
            match ints {
                Some(s) if in_range(&s) => (Some(s.iter().map(|&n| n as i32).collect()), a.len()),
                Some(_) if later => {
                    unknown = true;
                    (None, a.len())
                }
                _ => {
                    cx.error("bad-value", join(path, "steps"));
                    (None, a.len())
                }
            }
        }
        None => {
            cx.error("missing-member", join(path, "steps"));
            (None, MAX_STEPS)
        }
        Some(_) => {
            cx.error("bad-value", join(path, "steps"));
            (None, MAX_STEPS)
        }
    };
    let last = count as i64 - 1;
    let loop_step = cx.int(o, "loop", path, 0, last, Some(-1)).filter(|&n| n >= 0).map(|n| n as usize);
    let release = cx.int(o, "release", path, 0, last, Some(-1)).filter(|&n| n >= 0).map(|n| n as usize);
    let speed = cx.int(o, "speed", path, 1, 255, Some(1)).unwrap_or(1) as u32;
    if unknown {
        return Err(());
    }
    Ok(steps.map(|steps| Sequence { steps, loop_step, release, speed }))
}

/// The `wave` engine (section 8), or `Err` when it names a waveform, or has a `waveform` step, that
/// a later minor version added (`later`): read as an unknown engine (section 1).
fn load_wave(cx: &mut Ctx, o: &Map<String, Value>, path: &str, later: bool) -> Result<Wave, ()> {
    cx.members(o, &["kind", "waveform", "duty", "tables", "second", "nna", "sequences"], path);
    let mut unknown = false;
    let waveform = match o.get("waveform") {
        Some(Value::String(s)) => match WAVEFORMS.iter().find(|w| w.0 == s) {
            Some(w) => w.1,
            None if later => {
                unknown = true;
                Waveform::Pulse
            }
            None => {
                cx.error("bad-value", join(path, "waveform"));
                Waveform::Pulse
            }
        },
        None => {
            cx.error("missing-member", join(path, "waveform"));
            Waveform::Pulse
        }
        Some(_) => {
            cx.error("bad-value", join(path, "waveform"));
            Waveform::Pulse
        }
    };
    let duty = cx.int(o, "duty", path, 0, 255, Some(128)).unwrap_or(128) as i32;
    // The tables, a bad one kept as silence so that the others keep their indexes. Each is checked
    // even when there are too many or none (section 13).
    let tp = join(path, "tables");
    let mut tables: Vec<Vec<i16>> = Vec::new();
    match o.get("tables") {
        None => {}
        Some(Value::Array(list)) => {
            if list.is_empty() || list.len() > MAX_TABLES {
                cx.error("bad-value", &tp);
            }
            for (i, x) in list.iter().enumerate() {
                let t: Option<Vec<i16>> = x.as_array().and_then(|a| {
                    a.iter().map(|x| x.as_i64().filter(|&n| (-32767..=32767).contains(&n)).map(|n| n as i16)).collect()
                });
                match t {
                    Some(t) if t.len() >= 4 && t.len() <= 256 && t.len().is_power_of_two() => tables.push(t),
                    _ => {
                        cx.error("bad-value", format!("{tp}[{i}]"));
                        tables.push(vec![0; 4]);
                    }
                }
            }
        }
        Some(_) => cx.error("bad-value", &tp),
    }
    let mut second = None;
    if let Some(v) = o.get("second") {
        let sp = join(path, "second");
        if let Some(so) = cx.object(v, &sp) {
            cx.members(so, &["transpose", "level"], &sp);
            let transpose = cx.int(so, "transpose", &sp, -12288, 12288, None);
            let level = cx.int(so, "level", &sp, 0, 64, None);
            if let (Some(t), Some(l)) = (transpose, level) {
                second = Some((t as i32, l as i32));
            }
        }
    }
    const WAVE_ACTIONS: [(&str, Action); 3] = [("cut", Action::Cut), ("continue", Action::Continue), ("release", Action::Release)];
    let nna = named(cx, o, "nna", path, &WAVE_ACTIONS, Action::Cut);
    let mut sequences = Sequences::default();
    // The tables a waveform sequence selects: `tables` is required when there is one.
    let mut wants_table = waveform == Waveform::Table(0);
    if let Some(v) = o.get("sequences") {
        let sp = join(path, "sequences");
        if let Some(s) = cx.object(v, &sp) {
            cx.members(s, &["volume", "arpeggio", "pitch", "duty", "waveform"], &sp);
            let mut get = |cx: &mut Ctx, key: &str, lo: i64, hi: i64, later: bool| match s.get(key) {
                None => None,
                Some(v) => load_sequence(cx, v, &join(&sp, key), lo, hi, later).unwrap_or_else(|()| {
                    unknown = true;
                    None
                }),
            };
            sequences.volume = get(cx, "volume", 0, 64, false);
            sequences.arpeggio = get(cx, "arpeggio", -96, 96, false);
            sequences.pitch = get(cx, "pitch", -12288, 12288, false);
            sequences.duty = get(cx, "duty", 0, 255, false);
            sequences.waveform = get(cx, "waveform", 0, 5 + MAX_TABLES as i64, later);
            // The steps the reader knows, even beside a later version's steps (section 1): a
            // known step selecting a missing table stays `bad-value`.
            let known = s.get("waveform").and_then(|w| w.get("steps")).and_then(Value::as_array).map(|a| {
                a.iter().filter_map(Value::as_i64).filter(|&n| (0..=5 + MAX_TABLES as i64).contains(&n)).max().unwrap_or(0)
            });
            if let Some(highest) = known {
                if highest >= 6 {
                    wants_table = true;
                    // A step selecting a table `tables` doesn't have, when `tables` is a list,
                    // whatever else is wrong with it.
                    if o.get("tables").is_some_and(Value::is_array) && highest as usize >= 6 + tables.len() {
                        cx.error("bad-value", join(&sp, "waveform"));
                    }
                }
            }
        }
    }
    if wants_table && !o.contains_key("tables") {
        cx.error("missing-member", &tp);
    }
    if unknown {
        return Err(());
    }
    if tables.is_empty() && waveform == Waveform::Table(0) {
        tables.push(vec![0; 4]);
    }
    Ok(Wave { waveform, duty, tables, second, nna, sequences })
}

/// A point envelope with values in [lo, hi] (section 9), or `None` when its points can't be
/// played. A bad point is reported at `points[k]`, a count outside 1..=32 at `points`; ticks are
/// judged against the highest one so far, and the loops' indexes against the point count (32 when
/// the points are bad).
fn load_points(cx: &mut Ctx, v: &Value, path: &str, lo: i64, hi: i64) -> Option<PointEnvelope> {
    let o = cx.object(v, path)?;
    cx.members(o, &["points", "loop", "sustain"], path);
    let list = cx.array(o, "points", path, true);
    let mut ok = list.is_some();
    let mut count = MAX_POINTS;
    let mut points = Vec::new();
    if let Some(list) = list {
        if (1..=MAX_POINTS).contains(&list.len()) {
            count = list.len();
        } else {
            cx.error("bad-value", join(path, "points"));
            ok = false;
        }
        let mut previous: i64 = -1;
        for (k, x) in list.iter().enumerate() {
            let kp = format!("{path}.points[{k}]");
            let (tick, value) = match x.as_array().map(|a| a.as_slice()) {
                Some([t, v]) => (t.as_i64(), v.as_i64()),
                _ => (None, None),
            };
            // The first point at tick 0, then ticks strictly increasing up to 65535.
            let tick_ok = tick.is_some_and(|t| t > previous && t <= 65535 && (k > 0 || t == 0));
            if !tick_ok {
                cx.error("bad-value", &kp);
            }
            if let Some(t) = tick.filter(|&t| t > previous) {
                previous = t;
            }
            let value = value.filter(|&v| v >= lo && v <= hi);
            if value.is_none() {
                cx.error("bad-value", &kp);
            }
            match (tick, value) {
                (Some(t), Some(v)) if tick_ok => points.push((t as u32, v as i32)),
                _ => ok = false,
            }
        }
    }
    let pair = |cx: &mut Ctx, key: &str| -> Option<(usize, usize)> {
        let v = o.get(key)?;
        let ab = match v.as_array().map(|a| a.as_slice()) {
            Some([a, b]) => a.as_i64().zip(b.as_i64()),
            _ => None,
        };
        match ab {
            Some((a, b)) if a >= 0 && a <= b && (b as usize) < count => Some((a as usize, b as usize)),
            _ => {
                cx.error("bad-value", join(path, key));
                None
            }
        }
    };
    let looping = pair(cx, "loop");
    let sustain = pair(cx, "sustain");
    ok.then_some(PointEnvelope { points, looping, sustain })
}

fn named<T: Copy>(cx: &mut Ctx, o: &Map<String, Value>, key: &str, path: &str, names: &[(&str, T)], default: T) -> T {
    match o.get(key) {
        None => default,
        Some(Value::String(s)) => match names.iter().find(|n| n.0 == s) {
            Some(n) => n.1,
            None => {
                cx.error("bad-value", join(path, key));
                default
            }
        },
        Some(_) => {
            cx.error("bad-value", join(path, key));
            default
        }
    }
}

/// What loading an instrument's engine found beside the engine itself.
#[derive(Default)]
struct EngineMarks {
    /// The samples a sampler names that exist, whether or not it can play.
    used: Vec<usize>,
    /// A filter that is an object.
    filter: bool,
}

/// The sampler engine (section 9). It plays nothing, and counts as an unknown engine, when it has
/// neither `sample` nor `keymap`, both, or names a sample that is bad or doesn't exist.
fn load_sampler(cx: &mut Ctx, e: &Map<String, Value>, path: &str, sample_count: usize) -> (Engine, EngineMarks) {
    cx.members(
        e,
        &["kind", "sample", "keymap", "fadeout", "envelopes", "autovibrato", "nna", "dct", "dca", "filter"],
        path,
    );
    let mut marks = EngineMarks::default();
    let mut playable = true;
    match (e.contains_key("sample"), e.contains_key("keymap")) {
        (true, true) => {
            cx.error("bad-value", join(path, "keymap"));
            playable = false;
        }
        (false, false) => {
            cx.error("missing-member", join(path, "sample"));
            playable = false;
        }
        _ => {}
    }
    let mut sample = None;
    if e.contains_key("sample") {
        match cx.int(e, "sample", path, 0, i64::MAX, None) {
            Some(s) if (s as usize) < sample_count => {
                sample = Some(s as usize);
                marks.used.push(s as usize);
            }
            Some(_) => {
                cx.error("bad-reference", join(path, "sample"));
                playable = false;
            }
            None => playable = false,
        }
    }
    let mut zones: Vec<Zone> = Vec::new();
    if let Some(v) = e.get("keymap") {
        let kp = join(path, "keymap");
        match v.as_array() {
            None => {
                cx.error("bad-value", &kp);
                playable = false;
            }
            Some(list) => {
                if list.len() > MAX_ZONES {
                    cx.error("bad-value", &kp);
                }
                // Every range with good notes, whatever else is wrong with it: a later range
                // overlapping one of them is the one reported.
                let mut ranges: Vec<(i64, i64)> = Vec::new();
                for (k, x) in list.iter().enumerate() {
                    let zp = format!("{kp}[{k}]");
                    let Some([low, high, smp, transpose]) = x.as_array().map(|a| a.as_slice()) else {
                        cx.error("bad-value", &zp);
                        playable = false;
                        continue;
                    };
                    let mut good = true;
                    let smp = match smp.as_i64() {
                        Some(s) if s >= 0 && (s as usize) < sample_count => {
                            marks.used.push(s as usize);
                            s as usize
                        }
                        s => {
                            cx.error(if s.is_some_and(|s| s >= 0) { "bad-reference" } else { "bad-value" }, &zp);
                            playable = false;
                            good = false;
                            0
                        }
                    };
                    let transpose = transpose.as_i64().filter(|t| (-12288..=12288).contains(t));
                    if transpose.is_none() {
                        cx.error("bad-value", &zp);
                        good = false;
                    }
                    let (Some(low), Some(high)) = (low.as_i64(), high.as_i64()) else {
                        cx.error("bad-value", &zp);
                        continue;
                    };
                    if !(12 <= low && low <= high && high <= 131) {
                        cx.error("bad-value", &zp);
                        continue;
                    }
                    if ranges.iter().any(|&(l, h)| low <= h && l <= high) {
                        cx.error("bad-value", &zp);
                        good = false;
                    }
                    ranges.push((low, high));
                    if let (true, Some(t)) = (good, transpose) {
                        zones.push(Zone { low: low as i32, high: high as i32, sample: smp, transpose: t as i32 });
                    }
                }
            }
        }
    }
    let fadeout = cx.int(e, "fadeout", path, 1, 65536, Some(0)).filter(|&f| f > 0).map(|f| f as u32);
    let (mut volume_envelope, mut pan_envelope, mut pitch_envelope) = (None, None, None);
    if let Some(v) = e.get("envelopes") {
        let ep = join(path, "envelopes");
        if let Some(o) = cx.object(v, &ep) {
            cx.members(o, &["volume", "pan", "pitch"], &ep);
            volume_envelope = o.get("volume").and_then(|v| load_points(cx, v, &join(&ep, "volume"), 0, 64));
            pan_envelope = o.get("pan").and_then(|v| load_points(cx, v, &join(&ep, "pan"), -256, 256));
            pitch_envelope = o.get("pitch").and_then(|v| load_points(cx, v, &join(&ep, "pitch"), -12288, 12288));
        }
    }
    let mut autovibrato = None;
    if let Some(v) = e.get("autovibrato") {
        let vp = join(path, "autovibrato");
        if let Some(o) = cx.object(v, &vp) {
            cx.members(o, &["waveform", "depth", "rate", "sweep"], &vp);
            const WAVES: [(&str, VibratoWave); 3] =
                [("sine", VibratoWave::Sine), ("square", VibratoWave::Square), ("ramp", VibratoWave::Ramp)];
            if !o.contains_key("waveform") {
                cx.error("missing-member", join(&vp, "waveform"));
            }
            let waveform = named(cx, o, "waveform", &vp, &WAVES, VibratoWave::Sine);
            let depth = cx.int(o, "depth", &vp, 0, 4096, None).unwrap_or(0) as i32;
            let rate = cx.int(o, "rate", &vp, 0, 255, None).unwrap_or(0) as u32;
            let sweep = cx.int(o, "sweep", &vp, 0, 65535, None).unwrap_or(0) as u32;
            autovibrato = Some(AutoVibrato { waveform, depth, rate, sweep });
        }
    }
    let nna = named(cx, e, "nna", path, &ACTIONS, Action::Cut);
    const CHECKS: [(&str, Duplicate); 4] =
        [("off", Duplicate::Off), ("note", Duplicate::Note), ("sample", Duplicate::Sample), ("instrument", Duplicate::Instrument)];
    let dct = named(cx, e, "dct", path, &CHECKS, Duplicate::Off);
    const DUPLICATE_ACTIONS: [(&str, Action); 3] = [("cut", Action::Cut), ("release", Action::Release), ("fade", Action::Fade)];
    let dca = named(cx, e, "dca", path, &DUPLICATE_ACTIONS, Action::Cut);
    let mut filter = None;
    if let Some(v) = e.get("filter") {
        let fp = join(path, "filter");
        if let Some(o) = cx.object(v, &fp) {
            cx.members(o, &["cutoff", "resonance"], &fp);
            let cutoff = cx.int(o, "cutoff", &fp, 0, 127, None);
            let resonance = cx.int(o, "resonance", &fp, 0, 127, None);
            if let (Some(c), Some(r)) = (cutoff, resonance) {
                filter = Some((c as u8, r as u8));
            }
            marks.filter = true; // a filter the song has, whatever its values, isn't exact
        }
    }
    if !playable {
        return (Engine::Unknown("sampler".into()), marks);
    }
    let engine = Engine::Sampler(Sampler {
        sample,
        zones,
        fadeout,
        volume_envelope,
        pan_envelope,
        pitch_envelope,
        autovibrato,
        nna,
        dct,
        dca,
        filter,
    });
    (engine, marks)
}

/// The instruments by number, each number's index in the payload's array, and which samples the
/// song's samplers name. Every instrument is checked, also one whose number is bad or taken, which
/// the song then leaves out, with the samples it names.
pub(crate) fn load_instruments(cx: &mut Ctx, root: &Map<String, Value>, sample_count: usize) -> (Vec<Option<Arc<Instrument>>>, Vec<usize>, Vec<bool>) {
    let mut out: Vec<Option<Arc<Instrument>>> = vec![None; MAX_INSTRUMENT as usize + 1];
    let mut index = vec![0; MAX_INSTRUMENT as usize + 1];
    let mut used_samples = vec![false; sample_count];
    let Some(list) = cx.array(root, "instruments", "", false) else { return (out, index, used_samples) };
    for (i, v) in list.iter().enumerate() {
        let path = format!("instruments[{i}]");
        let Some(o) = cx.object(v, &path) else { continue };
        cx.members(o, &["number", "name", "volume", "transpose", "pan", "envelope", "engine", "credits"], &path);
        let number = cx.int(o, "number", &path, 1, MAX_INSTRUMENT as i64, None);
        let name = cx.string(o, "name", &path);
        let volume = cx.int(o, "volume", &path, 0, 64, Some(64)).unwrap_or(64) as i32;
        let transpose = cx.int(o, "transpose", &path, -12288, 12288, Some(0)).unwrap_or(0) as i32;
        let pan = if o.contains_key("pan") { cx.int(o, "pan", &path, -256, 256, None).map(|p| p as i32) } else { None };
        let mut envelope = Envelope { attack: 0, decay: 0, sustain: 64, release: 0 };
        if let Some(v) = o.get("envelope") {
            let ep = join(&path, "envelope");
            if let Some(e) = cx.object(v, &ep) {
                cx.members(e, &["attack", "decay", "sustain", "release"], &ep);
                envelope.attack = cx.int(e, "attack", &ep, 0, 60000, Some(0)).unwrap_or(0) as u32;
                envelope.decay = cx.int(e, "decay", &ep, 0, 60000, Some(0)).unwrap_or(0) as u32;
                envelope.sustain = cx.int(e, "sustain", &ep, 0, 64, Some(64)).unwrap_or(64) as u32;
                envelope.release = cx.int(e, "release", &ep, 0, 60000, Some(0)).unwrap_or(0) as u32;
            }
        }
        if let Some(c) = o.get("credits") {
            check_credits(cx, c, &join(&path, "credits"));
        }
        let ep = join(&path, "engine");
        let unknown = || (Engine::Unknown(String::new()), EngineMarks::default());
        let (engine, marks) = match o.get("engine") {
            None => {
                cx.error("missing-member", &ep);
                unknown()
            }
            Some(v) => match cx.object(v, &ep) {
                None => unknown(),
                Some(e) => match e.get("kind") {
                    Some(Value::String(k)) if k == "wave" => match load_wave(cx, e, &ep, cx.later) {
                        Ok(w) => (Engine::Wave(w), EngineMarks::default()),
                        // A waveform of a later minor version: an unknown engine (section 1).
                        Err(()) => {
                            cx.warn("unknown-engine", &ep);
                            (Engine::Unknown(k.clone()), EngineMarks::default())
                        }
                    },
                    Some(Value::String(k)) if k == "sampler" => load_sampler(cx, e, &ep, sample_count),
                    Some(Value::String(k)) => {
                        cx.warn("unknown-engine", &ep);
                        (Engine::Unknown(k.clone()), EngineMarks::default())
                    }
                    _ => {
                        cx.error(if e.contains_key("kind") { "bad-value" } else { "missing-member" }, join(&ep, "kind"));
                        unknown()
                    }
                },
            },
        };
        let Some(number) = number else { continue };
        let slot = &mut out[number as usize];
        if slot.is_some() {
            cx.error("duplicate-number", join(&path, "number"));
            continue;
        }
        *slot = Some(Arc::new(Instrument {
            number: number as u8,
            index: i,
            name,
            volume,
            transpose,
            pan,
            envelope,
            engine,
            filter: marks.filter,
        }));
        index[number as usize] = i;
        for s in marks.used {
            used_samples[s] = true;
        }
    }
    (out, index, used_samples)
}

fn check_credits(cx: &mut Ctx, v: &Value, path: &str) {
    let Some(o) = cx.object(v, path) else { return };
    cx.members(o, &["author", "source", "license"], path);
    for k in ["author", "source", "license"] {
        cx.string(o, k, path);
    }
}

pub(crate) fn load_samples(cx: &mut Ctx, root: &Map<String, Value>, resources: &[Source]) -> Vec<Arc<Sample>> {
    let mut out = Vec::new();
    let Some(list) = cx.array(root, "samples", "", false) else { return out };
    if list.len() > MAX_SAMPLES {
        cx.error("bad-value", "samples");
    }
    for (i, v) in list.iter().enumerate() {
        let path = format!("samples[{i}]");
        let Some(o) = cx.object(v, &path) else {
            out.push(Arc::new(Sample {
                name: String::new(), resource: 0, encoding: Encoding::Pcm16, rate: 44100, channels: 1,
                frames: 1, root: 15360, gain: 64, looping: None, data: None,
            }));
            continue;
        };
        cx.members(o, &["resource", "encoding", "rate", "channels", "frames", "root", "gain", "loop", "name", "credits"], &path);
        let resource = cx.int(o, "resource", &path, 0, i64::MAX, None);
        let encoding = match o.get("encoding") {
            Some(Value::String(s)) if s == "pcm16" => Some(Encoding::Pcm16),
            Some(Value::String(s)) if s == "flac" => Some(Encoding::Flac),
            Some(Value::String(s)) if s == "opus" => Some(Encoding::Opus),
            None => {
                cx.error("missing-member", join(&path, "encoding"));
                None
            }
            Some(_) => {
                cx.error("bad-value", join(&path, "encoding"));
                None
            }
        };
        let mut rate = cx.int(o, "rate", &path, 1000, 384000, None);
        if encoding == Some(Encoding::Opus) && rate.is_some_and(|r| r != 48000) {
            cx.error("bad-value", join(&path, "rate"));
            rate = None;
        }
        let gain = cx.int(o, "gain", &path, 0, 64, Some(64)).unwrap_or(64) as i32;
        let channels = cx.int(o, "channels", &path, 1, 2, None);
        let frames = cx.int(o, "frames", &path, 1, MAX_FRAMES as i64, None);
        let root_pitch = cx.int(o, "root", &path, 0, MAX_PITCH as i64, Some(15360)).unwrap_or(15360) as i32;
        let mut looping = None;
        if let Some(v) = o.get("loop") {
            let lp = join(&path, "loop");
            if let Some(l) = cx.object(v, &lp) {
                cx.members(l, &["mode", "start", "end"], &lp);
                let mode = match l.get("mode") {
                    Some(Value::String(s)) if s == "forward" => Some(LoopMode::Forward),
                    Some(Value::String(s)) if s == "pingpong" => Some(LoopMode::PingPong),
                    None => {
                        cx.error("missing-member", join(&lp, "mode"));
                        None
                    }
                    Some(_) => {
                        cx.error("bad-value", join(&lp, "mode"));
                        None
                    }
                };
                let f = frames.unwrap_or(MAX_FRAMES as i64);
                let start = cx.int(l, "start", &lp, 0, f - 1, None);
                let end = cx.int(l, "end", &lp, 1, f, None);
                match (mode, start, end) {
                    (Some(mode), Some(s), Some(e)) if s < e => looping = Some(Loop { mode, start: s as u32, end: e as u32 }),
                    (Some(_), Some(_), Some(_)) => cx.error("bad-value", &lp),
                    _ => {}
                }
            }
        }
        if let Some(c) = o.get("credits") {
            check_credits(cx, c, &join(&path, "credits"));
        }
        let mut data = None;
        if let (Some(r), Some(enc), Some(ch), Some(fr)) = (resource, encoding, channels, frames) {
            match resources.get(r as usize) {
                None => cx.error("bad-reference", join(&path, "resource")),
                Some(Source::Missing) => {}
                Some(Source::Pcm(pcm)) => {
                    if pcm.len() as u64 == fr as u64 * ch as u64 {
                        data = Some(pcm.clone());
                    } else {
                        cx.error("sample-mismatch", &path);
                    }
                }
                Some(Source::Bytes(bytes)) => {
                    let decoded = match enc {
                        Encoding::Pcm16 => (bytes.len() as u64 == fr as u64 * ch as u64 * 2).then(|| {
                            bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect::<Vec<i16>>()
                        }),
                        Encoding::Flac => crate::flac::decode(bytes, ch as u32, fr as u32),
                        // This engine doesn't decode Opus, only checks the stream's headers: without
                        // PCM from its program, a matching sample is missing, not wrong.
                        Encoding::Opus => {
                            if !crate::ogg::matches(bytes, ch as u32, fr as u32) {
                                cx.error("sample-mismatch", &path);
                            }
                            None
                        }
                    };
                    match decoded {
                        Some(d) => data = Some(Arc::from(d)),
                        None if enc == Encoding::Opus => {}
                        None => cx.error("sample-mismatch", &path),
                    }
                }
            }
        }
        out.push(Arc::new(Sample {
            name: cx.string(o, "name", &path),
            resource: resource.unwrap_or(0) as usize,
            encoding: encoding.unwrap_or(Encoding::Pcm16),
            rate: rate.unwrap_or(44100) as u32,
            channels: channels.unwrap_or(1) as u32,
            frames: frames.unwrap_or(1) as u32,
            root: root_pitch,
            gain,
            looping,
            data,
        }));
    }
    out
}
