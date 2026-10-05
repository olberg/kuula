//! Reading and validating a payload (docs/omt.md section 13): the song as a whole, its channels,
//! tracks and arrangements. The instruments and samples are read in `instruments.rs`.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Map, Value};

use super::ijson::parse_ijson;
use super::instruments::{load_instruments, load_samples};
use super::profile::profile_violations;
use super::*;
use crate::cell::{self, CellError, Note};

/// What reading a payload has found so far; the bank loader (OMQ) reads its instruments, samples
/// and tracks with the same one.
pub(crate) struct Ctx {
    pub(crate) diags: Vec<Diag>,
    /// The song's minor version is above the reader's (section 1).
    pub(crate) later: bool,
}

impl Ctx {
    pub(crate) fn error(&mut self, code: &str, path: impl Into<String>) {
        self.diags.push(Diag { code: code.to_string(), path: path.into(), error: true });
    }

    pub(crate) fn warn(&mut self, code: &str, path: impl Into<String>) {
        self.diags.push(Diag { code: code.to_string(), path: path.into(), error: false });
    }

    pub(crate) fn members(&mut self, obj: &Map<String, Value>, known: &[&str], path: &str) {
        for key in obj.keys() {
            if !known.contains(&key.as_str()) {
                self.warn("unknown-member", join(path, key));
            }
        }
    }

    /// An integer member within [lo, hi]: `default` when absent (`None` for a required member).
    pub(crate) fn int(&mut self, obj: &Map<String, Value>, key: &str, path: &str, lo: i64, hi: i64, default: Option<i64>) -> Option<i64> {
        match obj.get(key) {
            None => {
                if default.is_none() {
                    self.error("missing-member", join(path, key));
                }
                default
            }
            Some(v) => match v.as_i64() {
                Some(n) if n >= lo && n <= hi => Some(n),
                _ => {
                    self.error("bad-value", join(path, key));
                    None
                }
            },
        }
    }

    pub(crate) fn string(&mut self, obj: &Map<String, Value>, key: &str, path: &str) -> String {
        match obj.get(key) {
            None => String::new(),
            Some(Value::String(s)) => s.clone(),
            Some(_) => {
                self.error("bad-value", join(path, key));
                String::new()
            }
        }
    }

    pub(crate) fn object<'a>(&mut self, v: &'a Value, path: &str) -> Option<&'a Map<String, Value>> {
        let o = v.as_object();
        if o.is_none() {
            self.error("bad-value", path);
        }
        o
    }

    pub(crate) fn array<'a>(&mut self, obj: &'a Map<String, Value>, key: &str, path: &str, required: bool) -> Option<&'a Vec<Value>> {
        match obj.get(key) {
            None => {
                if required {
                    self.error("missing-member", join(path, key));
                }
                None
            }
            Some(Value::Array(a)) => Some(a),
            Some(_) => {
                self.error("bad-value", join(path, key));
                None
            }
        }
    }
}

pub(crate) fn join(path: &str, key: &str) -> String {
    if path.is_empty() { key.to_string() } else { format!("{path}.{key}") }
}

/// One resource of the song entry: its bytes, or `None` when it is missing or damaged.
pub type Resource<'a> = Option<&'a [u8]>;

/// A resource as a program that already holds the decoded sample can give it.
#[derive(Debug, Clone)]
pub enum Source<'a> {
    Missing,
    Bytes(&'a [u8]),
    /// Interleaved 16-bit frames, whatever the record's encoding.
    Pcm(Arc<[i16]>),
}

/// Reads and validates a payload (the plain JSON text).
pub fn load(payload: &[u8], resources: &[Resource]) -> Loaded {
    let sources: Vec<Source> = resources.iter().map(|r| r.map_or(Source::Missing, Source::Bytes)).collect();
    load_sources(payload, &sources)
}

/// As `load`, with resources that may come decoded.
pub fn load_sources(payload: &[u8], resources: &[Source]) -> Loaded {
    load_as(payload, resources, READER)
}

/// As `load_sources`, as a reader of version `reader` would: for testing the version rule of
/// section 1 with versions this engine isn't.
pub(crate) fn load_as(payload: &[u8], resources: &[Source], reader: (u32, u32)) -> Loaded {
    let mut cx = Ctx { diags: Vec::new(), later: false };
    let song = load_song(&mut cx, payload, resources, reader);
    let mut diags = cx.diags;
    if let Some(song) = &song {
        if let Some(p) = &song.profile {
            match profile_violations(song, p) {
                None => diags.push(Diag { code: "unknown-profile".into(), path: "profile".into(), error: false }),
                Some(v) => diags.extend(v.into_iter().map(|rule| Diag {
                    code: format!("profile:{rule}"),
                    path: String::new(),
                    error: true,
                })),
            }
        }
    }
    diags.sort();
    diags.dedup();
    Loaded { song, diags }
}

