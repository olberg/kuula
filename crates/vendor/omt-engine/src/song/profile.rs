//! The profiles (docs/omt.md section 12) and the tier (section 13) of a song that was read.

use super::*;
use crate::cell::Effect;

/// The profiles this engine knows.
pub const PROFILES: [&str; 3] = ["kuula", "tracker", "pico8"];

fn uses_effect(song: &Song, f: impl Fn(&Effect) -> bool) -> bool {
    song.tracks.iter().any(|t| t.cells.iter().any(|(_, c)| c.effects.iter().any(&f)))
}

fn only_known_engines(song: &Song) -> bool {
    song.instruments.iter().flatten().all(|i| matches!(i.engine, Engine::Wave(_) | Engine::Sampler(_)))
}

/// The rules of profile `name` that the song breaks, or `None` for a profile this version doesn't
/// define (section 12).
pub fn profile_violations(song: &Song, name: &str) -> Option<Vec<String>> {
    let mut v: Vec<&str> = Vec::new();
    match name {
        "kuula" => {
            if song.rate != 44100 {
                v.push("rate");
            }
            if song.channels.len() > 8 {
                v.push("channels");
            }
            if !only_known_engines(song) {
                v.push("engine");
            }
            if song.resampling != Resampling::Nearest {
                v.push("resampling");
            }
            if song.samples.iter().any(|s| s.channels != 1) {
                v.push("stereo-sample");
            }
            let bytes: u64 = song.samples.iter().map(|s| s.frames as u64 * s.channels as u64 * 2).sum();
            if bytes > 2 * 1024 * 1024 {
                v.push("sample-budget");
            }
            if song.tracks.iter().any(|t| t.written_rows > MAX_ROWS as i64) {
                v.push("rows");
            }
            if tier(song) != "exact" {
                v.push("tier");
            }
        }
        "tracker" => {
            if song.rate != 44100 && song.rate != 48000 {
                v.push("rate");
            }
            if !only_known_engines(song) {
                v.push("engine");
            }
        }
        "pico8" => {
            if song.rate != 22050 {
                v.push("rate");
            }
            if song.tick != (183, 22050) {
                v.push("tick");
            }
            if song.volume != 256 {
                v.push("volume");
            }
            if song.channels.len() > 4 || song.channels.iter().any(|c| c.pan != 0 || c.volume != 64) {
                v.push("channels");
            }
            if !song.instruments.iter().flatten().all(|i| matches!(i.engine, Engine::Wave(_))) {
                v.push("engine");
            }
            if tier(song) != "exact" {
                v.push("tier");
            }
            if song.tracks.iter().any(|t| t.rows > 32) {
                v.push("tracks");
            }
            let allowed = |e: &Effect| {
                matches!(e, Effect::FGlide(_) | Effect::Drop(_) | Effect::VGlide(..) | Effect::Vib(..) | Effect::Vibw(..) | Effect::Arp4(..))
            };
            if uses_effect(song, |e| !allowed(e)) {
                v.push("effects");
            }
            if song.arrangements.iter().any(|a| a.global_entries > 0) {
                v.push("global");
            }
            if song.arrangements.iter().map(|a| a.orders.len()).sum::<usize>() > 64 {
                v.push("orders");
            }
        }
        _ => return None,
    }
    Some(v.into_iter().map(String::from).collect())
}

/// The profiles the song fits.
pub fn fits(song: &Song) -> Vec<&'static str> {
    PROFILES
        .iter()
        .copied()
        .filter(|p| profile_violations(song, p).is_some_and(|v| v.is_empty()))
        .collect()
}

/// `exact` or `faithful` (section 13).
pub fn tier(song: &Song) -> &'static str {
    let faithful = !only_known_engines(song)
        || song.samples.iter().any(|s| s.encoding == Encoding::Opus)
        || song.instruments.iter().flatten().any(|i| i.filter)
        || uses_effect(song, |e| matches!(e, Effect::Cutoff(_) | Effect::Reso(_)));
    if faithful { "faithful" } else { "exact" }
}
