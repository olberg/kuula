//! Cue banks: loading `cues/<bank>.omc`, an Open Module Cues bank of a
//! game's sound effects. A file is taken only after its bounds hold before
//! anything is allocated, the OMQ validator reports no error, the bank
//! fits the format's `kuula` profile and every cue can play; the result is
//! a shared [`Bank`] whose cues the mixer plays by name.

use std::collections::HashSet;
use std::sync::Arc;

use omt_engine::omq::{self, Bank, Content, PROFILE_KUULA};
use omt_engine::song::MAX_FRAMES;
use omt_engine::{flac, omc, wave};
use serde_json::Value;

use super::sample::SAMPLE_BUDGET;
use super::songs::{bounded_chunks, declared_sample_bytes, describe, sample_error, song_error};
use super::AudioError;

/// What a successful load costs the budgets.
#[derive(Debug)]
pub struct LoadedBank {
    pub bank: Arc<Bank>,
    /// The bank's inflated payload.
    pub song_bytes: usize,
    /// 2 bytes per sample frame of every sample record and of every audio
    /// file a plain cue names.
    pub sample_bytes: usize,
}

/// The audio entries the payload's plain cues name, each once, in the
/// order of the cues, read without trusting anything else about the
/// payload. A payload that is not JSON names none: the validator reports
/// the damage.
fn named_audio(payload: &[u8]) -> Vec<String> {
    let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(payload) else {
        return Vec::new();
    };
    let Some(Value::Array(cues)) = root.get("cues") else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    cues.iter()
        .filter_map(|c| c.get("audio").and_then(Value::as_str))
        .filter(|name| seen.insert(*name))
        .map(str::to_string)
        .collect()
}

/// The bytes the audio file `name` is once decoded, 2 for each sample of
/// each channel, as its header says and without decoding it, when it is of
/// one of the two kinds the format plays exactly; `sample_error` when they
/// are more than `room`. `Ok(0)` for any other file: the engine decodes
/// nothing of it. A FLAC of the kind states its length (OMQ section 4),
/// and the engine decodes it no further than that, so what fits here is
/// all a damaged or lying stream can cost.
fn audio_bytes(path: &str, name: &str, data: &[u8], room: u64) -> Result<u64, AudioError> {
    let over = |bytes: Option<u64>| {
        let what = match bytes {
            Some(n) => format!("is {n} bytes of samples"),
            None => "is damaged, or longer than the sample budget has room for".to_string(),
        };
        Err(sample_error(
            path,
            format!(
                "cue audio {name:?} {what}; the sample budget has {room} left of {SAMPLE_BUDGET}"
            ),
        ))
    };
    if data.starts_with(b"RIFF") {
        // A WAVE's samples are its `data` chunk: read with the frames the
        // room allows as its bound, nothing longer is allocated. A file
        // of two channels has twice the samples, so it is measured again.
        let frames = (room / 2).min(MAX_FRAMES as u64) as u32;
        return match wave::read(data, frames) {
            wave::Read::Pcm { pcm, .. } if pcm.len() as u64 * 2 > room => {
                over(Some(pcm.len() as u64 * 2))
            }
            wave::Read::Pcm { pcm, .. } => Ok(pcm.len() as u64 * 2),
            wave::Read::Damaged => over(None),
            wave::Read::Other | wave::Read::NotWave => Ok(0),
        };
    }
    if data.starts_with(b"fLaC") {
        // The kind is STREAMINFO's fixed fields alone, as the engine
        // reads it; a stream without them, or of another kind, it does
        // not decode.
        let Some(info) = flac::stream_info(data) else {
            return Ok(0);
        };
        let of_the_kind = info.bits == 16
            && info.channels <= 2
            && (1000..=384_000).contains(&info.rate)
            && info.total != 0;
        if !of_the_kind || info.total > MAX_FRAMES as u64 {
            return Ok(0);
        }
        let bytes = info.total * info.channels as u64 * 2;
        if bytes > room {
            return over(Some(bytes));
        }
        return Ok(bytes);
    }
    Ok(0)
}