fn load_song(cx: &mut Ctx, payload: &[u8], resources: &[Source], reader: (u32, u32)) -> Option<Song> {
    if payload.len() > MAX_PAYLOAD {
        cx.error("too-large", "");
        return None;
    }
    let Some(Value::Object(root)) = parse_ijson(payload) else {
        cx.error("not-json", "");
        return None;
    };
    match root.get("omt") {
        None => {
            cx.error("missing-member", "omt");
            return None;
        }
        Some(v) => match v.as_str().and_then(parse_version).and_then(|song| reads(reader, song)) {
            Some(later) => cx.later = later,
            None => {
                cx.error("unknown-version", "omt");
                return None;
            }
        },
    }
    cx.members(
        &root,
        &["omt", "title", "profile", "rate", "tick", "ticksPerBeat", "volume", "resampling", "channels",
          "instruments", "samples", "tracks", "arrangements"],
        "",
    );
    let title = cx.string(&root, "title", "");
    let profile = match root.get("profile") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => {
            cx.error("bad-value", "profile");
            None
        }
    };
    let (rate, tick) = load_rate_tick(cx, &root);
    let ticks_per_beat = cx.int(&root, "ticksPerBeat", "", 1, i64::MAX, Some(0)).map(|n| n as u64).filter(|&n| n > 0);
    let (volume, resampling) = load_volume_resampling(cx, &root);

    let channels = load_channels(cx, &root);
    let samples = load_samples(cx, &root, resources);
    let (instruments, index, used_samples) = load_instruments(cx, &root, samples.len());
    let tracks = load_tracks(cx, &root, &instruments);
    let arrangements = load_arrangements(cx, &root, channels.as_ref().map_or(0, |c| c.len()), tracks.len(), rate);

    let (Some(rate), Some(tick), Some(channels), Some(arrangements)) = (rate, tick, channels, arrangements) else {
        return None;
    };
    let song = Song {
        title,
        profile,
        rate: rate as u32,
        tick,
        ticks_per_beat,
        volume,
        resampling,
        channels,
        instruments,
        samples,
        tracks,
        arrangements,
    };
    check_song(cx, &song, &index, &used_samples);
    Some(song)
}

/// The `rate` and the `tick` of a payload (sections 1 and 2), each `None` when missing or bad.
pub(crate) fn load_rate_tick(cx: &mut Ctx, root: &Map<String, Value>) -> (Option<i64>, Option<(u64, u64)>) {
    let rate = cx.int(root, "rate", "", 8000, 192000, None);
    let tick = match root.get("tick") {
        None => {
            cx.error("missing-member", "tick");
            None
        }
        Some(Value::Array(a)) if a.len() == 2 => {
            let n = a[0].as_i64().filter(|&n| n >= 1);
            let d = a[1].as_i64().filter(|&d| d >= 1);
            match (n, d, rate) {
                (Some(n), Some(d), Some(r)) if n as u128 * r as u128 >= d as u128 => Some((n as u64, d as u64)),
                (Some(n), Some(d), None) => Some((n as u64, d as u64)),
                _ => {
                    cx.error("bad-value", "tick");
                    None
                }
            }
        }
        Some(_) => {
            cx.error("bad-value", "tick");
            None
        }
    };
    (rate, tick)
}

/// The `volume` and the `resampling` of a payload (section 1), a bad one read as its default.
pub(crate) fn load_volume_resampling(cx: &mut Ctx, root: &Map<String, Value>) -> (i32, Resampling) {
    let volume = cx.int(root, "volume", "", 0, 1024, Some(256)).unwrap_or(256) as i32;
    let resampling = match root.get("resampling") {
        None => Resampling::Nearest,
        Some(Value::String(s)) if s == "nearest" => Resampling::Nearest,
        Some(Value::String(s)) if s == "linear" => Resampling::Linear,
        Some(_) => {
            cx.error("bad-value", "resampling");
            Resampling::Nearest
        }
    };
    (volume, resampling)
}

