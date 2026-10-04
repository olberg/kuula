//! Just enough of the Open Module Container to find an OMT song and its resources, and to write
//! one: the container's own readers (`c/`, `python/`) are the reference for everything else.

use serde_json::Value;

use crate::song::MAX_PAYLOAD;

const MANIFEST_LIMIT: usize = 1 << 20;

/// The container version read and written, major and minor: the pre-release 0.7. Until the
/// container's 1.0 is released a reader accepts exactly this one (docs/container.md, *Versions*).
const VERSION: (u16, u16) = (0, 7);

/// One chunk: its type and data, or `None` for data whose CRC doesn't match.
pub struct Chunk<'a> {
    pub kind: [u8; 4],
    pub data: Option<&'a [u8]>,
}

/// The default-format song of a file: its plain payload and its resources.
pub struct SongEntry<'a> {
    pub payload: Vec<u8>,
    pub resources: Vec<Option<&'a [u8]>>,
    pub subsong: usize,
}

fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

pub fn chunks(file: &[u8]) -> Result<Vec<Chunk<'_>>, String> {
    if file.len() < 12 || &file[..4] != b"\x89OMC" {
        return Err("not an OMC file".into());
    }
    let version = (u16::from_le_bytes([file[4], file[5]]), u16::from_le_bytes([file[6], file[7]]));
    if version != VERSION {
        return Err(format!("unknown container version {}.{}", version.0, version.1));
    }
    let length = (u32le(&file[8..12]) as usize).min(file.len());
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
        let ok = h.finalize() == u32le(&file[crc_at..]);
        out.push(Chunk { kind, data: ok.then_some(data) });
        p = crc_at + 4;
    }
    if out.is_empty() {
        return Err("the file has no chunks".into());
    }
    Ok(out)
}

/// Inflates one zlib stream within `limit` bytes.
pub fn inflate(data: &[u8], limit: usize) -> Option<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, limit).ok()
}

fn chunk_ref(v: &Value) -> Option<usize> {
    v.get("chunk")?.as_u64().map(|n| n as usize)
}

/// The first song entry with `format` omitted, as a player picks it.
pub fn read_song(file: &[u8]) -> Result<SongEntry<'_>, String> {
    let chunks = chunks(file)?;
    let manifest = match (&chunks[0].kind, chunks[0].data) {
        (b"JSON", Some(d)) => d.to_vec(),
        (b"JSNZ", Some(d)) => inflate(d, MANIFEST_LIMIT).ok_or("the manifest doesn't inflate")?,
        _ => return Err("no intact manifest".into()),
    };
    let manifest: Value = serde_json::from_slice(&manifest).map_err(|_| "the manifest isn't JSON")?;
    let is_song = |i: usize| chunks.get(i).is_some_and(|c| &c.kind == b"SONG" || &c.kind == b"SONZ");
    let entry = match manifest.get("songs") {
        Some(Value::Array(songs)) => songs
            .iter()
            .find(|s| s.get("format").is_none())
            .cloned()
            .ok_or("no Open Module Track song in the file")?,
        _ if is_song(1) => serde_json::json!({"chunk": 1}),
        _ => return Err("no Open Module Track song in the file".into()),
    };
    let song_chunk = chunk_ref(&entry).filter(|&c| is_song(c)).ok_or("the song entry names no SONG or SONZ")?;
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
    let subsong = entry.get("subsong").and_then(|s| s.as_u64()).unwrap_or(0) as usize;
    Ok(SongEntry { payload, resources, subsong })
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

/// Writes a file of one default-format song: the manifest, the song (compressed when that is
/// smaller) and one `SMPL` chunk per resource. `title` goes in the manifest's metadata.
pub fn write_song(payload: &[u8], resources: &[ResourceOut], title: Option<&str>, generator: &str) -> Vec<u8> {
    let mut entry = serde_json::Map::new();
    entry.insert("chunk".into(), 1.into());
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
    let len = out.len() as u32;
    out[8..12].copy_from_slice(&len.to_le_bytes());
    out
}