/// The rule of the `kuula` profile a bank that does not fit it breaks
/// first, as the validator names it. The validator gives the rules only
/// for a bank that names the profile, so the payload is validated once
/// more with that name in it; this is the path of a refusal, and it
/// decodes nothing the first validation did not.
fn broken_rule(entry: &omc::BankEntry) -> Option<String> {
    let mut payload: Value = serde_json::from_slice(&entry.song.payload).ok()?;
    payload
        .as_object_mut()?
        .insert("profile".into(), PROFILE_KUULA.into());
    let payload = serde_json::to_vec(&payload).ok()?;
    let loaded = omq::load(&payload, &entry.song.resources, Some(&entry.audio));
    let rule = loaded
        .errors()
        .find_map(|d| d.code.strip_prefix("profile:"))?;
    Some(rule.to_string())
}

/// Read the OMC `file` at `path` as a cue bank, given the room the two
/// budgets have left. Nothing is kept: the caller charges the result to
/// the song bank and the sample bank, so a refused load changes nothing.
pub fn load(
    path: &str,
    file: &[u8],
    song_room: usize,
    sample_room: usize,
) -> Result<LoadedBank, AudioError> {
    bounded_chunks(path, file, song_room)?;
    let entry = omc::read_bank(file).map_err(|e| song_error(path, e))?;

    // What the sample records declare, then what the plain cues' files
    // declare, must fit before the validator decodes anything.
    let declared = declared_sample_bytes(&entry.song.payload);
    if declared > sample_room as u64 {
        return Err(sample_error(
            path,
            format!(
                "sample budget exceeded: the sample records declare {declared} bytes, {sample_room} left of {SAMPLE_BUDGET}"
            ),
        ));
    }
    let mut room = sample_room as u64 - declared;
    for name in named_audio(&entry.song.payload) {
        // A name with no entry is the validator's `bad-reference`; an entry
        // whose chunk is damaged or lost leaves its cue with nothing to
        // play, which is refused below.
        let data = entry
            .audio
            .iter()
            .find(|a| a.name == name)
            .and_then(|a| a.data);
        if let Some(data) = data {
            room -= audio_bytes(path, &name, data, room)?;
        }
    }

    let loaded = omq::load(
        &entry.song.payload,
        &entry.song.resources,
        Some(&entry.audio),
    );
    // The `kuula` profile is named by its rule whichever profile the bank
    // names, as a song's is, so the bank's own `profile:<rule>` errors are
    // left to the check after this one.
    if let Some(d) = loaded.errors().find(|d| !d.code.starts_with("profile:")) {
        return Err(song_error(path, describe(d)));
    }
    let fits = loaded
        .fits
        .as_ref()
        .is_some_and(|f| f.contains(&PROFILE_KUULA));
    if loaded.bank.is_some() && !fits {
        let why = match broken_rule(&entry) {
            Some(rule) => format!("breaks the kuula profile: {rule}"),
            None => "breaks the kuula profile".to_string(),
        };
        return Err(song_error(path, why));
    }
    if let Some(d) = loaded.errors().next() {
        return Err(song_error(path, describe(d)));
    }
    let Some(bank) = loaded.bank else {
        return Err(song_error(path, "the bank can't be read"));
    };

    // Every cue plays: a cue whose audio entry is damaged or lost has
    // nothing to play, and the profile does not see it.
    let mut sample_bytes: u64 = bank
        .samples
        .iter()
        .map(|s| s.frames as u64 * s.channels as u64 * 2)
        .sum();
    let mut counted = HashSet::new();
    for cue in &bank.cues {
        match &cue.content {
            Content::Tracked { .. } => {}
            Content::Plain {
                sound: Some(sound), ..
            } => {
                if counted.insert(Arc::as_ptr(sound)) {
                    sample_bytes += sound.pcm.len() as u64 * 2;
                }
            }
            Content::Plain { .. } => {
                return Err(song_error(
                    path,
                    format!(
                        "cue {:?} has nothing to play: its audio is damaged or lost",
                        cue.name
                    ),
                ));
            }
        }
    }
    if sample_bytes > sample_room as u64 {
        return Err(sample_error(
            path,
            format!(
                "sample budget exceeded: the bank's samples and audio are {sample_bytes} bytes, {sample_room} left of {SAMPLE_BUDGET}"
            ),
        ));
    }
    let song_bytes = entry.song.payload.len();
    if song_bytes > song_room {
        return Err(song_error(
            path,
            format!("the bank is {song_bytes} bytes; the song budget has {song_room} left"),
        ));
    }
    Ok(LoadedBank {
        bank: Arc::new(bank),
        song_bytes,
        sample_bytes: sample_bytes as usize,
    })
}