fn load_channels(cx: &mut Ctx, root: &Map<String, Value>) -> Option<Vec<Channel>> {
    let list = cx.array(root, "channels", "", true)?;
    if list.is_empty() || list.len() > MAX_CHANNELS {
        cx.error("bad-value", "channels");
        return None;
    }
    let mut out = Vec::new();
    for (i, v) in list.iter().enumerate() {
        let path = format!("channels[{i}]");
        let Some(o) = cx.object(v, &path) else {
            out.push(Channel { name: String::new(), role: Role::Music, volume: 64, pan: 0 });
            continue;
        };
        cx.members(o, &["name", "role", "volume", "pan"], &path);
        let role = match o.get("role") {
            None => Role::Music,
            Some(Value::String(s)) if s == "music" => Role::Music,
            Some(Value::String(s)) if s == "reserved" => Role::Reserved,
            Some(_) => {
                cx.error("bad-value", join(&path, "role"));
                Role::Music
            }
        };
        out.push(Channel {
            name: cx.string(o, "name", &path),
            role,
            volume: cx.int(o, "volume", &path, 0, 64, Some(64)).unwrap_or(64) as i32,
            pan: cx.int(o, "pan", &path, -256, 256, Some(0)).unwrap_or(0) as i32,
        });
    }
    Some(out)
}

pub(crate) fn load_tracks(cx: &mut Ctx, root: &Map<String, Value>, instruments: &[Option<Arc<Instrument>>]) -> Vec<Track> {
    let mut out = Vec::new();
    let Some(list) = cx.array(root, "tracks", "", false) else { return out };
    if list.len() > MAX_TRACKS {
        cx.error("bad-value", "tracks");
    }
    for (t, v) in list.iter().enumerate() {
        let path = format!("tracks[{t}]");
        let empty = Track { name: String::new(), rows: 1, written_rows: 1, speed: 1, loop_row: None, cells: Vec::new() };
        let Some(o) = cx.object(v, &path) else {
            out.push(empty);
            continue;
        };
        cx.members(o, &["name", "rows", "speed", "loop", "cells"], &path);
        let rows = cx.int(o, "rows", &path, 1, MAX_ROWS as i64, None).unwrap_or(1) as u32;
        let written_rows = o.get("rows").and_then(Value::as_i64).unwrap_or(rows as i64);
        let speed = cx.int(o, "speed", &path, 1, 255, None).unwrap_or(1) as u32;
        let loop_row = cx.int(o, "loop", &path, 0, rows as i64 - 1, Some(-1)).filter(|&n| n >= 0).map(|n| n as u32);
        let mut cells: Vec<(u32, Cell)> = Vec::new();
        let mut last_ins = 0u8;
        let mut last_vol: Option<u8> = None;
        let mut prev_row: i64 = -1;
        if let Some(list) = cx.array(o, "cells", &path, false) {
            for (i, entry) in list.iter().enumerate() {
                let cp = format!("{path}.cells[{i}]");
                let (row, text) = match entry.as_array().map(|a| a.as_slice()) {
                    Some([r, Value::String(s)]) if r.is_i64() => (r.as_i64().unwrap(), s),
                    _ => {
                        cx.error("bad-value", &cp);
                        continue;
                    }
                };
                if row <= prev_row || row >= rows as i64 {
                    cx.error("bad-row", &cp);
                    continue;
                }
                prev_row = row;
                let (parsed, canonical) = match cell::parse_as(text, cx.later) {
                    Ok((p, canonical, unknown)) => {
                        // Effects of a later minor version, ignored (section 1).
                        if unknown > 0 {
                            cx.warn("unknown-effect", &cp);
                        }
                        (p, canonical)
                    }
                    Err(CellError::Bad(_)) => {
                        cx.error("bad-cell", &cp);
                        continue;
                    }
                    Err(CellError::UnknownEffect(_)) => {
                        cx.error("unknown-effect", &cp);
                        continue;
                    }
                };
                if !canonical {
                    cx.warn("cell-spelling", &cp);
                }
                if parsed.ins != 0 {
                    if instruments.get(parsed.ins as usize).is_none_or(|i| i.is_none()) {
                        cx.error("unknown-instrument", &cp);
                    }
                    last_ins = parsed.ins;
                }
                if matches!(parsed.note, Note::On(_)) && last_ins == 0 {
                    cx.error("no-instrument", &cp);
                }
                let default_vol = instruments
                    .get(last_ins as usize)
                    .and_then(|i| i.as_ref())
                    .map_or(64, |i| i.volume as u8);
                let note_vol = parsed.vol.or(last_vol).unwrap_or(default_vol);
                if parsed.vol.is_some() {
                    last_vol = parsed.vol;
                }
                let cell = Cell {
                    note: parsed.note,
                    ins: last_ins,
                    written_ins: parsed.ins,
                    vol: parsed.vol,
                    note_vol,
                    effects: parsed.effects,
                    text: text.clone(),
                };
                if cell.delay() >= speed {
                    cx.warn("delay-past-row", &cp);
                }
                cells.push((row as u32, cell)); // rows strictly increase
            }
        }
        out.push(Track { name: cx.string(o, "name", &path), rows, written_rows, speed, loop_row, cells });
    }
    out
}

