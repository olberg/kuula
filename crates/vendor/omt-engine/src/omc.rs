//! Just enough of the Open Module Container to find an OMT song, or an OMQ bank, and its resources,
//! and to write one: the container's own readers (`c/`, `python/`) are the reference for everything
//! else.

use serde_json::Value;

use crate::song::MAX_PAYLOAD;

const MANIFEST_LIMIT: usize = 1 << 20;

/// The largest integer a manifest holds exactly (I-JSON).
const MAX_INTEGER: f64 = ((1u64 << 53) - 1) as f64;

/// The highest chunk number an entry may name (docs/container.md, *Entries*).
const MAX_CHUNK: i64 = 65_534;

/// The container version read and written, major and minor: the pre-release 0.8. Until the
/// container's 1.0 is released a reader accepts exactly this one (docs/container.md, *Versions*).
const VERSION: (u16, u16) = (0, 8);

/// One chunk: its type and data, or `None` for data whose CRC doesn't match.
pub struct Chunk<'a> {
    pub kind: [u8; 4],
    pub data: Option<&'a [u8]>,
}

/// A native-format song of a file (OMT, or an OMQ bank): its plain payload and its resources.
pub struct SongEntry<'a> {
    pub payload: Vec<u8>,
    pub resources: Vec<Option<&'a [u8]>>,
    pub subsong: usize,
}

fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

pub fn chunks(file: &[u8]) -> Result<Vec<Chunk<'_>>, String> {
    walk(file).map(|(chunks, _)| chunks)
}

/// The chunks of a file, and whether they end exactly at the length the header states. A chunk is
/// intact (its `data` is there) when it is whole, its padding is zero and its CRC matches.
fn walk(file: &[u8]) -> Result<(Vec<Chunk<'_>>, bool), String> {
    if file.len() < 12 || &file[..4] != b"\x89OMC" {
        return Err("not an OMC file".into());
    }
    let version = (u16::from_le_bytes([file[4], file[5]]), u16::from_le_bytes([file[6], file[7]]));
    if version != VERSION {
        return Err(format!("unknown container version {}.{}", version.0, version.1));
    }
    let stated = u32le(&file[8..12]) as usize;
    let length = stated.min(file.len());
    let mut out = Vec::new();
    let mut p = 12usize;
    while p + 8 <= length {
        let n = u32le(&file[p..]) as usize;
        let kind: [u8; 4] = file[p + 4..p + 8].try_into().unwrap();
        let crc_at = p + 8 + n + ((4 - n % 4) % 4);
        if crc_at + 4 > length {
            break;
        }
        let data = &file[p + 8..p + 8 + n];
        let mut h = crc32fast::Hasher::new();
        h.update(&kind);
        h.update(data);
        let ok = h.finalize() == u32le(&file[crc_at..]) && file[p + 8 + n..crc_at].iter().all(|&b| b == 0);
        out.push(Chunk { kind, data: ok.then_some(data) });
        p = crc_at + 4;
    }
    if out.is_empty() {
        return Err("the file has no chunks".into());
    }
    Ok((out, p == stated))
}

/// Inflates one zlib stream within `limit` bytes.
pub fn inflate(data: &[u8], limit: usize) -> Option<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, limit).ok()
}

/// An integer-valued number, judged by value as the container does: `1.0` is the integer 1, `true`
/// is not an integer (docs/container.md, *Validation*).
fn integer(v: &Value) -> Option<i64> {
    let Value::Number(n) = v else { return None };
    n.as_i64().or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() <= MAX_INTEGER).map(|f| f as i64))
}

/// A nonnegative integer.
fn natural(v: &Value) -> Option<u64> {
    integer(v).and_then(|n| u64::try_from(n).ok())
}

fn chunk_ref(v: &Value) -> Option<usize> {
    natural(v.get("chunk")?).and_then(|n| usize::try_from(n).ok())
}

/// The chunks of a file, whether they end where its header says, and its manifest.
fn open(file: &[u8]) -> Result<(Vec<Chunk<'_>>, bool, Value), String> {
    let (chunks, complete) = walk(file)?;
    let manifest = match (&chunks[0].kind, chunks[0].data) {
        (b"JSON", Some(d)) => d.to_vec(),
        (b"JSNZ", Some(d)) => inflate(d, MANIFEST_LIMIT).ok_or("the manifest doesn't inflate")?,
        _ => return Err("no intact manifest".into()),
    };
    let manifest: Value = serde_json::from_slice(&manifest).map_err(|_| "the manifest isn't JSON")?;
    Ok((chunks, complete, manifest))
}

