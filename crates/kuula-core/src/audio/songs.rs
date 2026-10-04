//! Songs: loading `sfx/<name>.omc` and `music/<name>.omc`. A file is taken
//! only after its bounds hold before anything is allocated, the OMT
//! validator reports no error and the `kuula` profile is kept; the result
//! is a shared [`Song`] the mixer plays. The bank keeps the loaded songs by
//! path and counts the song budget.

use std::collections::HashMap;
use std::sync::Arc;

use omt_engine::omc;
use omt_engine::song::{self, Diag, Song};
use serde_json::Value;

use super::sample::SAMPLE_BUDGET;
use super::AudioError;

/// Inflated song bytes a cart may hold across every song it loads.
pub const SONG_BUDGET: usize = 1024 * 1024;

/// The manifest of a container, stored or inflated.
const MANIFEST_LIMIT: usize = 1024 * 1024;

/// A song ready to play: the validated song and the arrangement the
/// container's `subsong` selects.
#[derive(Debug, Clone)]
pub struct PlayableSong {
    pub song: Arc<Song>,
    pub arrangement: usize,
}

/// What a successful load costs the budgets.
#[derive(Debug)]
pub struct LoadedSong {
    pub playable: PlayableSong,
    /// The song's inflated payload.
    pub song_bytes: usize,
    /// 2 bytes per sample frame of every sample record.
    pub sample_bytes: usize,
}

/// The cart's loaded songs by path, within [`SONG_BUDGET`].
#[derive(Default)]
pub struct SongBank {
    songs: HashMap<String, PlayableSong>,
    used: usize,
}

impl SongBank {
    pub fn get(&self, path: &str) -> Option<PlayableSong> {
        self.songs.get(path).cloned()
    }

    /// Song bytes loaded so far.
    pub fn used(&self) -> usize {
        self.used
    }

    /// Song bytes still free.
    pub fn room(&self) -> usize {
        SONG_BUDGET - self.used
    }

    /// Keep a song that [`load`] accepted against the room it was given.
    pub fn insert(&mut self, path: &str, song: &LoadedSong) -> PlayableSong {
        self.used += song.song_bytes;
        self.songs.insert(path.to_string(), song.playable.clone());
        song.playable.clone()
    }
}

fn song_error(path: &str, why: impl Into<String>) -> AudioError {
    AudioError::Song {
        path: path.to_string(),
        why: why.into(),
    }
}

fn sample_error(path: &str, why: impl Into<String>) -> AudioError {
    AudioError::Sample {
        path: path.to_string(),
        why: why.into(),
    }
}

/// `<code> at <path>`, or the code alone for a diagnostic about the whole
/// song.
fn describe(d: &Diag) -> String {
    if d.path.is_empty() {
        d.code.clone()
    } else {
        format!("{} at {}", d.code, d.path)
    }
}

/// A member the validator reads as an integer by its value, whatever its
/// spelling: `1.6e7` is 16 000 000 to it, so it is here too. Rounded up,
/// and 0 for what is not a positive number.
fn count(v: Option<&Value>) -> u64 {
    match v.and_then(Value::as_f64) {
        Some(x) if x > 0.0 => x.ceil() as u64,
        _ => 0,
    }
}

/// The bytes a payload's sample records declare, 2 per sample frame, read
/// without decoding anything. A payload that is not JSON, or has no
/// records, declares none: the validator reports the damage.
fn declared_sample_bytes(payload: &[u8]) -> u64 {
    let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(payload) else {
        return 0;
    };
    let Some(Value::Array(records)) = root.get("samples") else {
        return 0;
    };
    records
        .iter()
        .map(|r| {
            let frames = count(r.get("frames"));
            let channels = count(r.get("channels")).clamp(1, 2);
            frames.saturating_mul(channels).saturating_mul(2)
        })
        .fold(0, u64::saturating_add)
}