/// The arrangements, or `None` when one of them can't be read. Every arrangement is checked
/// either way.
fn load_arrangements(cx: &mut Ctx, root: &Map<String, Value>, channels: usize, tracks: usize, rate: Option<i64>) -> Option<Vec<Arrangement>> {
    let list = cx.array(root, "arrangements", "", true)?;
    if list.is_empty() || list.len() > MAX_ARRANGEMENTS {
        cx.error("bad-value", "arrangements");
        return None;
    }
    let mut out = Vec::new();
    let mut readable = true;
    let mut names = HashSet::new();
    for (a, v) in list.iter().enumerate() {
        let path = format!("arrangements[{a}]");
        let Some(o) = cx.object(v, &path) else {
            readable = false;
            continue;
        };
        cx.members(o, &["name", "orders", "global"], &path);
        let name = cx.string(o, "name", &path);
        if !name.is_empty() && !names.insert(name.clone()) {
            cx.warn("duplicate-name", join(&path, "name"));
        }
        let (orders, ticks) = load_orders(cx, o, &path, channels, tracks);
        let global = match o.get("global") {
            Some(v) => load_global(cx, v, &join(&path, "global"), ticks.as_deref(), rate),
            None => Vec::new(),
        };
        let global_entries = o.get("global").and_then(Value::as_array).map_or(0, Vec::len);
        match orders {
            Some(orders) => out.push(Arrangement { name, orders, global, global_entries }),
            None => readable = false,
        }
    }
    readable.then_some(out)
}

/// An arrangement's order rows, `None` when they can't be read, and each row's ticks (`None`
/// where unknown), or `None` when the rows aren't there at all.
#[allow(clippy::type_complexity)]
fn load_orders(cx: &mut Ctx, o: &Map<String, Value>, path: &str, channels: usize, tracks: usize) -> (Option<Vec<Order>>, Option<Vec<Option<u64>>>) {
    let Some(orders_list) = cx.array(o, "orders", path, true) else { return (None, None) };
    if orders_list.is_empty() || orders_list.len() > MAX_ORDERS {
        cx.error("bad-value", join(path, "orders"));
        return (None, None);
    }
    let (mut orders, mut row_ticks, mut readable) = (Vec::new(), Vec::new(), true);
    for (r, v) in orders_list.iter().enumerate() {
        let op = format!("{path}.orders[{r}]");
        let Some(row) = cx.object(v, &op) else {
            readable = false;
            row_ticks.push(None);
            continue;
        };
        cx.members(row, &["tracks", "ticks", "next"], &op);
        let mut refs = Vec::new();
        if let Some(t) = cx.array(row, "tracks", &op, true) {
            if t.len() != channels && channels > 0 {
                cx.error("bad-order", join(&op, "tracks"));
            }
            for (c, x) in t.iter().enumerate() {
                match x {
                    Value::Null => refs.push(None),
                    _ => match x.as_i64() {
                        Some(i) if i >= 0 && (i as usize) < tracks => refs.push(Some(i as usize)),
                        Some(i) if i >= 0 => {
                            cx.error("bad-reference", format!("{op}.tracks[{c}]"));
                            refs.push(None);
                        }
                        _ => {
                            cx.error("bad-value", format!("{op}.tracks[{c}]"));
                            refs.push(None);
                        }
                    },
                }
            }
        }
        refs.resize(channels, None);
        let ticks = cx.int(row, "ticks", &op, 1, 1 << 24, None).map(|t| t as u64);
        row_ticks.push(ticks);
        let ticks = ticks.unwrap_or(1);
        // An index out of range is a bad order row; anything but an index or "stop", a bad value.
        let next = match row.get("next") {
            None => Next::Following,
            Some(Value::String(s)) if s == "stop" => Next::Stop,
            Some(x) => match x.as_i64() {
                Some(i) if i >= 0 && (i as usize) < orders_list.len() => Next::Order(i as usize),
                Some(_) => {
                    cx.error("bad-order", join(&op, "next"));
                    Next::Stop
                }
                None => {
                    cx.error("bad-value", join(&op, "next"));
                    Next::Stop
                }
            },
        };
        orders.push(Order { tracks: refs, ticks, next });
    }
    (readable.then_some(orders), Some(row_ticks))
}

