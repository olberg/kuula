//! A plain cue's file (docs/omq.md section 4): which of the two kinds that play exactly it is, if
//! either, and its frames.

use std::sync::Arc;

use super::Sound;
use crate::song::MAX_FRAMES;
use crate::{flac, wave};

/// What a cue's file is.
pub(super) enum File {
    /// One of the two kinds, whole: its frames.
    Sound { rate: u32, channels: u32, pcm: Arc<[i16]> },
    /// A file of one of the two kinds that is damaged, has no frames or has more than 2^24 (a FLAC
    /// whose frames are not the total it states): the cue plays nothing and the file is `bad-audio`.
    Damaged,
    /// Any other file: left to the player, and `unknown-audio` here.
    Unknown,
}

/// Classifies a cue's file by its content, as the container recognises payloads: a RIFF WAVE
/// (8 or 16 bits, integer PCM) or a FLAC of 16-bit samples that states its length (a total of
/// samples that isn't 0), with one or two channels and a rate of 1000 to 384000 Hz, is one of the
/// two kinds. A file that starts as one of them, and whose header says it is of the kind, but is
/// broken, is damaged; so is one whose audio entry declares a loop that doesn't fit it (`start`
/// and `end` are the entry's `loopStart` and `loopEnd`, section 4). A file whose header says
/// another coding, depth, channel count or rate, or a FLAC that doesn't state its length, is left
/// to the player, as is anything else (Ogg Opus, MP3...).
pub(super) fn classify(bytes: &[u8], start: Option<u64>, end: Option<u64>) -> File {
    if bytes.starts_with(b"RIFF") {
        return match wave::read(bytes, MAX_FRAMES) {
            wave::Read::Pcm { rate, channels, pcm } if loop_fits(pcm.len() / channels as usize, start, end) => {
                File::Sound { rate, channels, pcm: pcm.into() }
            }
            wave::Read::Pcm { .. } => File::Damaged,
            wave::Read::Damaged => File::Damaged,
            wave::Read::Other | wave::Read::NotWave => File::Unknown,
        };
    }
    if bytes.starts_with(b"fLaC") {
        // The kind is decided by STREAMINFO's fixed fields alone, before any damage: a file whose
        // header says another depth, channel count or rate is the player's even if its later
        // metadata is broken.
        let Some(info) = flac::stream_info(bytes) else { return File::Damaged };
        // A total of 0 is a length unknown (section 4): the file is not of the kind either.
        if info.bits != 16 || info.channels > 2 || !(1000..=384000).contains(&info.rate) || info.total == 0 {
            return File::Unknown;
        }
        // The length is known from STREAMINFO: one over the bound is read from it, decoding nothing,
        // and the stream is decoded no further than the length it states, which its frames must be.
        if info.total > MAX_FRAMES as u64 {
            return File::Damaged;
        }
        return match flac::decode_stream(bytes, info.total as u32) {
            Some(pcm) if pcm.len() == info.total as usize * info.channels as usize && loop_fits(info.total as usize, start, end) => {
                File::Sound { rate: info.rate, channels: info.channels, pcm: pcm.into() }
            }
            _ => File::Damaged,
        };
    }
    File::Unknown
}

/// Whether the loop an audio entry declares fits a file of `frames` frames (section 4): none, or
/// `0 <= loopStart < loopEnd <= frames` (the frame count when `loopEnd` is absent). A `loopEnd`
/// without a `loopStart` doesn't fit.
fn loop_fits(frames: usize, start: Option<u64>, end: Option<u64>) -> bool {
    let frames = frames as u64;
    match start {
        None => end.is_none(),
        Some(s) => {
            let e = end.unwrap_or(frames);
            s < e && e <= frames
        }
    }
}

/// The sound of a file, with the loop its audio entry declares, which fits it: `loopStart` to
/// `loopEnd`, the frame count when that is absent.
pub(super) fn sound(rate: u32, channels: u32, pcm: Arc<[i16]>, start: Option<u64>, end: Option<u64>) -> Sound {
    let frames = (pcm.len() / channels as usize) as u64;
    let looping = start.map(|s| (s as u32, end.unwrap_or(frames) as u32));
    Sound { rate, channels, pcm, looping }
}