fn is_song(chunks: &[Chunk], i: usize) -> bool {
    chunks.get(i).is_some_and(|c| &c.kind == b"SONG" || &c.kind == b"SONZ")
}

/// The payload (a `SONZ` inflated within 16 MiB), the resources and the selector of a song entry.
fn song_of<'a>(chunks: &[Chunk<'a>], entry: &Value) -> Result<SongEntry<'a>, String> {
    let song_chunk = chunk_ref(entry).filter(|&c| is_song(chunks, c)).ok_or("the song entry names no SONG or SONZ")?;
    let c = &chunks[song_chunk];
    let data = c.data.ok_or("the song chunk is damaged")?;
    let payload = if &c.kind == b"SONZ" {
        inflate(data, MAX_PAYLOAD).ok_or("the compressed song doesn't inflate within 16 MiB")?
    } else {
        data.to_vec()
    };
    let resources = match entry.get("resources") {
        Some(Value::Array(r)) => r
            .iter()
            .map(|r| chunk_ref(r).and_then(|c| chunks.get(c)).and_then(|c| c.data))
            .collect(),
        _ => Vec::new(),
    };
    let subsong = entry.get("subsong").and_then(natural).unwrap_or(0) as usize;
    Ok(SongEntry { payload, resources, subsong })
}

/// The first song entry with `format` omitted, as a player picks it.
pub fn read_song(file: &[u8]) -> Result<SongEntry<'_>, String> {
    let (chunks, _, manifest) = open(file)?;
    let entry = match manifest.get("songs") {
        Some(Value::Array(songs)) => songs
            .iter()
            .find(|s| s.get("format").is_none())
            .cloned()
            .ok_or("no Open Module Track song in the file")?,
        _ if is_song(&chunks, 1) => serde_json::json!({"chunk": 1}),
        _ => return Err("no Open Module Track song in the file".into()),
    };
    song_of(&chunks, &entry)
}

/// A cue audio file of a bank's file: an entry of `audio` whose `role` is `cue` (docs/container.md,
/// *Cue audio*).
pub struct CueAudio<'a> {
    pub name: String,
    /// The chunk's bytes when the entry is well formed and its chunk an intact `AUDI` (see
    /// `read_bank`); `None` when the entry is malformed, or the chunk is damaged, lost or another
    /// type.
    pub data: Option<&'a [u8]>,
    /// The loop positions the entry declares, in decoded frames. For a file of one of the two
    /// kinds of OMQ section 4 a loop that doesn't fit it (a `loopEnd` without a `loopStart`, or
    /// anything but `0 <= loopStart < loopEnd <= frames`) is `bad-audio`, and the cue plays nothing.
    pub loop_start: Option<u64>,
    pub loop_end: Option<u64>,
}

/// A bank of a file: its `omq` song entry and the cue audio files beside it.
pub struct BankEntry<'a> {
    pub song: SongEntry<'a>,
    pub audio: Vec<CueAudio<'a>>,
}

/// The first song entry whose `format` is `"omq"`, as a player of banks picks it, with the file's
/// cue audio: each entry of `audio` of role `cue` that has a nonempty string `name`, in file order,
/// one whose name an earlier one has left out (the container makes the later one malformed).
///
/// An entry the container calls malformed, as far as the manifest and the chunk table show it, has
/// no `data`: its cue plays nothing and the bank is degraded (OMQ section 4). That is `cue_data`'s
/// list; a loop that merely doesn't fit is not on it, since that is the loader's `bad-audio`.
pub fn read_bank(file: &[u8]) -> Result<BankEntry<'_>, String> {
    let (chunks, complete, manifest) = open(file)?;
    let songs = match manifest.get("songs") {
        Some(Value::Array(songs)) => songs,
        _ => return Err("no Open Module Cues bank in the file".into()),
    };
    let entry = songs
        .iter()
        .find(|s| s.get("format").and_then(Value::as_str) == Some("omq"))
        .ok_or("no Open Module Cues bank in the file")?;
    let song = song_of(&chunks, entry)?;
    let mut audio: Vec<CueAudio> = Vec::new();
    if let Some(Value::Array(list)) = manifest.get("audio") {
        for a in list.iter().filter(|a| a.get("role").and_then(Value::as_str) == Some("cue")) {
            let Some(name) = a.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) else { continue };
            if audio.iter().any(|x| x.name == name) {
                continue;
            }
            audio.push(CueAudio {
                name: name.to_string(),
                data: cue_data(&chunks, complete, &manifest, a),
                loop_start: a.get("loopStart").and_then(natural),
                loop_end: a.get("loopEnd").and_then(natural),
            });
        }
    }
    Ok(BankEntry { song, audio })
}