/// An integer as cells spell them: no leading zeros, no "-0" (section 5).
fn spelled_int(s: &str) -> Option<i64> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) || digits.len() > 16 {
        return None;
    }
    if (digits.len() > 1 && digits.starts_with('0')) || (s.starts_with('-') && digits == "0") {
        return None;
    }
    s.parse().ok()
}

/// A global event's text (section 2), or `None` if it isn't one.
pub fn parse_global(text: &str, rate: Option<i64>) -> Option<Global> {
    let words: Vec<&str> = text.split(' ').collect();
    let numbers: Option<Vec<i64>> = words[1..].iter().map(|w| spelled_int(w)).collect();
    let numbers = numbers?;
    match (words[0], numbers.as_slice()) {
        ("tick", &[n, d]) if n >= 1 && d >= 1 && n <= 9007199254740991 && d <= 9007199254740991 => {
            if rate.is_some_and(|r| (n as u128) * (r as u128) < d as u128) {
                return None;
            }
            Some(Global::Tick(n as u64, d as u64))
        }
        ("groove", w) if !w.is_empty() && w.len() <= 16 && w.iter().all(|&x| (1..=255).contains(&x)) => {
            Some(Global::Groove(w.iter().map(|&x| x as u32).collect()))
        }
        ("volume", &[g]) if (0..=256).contains(&g) => Some(Global::Volume(g as i32)),
        _ => None,
    }
}

/// The global lane (sections 2 and 4): each event at a tick of an order row that exists, in order,
/// and spelled as one of section 2's events. An entry that isn't `[order, tick, text]` is a bad
/// value; one out of place or not an event, a bad global event. Where the rows or a row's ticks
/// are unknown (`ticks`), an event is only checked against what is known.
fn load_global(cx: &mut Ctx, v: &Value, path: &str, ticks: Option<&[Option<u64>]>, rate: Option<i64>) -> Vec<GlobalEvent> {
    let mut out = Vec::new();
    let Some(list) = v.as_array() else {
        cx.error("bad-value", path);
        return out;
    };
    if list.len() > MAX_GLOBAL {
        cx.error("bad-value", path);
    }
    let mut last = (-1i64, -1i64);
    for (i, x) in list.iter().enumerate() {
        let ep = format!("{path}[{i}]");
        let Some([order, tick, Value::String(text)]) = x.as_array().map(|a| a.as_slice()) else {
            cx.error("bad-value", &ep);
            continue;
        };
        let (Some(order), Some(tick)) = (order.as_i64(), tick.as_i64()) else {
            cx.error("bad-value", &ep);
            continue;
        };
        let exists = order >= 0
            && tick >= 0
            && ticks.is_none_or(|t| t.get(order as usize).is_some_and(|row| row.is_none_or(|n| (tick as u64) < n)));
        let placed = exists && (order, tick) >= last;
        if placed {
            last = (order, tick);
        } else {
            cx.error("bad-global", &ep);
        }
        match parse_global(text, rate) {
            Some(event) if placed => out.push(GlobalEvent { order: order as usize, tick: tick as u64, event }),
            Some(_) => {}
            None => cx.error("bad-global", &ep),
        }
    }
    out
}

/// Whole-song checks: reserved channels and what nothing uses. `index` gives each instrument
/// number's place in the payload, `used_samples` the samples its samplers name.
fn check_song(cx: &mut Ctx, song: &Song, index: &[usize], used_samples: &[bool]) {
    let mut used_tracks = vec![false; song.tracks.len()];
    for (a, arr) in song.arrangements.iter().enumerate() {
        for (r, row) in arr.orders.iter().enumerate() {
            for (c, t) in row.tracks.iter().enumerate() {
                if let Some(t) = t {
                    used_tracks[*t] = true;
                    if song.channels.get(c).is_some_and(|ch| ch.role == Role::Reserved) {
                        cx.error("reserved-channel", format!("arrangements[{a}].orders[{r}].tracks[{c}]"));
                    }
                }
            }
        }
    }
    for (t, used) in used_tracks.iter().enumerate() {
        if !used {
            cx.warn("unused", format!("tracks[{t}]"));
        }
    }
    let mut used_ins = [false; MAX_INSTRUMENT as usize + 1];
    for track in &song.tracks {
        for (_, cell) in &track.cells {
            used_ins[cell.written_ins as usize] = true;
        }
    }
    for (i, ins) in song.instruments.iter().enumerate() {
        let Some(ins) = ins else { continue };
        if !used_ins[i] {
            cx.warn("unused", format!("instruments[{}]", index[ins.number as usize]));
        }
    }
    for (s, used) in used_samples.iter().enumerate() {
        if !used {
            cx.warn("unused", format!("samples[{s}]"));
        }
    }
}