/// Read the OMC `file` at `path` as a song, given the room the two budgets
/// have left. Nothing is kept: the caller charges the result to the bank
/// and the sample bank, so a refused load changes nothing.
pub fn load(
    path: &str,
    file: &[u8],
    song_room: usize,
    sample_room: usize,
) -> Result<LoadedSong, AudioError> {
    let chunks = omc::chunks(file).map_err(|e| song_error(path, e))?;

    // The bounds, on the chunks that could hold a song taken together,
    // before the manifest is trusted to say which one plays. Each chunk
    // gets what the ones before it left of the room, so a load inflates
    // at most `song_room` bytes however many chunks the file has.
    let first = &chunks[0];
    if &first.kind == b"JSON" && first.data.is_some_and(|d| d.len() > MANIFEST_LIMIT) {
        return Err(song_error(path, "the manifest is larger than 1 MiB"));
    }
    let mut left = song_room;
    for (i, chunk) in chunks.iter().enumerate() {
        let Some(data) = chunk.data else { continue };
        let size = match &chunk.kind {
            b"SONG" if data.len() > left => {
                return Err(song_error(
                    path,
                    format!(
                        "SONG chunk {i} is {} bytes; the song budget has {left} left for it",
                        data.len()
                    ),
                ));
            }
            b"SONG" => data.len(),
            b"SONZ" => match omc::inflate(data, left) {
                Some(plain) => plain.len(),
                None => {
                    return Err(song_error(
                        path,
                        format!(
                            "SONZ chunk {i} does not inflate within the {left} bytes the song budget has left for it"
                        ),
                    ));
                }
            },
            _ => continue,
        };
        left -= size;
    }

    let entry = omc::read_song(file).map_err(|e| song_error(path, e))?;

    // What the sample records declare must fit before the validator
    // decodes any resource.
    let declared = declared_sample_bytes(&entry.payload);
    if declared > sample_room as u64 {
        return Err(sample_error(
            path,
            format!(
                "sample budget exceeded: the sample records declare {declared} bytes, {sample_room} left of {SAMPLE_BUDGET}"
            ),
        ));
    }

    let loaded = song::load(&entry.payload, &entry.resources);
    // The `kuula` profile is named by its rule whichever profile the song
    // names, so a song's own `profile:<rule>` error comes after it.
    if let Some(d) = loaded.errors().find(|d| !d.code.starts_with("profile:")) {
        return Err(song_error(path, describe(d)));
    }
    let Some(song) = loaded.song.as_ref() else {
        return Err(song_error(path, "the song can't be read"));
    };
    if let Some(rule) = song::profile_violations(song, "kuula").and_then(|v| v.into_iter().next()) {
        return Err(song_error(
            path,
            format!("breaks the kuula profile: {rule}"),
        ));
    }
    if let Some(d) = loaded.errors().next() {
        return Err(song_error(path, describe(d)));
    }
    if entry.subsong >= song.arrangements.len() {
        return Err(song_error(
            path,
            format!(
                "subsong {} does not exist: the song has {} arrangements",
                entry.subsong,
                song.arrangements.len()
            ),
        ));
    }
    let sample_bytes: u64 = song
        .samples
        .iter()
        .map(|s| s.frames as u64 * s.channels as u64 * 2)
        .sum();
    if sample_bytes > sample_room as u64 {
        return Err(sample_error(
            path,
            format!(
                "sample budget exceeded: the song's samples are {sample_bytes} bytes, {sample_room} left of {SAMPLE_BUDGET}"
            ),
        ));
    }
    let song_bytes = entry.payload.len();
    if song_bytes > song_room {
        return Err(song_error(
            path,
            format!("the song is {song_bytes} bytes; the song budget has {song_room} left"),
        ));
    }
    let arrangement = entry.subsong;
    let song = loaded.song.expect("checked above");
    Ok(LoadedSong {
        playable: PlayableSong {
            song: Arc::new(song),
            arrangement,
        },
        song_bytes,
        sample_bytes: sample_bytes as usize,
    })
}