/// The bytes of a cue entry's chunk, or `None` when the entry is malformed or its chunk unusable
/// (docs/container.md, *Validation*, *Rendered audio*). Python's reader (`reader.py`: `entry`,
/// `member`, `extension_members`, `audio_members`, `sharing`, `references`) is the reference. The
/// entry is malformed when:
///
/// * its `chunk` isn't an integer from 1 to 65,534, names a chunk the file doesn't have, or one that
///   isn't an `AUDI`; the chunk is damaged;
/// * another entry of `songs`, a song's `resources`, `pictures` or `audio` names the same chunk (an
///   album's `tracks` are ignored beside a song, so they take no part);
/// * its `mediaType` isn't a string, its `song` isn't an index of `songs`, or its `loopStart` or
///   `loopEnd` isn't a nonnegative integer;
/// * its `extensionsUsed` or `extensionsRequired` isn't an array of distinct strings, its
///   `extensions` isn't an object, a name of the last two isn't among the used ones, or it
///   requires an extension at all, which this engine has none of;
/// * a `chunk` member inside any of its other members is a bad reference.
fn cue_data<'a>(chunks: &[Chunk<'a>], complete: bool, manifest: &Value, a: &Value) -> Option<&'a [u8]> {
    let c = a.get("chunk").and_then(integer).filter(|c| (1..=MAX_CHUNK).contains(c))? as usize;
    let chunk = chunks.get(c).filter(|k| &k.kind == b"AUDI")?;
    let data = chunk.data?;
    let songs = manifest.get("songs").and_then(Value::as_array).map_or(0, Vec::len);
    let ok = a.get("mediaType").is_none_or(Value::is_string)
        && a.get("song").is_none_or(|s| natural(s).is_some_and(|s| s < songs as u64))
        && ["loopStart", "loopEnd"].iter().all(|k| a.get(k).is_none_or(|v| natural(v).is_some()));
    if !ok || !extensions_ok(a) || !references_ok(a, chunks.len(), complete) {
        return None;
    }
    // Different core arrays never share a chunk, nor do two entries of `audio`.
    let named = |v: &Value| v.get("chunk").and_then(integer) == Some(c as i64);
    let list = |name: &str| manifest.get(name).and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    let resources = list("songs").iter().filter_map(|s| s.get("resources")?.as_array()).flatten();
    let sharing = list("songs").iter().chain(resources).chain(list("pictures")).chain(list("audio")).filter(|v| named(v)).count();
    (sharing == 1).then_some(data)
}

/// Whether a name list is an array of distinct strings, as the container's extension lists are.
fn names(v: &Value) -> Option<Vec<&str>> {
    let all: Vec<&str> = v.as_array()?.iter().map(Value::as_str).collect::<Option<_>>()?;
    let mut sorted = all.clone();
    sorted.sort_unstable();
    sorted.dedup();
    (sorted.len() == all.len()).then_some(all)
}

/// A cue entry's extension members: well typed, required ones within the used ones, and none
/// required (this engine has no extension).
fn extensions_ok(a: &Value) -> bool {
    let used = match a.get("extensionsUsed") {
        None => Some(Vec::new()),
        Some(u) => names(u),
    };
    let required_ok = a.get("extensionsRequired").is_none_or(|r| names(r).is_some_and(|r| r.is_empty()));
    let declared_ok = match (a.get("extensions"), &used) {
        (None, _) => true,
        (Some(Value::Object(e)), Some(used)) => e.keys().all(|k| used.contains(&k.as_str())),
        _ => false,
    };
    used.is_some() && required_ok && declared_ok
}

/// Whether every object with a `chunk` member inside an entry's members other than its own `chunk`
/// has a valid reference: an integer from 1 to 65,534, naming a chunk the file has when it is
/// complete (docs/container.md, *Validation*).
fn references_ok(entry: &Value, chunk_count: usize, complete: bool) -> bool {
    fn walk(v: &Value, chunk_count: usize, complete: bool) -> bool {
        match v {
            Value::Object(o) => o.iter().all(|(k, v)| {
                if k == "chunk" { valid_reference(v, chunk_count, complete) } else { walk(v, chunk_count, complete) }
            }),
            Value::Array(a) => a.iter().all(|v| walk(v, chunk_count, complete)),
            _ => true,
        }
    }
    let Value::Object(o) = entry else { return true };
    o.iter().all(|(k, v)| k == "chunk" || walk(v, chunk_count, complete))
}

fn valid_reference(v: &Value, chunk_count: usize, complete: bool) -> bool {
    integer(v).is_some_and(|c| (1..=MAX_CHUNK).contains(&c) && (!complete || (c as usize) < chunk_count))
}

fn put_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.resize(out.len() + (4 - data.len() % 4) % 4, 0);
    let mut h = crc32fast::Hasher::new();
    h.update(kind);
    h.update(data);
    out.extend_from_slice(&h.finalize().to_le_bytes());
}

/// A resource to write: its bytes and its `mediaType`, if it has one.
pub struct ResourceOut<'a> {
    pub data: &'a [u8],
    pub media_type: Option<&'a str>,
}

/// A cue audio file to write: an `audio` entry of role `cue`.
pub struct CueAudioOut<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
    /// `audio/vnd.wave` or `audio/flac` for those payloads; for another, what it is.
    pub media_type: Option<&'a str>,
    pub loop_start: Option<u64>,
    pub loop_end: Option<u64>,
}

/// Writes a file of one default-format song: the manifest, the song (compressed when that is
/// smaller) and one `SMPL` chunk per resource. `title` goes in the manifest's metadata.
pub fn write_song(payload: &[u8], resources: &[ResourceOut], title: Option<&str>, generator: &str) -> Vec<u8> {
    write(None, payload, resources, &[], title, generator)
}

/// Writes a file of one `omq` bank: the manifest, the bank (compressed when that is smaller), one
/// `SMPL` chunk per resource and one `AUDI` chunk per cue audio file, of role `cue`.
pub fn write_bank(payload: &[u8], resources: &[ResourceOut], audio: &[CueAudioOut], title: Option<&str>, generator: &str) -> Vec<u8> {
    write(Some("omq"), payload, resources, audio, title, generator)
}

fn write(format: Option<&str>, payload: &[u8], resources: &[ResourceOut], audio: &[CueAudioOut], title: Option<&str>, generator: &str) -> Vec<u8> {
    let mut entry = serde_json::Map::new();
    entry.insert("chunk".into(), 1.into());
    if let Some(f) = format {
        entry.insert("format".into(), f.into());
    }
    if !resources.is_empty() {
        let list: Vec<Value> = resources
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let mut o = serde_json::Map::new();
                o.insert("chunk".into(), (2 + i).into());
                if let Some(m) = r.media_type {
                    o.insert("mediaType".into(), m.into());
                }
                Value::Object(o)
            })
            .collect();
        entry.insert("resources".into(), Value::Array(list));
    }
    let mut manifest = serde_json::Map::new();
    manifest.insert("generator".into(), generator.into());
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        manifest.insert("metadata".into(), serde_json::json!({"title": t}));
    }
    manifest.insert("songs".into(), Value::Array(vec![Value::Object(entry)]));
    if !audio.is_empty() {
        let list: Vec<Value> = audio
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let mut o = serde_json::Map::new();
                o.insert("chunk".into(), (2 + resources.len() + i).into());
                o.insert("role".into(), "cue".into());
                o.insert("name".into(), a.name.into());
                if let Some(m) = a.media_type {
                    o.insert("mediaType".into(), m.into());
                }
                if let Some(s) = a.loop_start {
                    o.insert("loopStart".into(), s.into());
                }
                if let Some(e) = a.loop_end {
                    o.insert("loopEnd".into(), e.into());
                }
                Value::Object(o)
            })
            .collect();
        manifest.insert("audio".into(), Value::Array(list));
    }
    let manifest = serde_json::to_vec(&Value::Object(manifest)).unwrap();

    let mut out = b"\x89OMC".to_vec();
    out.extend_from_slice(&VERSION.0.to_le_bytes());
    out.extend_from_slice(&VERSION.1.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    put_chunk(&mut out, b"JSON", &manifest);
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(payload, 9);
    let pad = |n: usize| n + (4 - n % 4) % 4;
    if pad(packed.len()) < pad(payload.len()) {
        put_chunk(&mut out, b"SONZ", &packed);
    } else {
        put_chunk(&mut out, b"SONG", payload);
    }
    for r in resources {
        put_chunk(&mut out, b"SMPL", r.data);
    }
    for a in audio {
        put_chunk(&mut out, b"AUDI", a.data);
    }
    let len = out.len() as u32;
    out[8..12].copy_from_slice(&len.to_le_bytes());
    out
}
