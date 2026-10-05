//! Cue banks: loading with its bounds and refusals, the channels a cue
//! takes, triggers and the console's variation numbers, plain cues, and
//! `stop`. A cue played alone at unity is the engine's own rendering of it.

use std::rc::Rc;
use std::sync::Arc;

use omt_engine::omc::{self, CueAudioOut};
use omt_engine::omq::{self, Bank};
use serde_json::{json, Value};

use super::cues::{self, LoadedBank};
use super::sample::{encode_wav, SAMPLE_BUDGET};
use super::songs::SONG_BUDGET;
use super::testsong::{omc as song_file, Song};
use super::{AudioError, Mixer, Trigger, VALUES_PER_FRAME, VARIATION_SEED};
use crate::draw::DrawState;
use crate::snapshot::{Snapshot, SnapshotLimits};
use crate::source::CartSource;

const PATH: &str = "cues/fx.omc";

/// A bank of one pulse instrument and two tracks, a note of 6 ticks and a
/// note of 12, with a tick a frame: `hit` (one track), `chord` (both),
/// `hum` (looping), `varied` (with ranges) and `file` (a plain cue).
fn payload() -> Value {
    json!({
        "omq": "0.2",
        "rate": 44100,
        "tick": [1, 60],
        "instruments": [
            {"number": 1, "volume": 32, "engine": {"kind": "wave", "waveform": "pulse"}},
        ],
        "tracks": [
            {"rows": 1, "speed": 6, "cells": [[0, "C-4 01"]]},
            {"rows": 1, "speed": 12, "cells": [[0, "E-4 01"]]},
        ],
        "cues": [
            {"name": "hit", "tracks": [0]},
            {"name": "chord", "tracks": [0, 1]},
            {"name": "hum", "tracks": [1], "loop": true},
            {"name": "varied", "tracks": [0],
             "vary": {"transpose": [-256, 256], "gain": [128, 256]}},
            {"name": "file", "audio": "click"},
        ],
    })
}

/// A mono 16-bit WAVE of `frames` frames, each of value 8000.
fn click(frames: usize) -> Vec<u8> {
    encode_wav(44100, 16, 1, &8000i16.to_le_bytes().repeat(frames))
}

/// The bank file of `payload` with the audio files `audio`, by name.
fn file(payload: &Value, audio: &[(&str, &[u8])]) -> Vec<u8> {
    let audio: Vec<CueAudioOut> = audio
        .iter()
        .map(|(name, data)| CueAudioOut {
            name,
            data,
            media_type: None,
            loop_start: None,
            loop_end: None,
        })
        .collect();
    omc::write_bank(
        &serde_json::to_vec(payload).unwrap(),
        &[],
        &audio,
        None,
        "kuula tests",
    )
}

fn bank_file() -> Vec<u8> {
    file(&payload(), &[("click", &click(1470))])
}

fn load(bytes: &[u8]) -> Result<LoadedBank, AudioError> {
    cues::load(PATH, bytes, SONG_BUDGET, SAMPLE_BUDGET)
}

/// The message of a refused load, `code: path: why`.
fn refused(bytes: &[u8]) -> String {
    load(bytes).expect_err("refused").to_string()
}

fn state(entries: Vec<(&str, Vec<u8>)>) -> DrawState {
    let cart: Rc<dyn CartSource> =
        Rc::new(Snapshot::from_entries(entries, SnapshotLimits::default()).unwrap());
    DrawState::new(8, 8, cart)
}

/// `frames` frames of the mixer, one after the other.
fn frames(m: &mut Mixer, frames: usize) -> Vec<i16> {
    let mut out = Vec::with_capacity(frames * VALUES_PER_FRAME);
    for _ in 0..frames {
        out.extend_from_slice(m.render());
    }
    out
}

/// The engine's own rendering of cue `name` with `trigger`, padded with
/// silence to `frames` frames of the console.
fn engine(bank: &Bank, name: &str, trigger: Trigger, frames: usize) -> Vec<i16> {
    let cue = bank.cue_named(name).unwrap();
    let mut pcm = omq::render(bank, cue, trigger).pcm;
    assert!(pcm.len() <= frames * VALUES_PER_FRAME, "{}", pcm.len());
    pcm.resize(frames * VALUES_PER_FRAME, 0);
    pcm
}

/// A mixer playing cue `name` of the test bank with `trigger`.
fn playing(name: &str, trigger: Option<Trigger>) -> (Mixer, Arc<Bank>, usize) {
    let bank = load(&bank_file()).unwrap().bank;
    let mut m = Mixer::new();
    let cue = bank.cue_named(name).unwrap();
    let channel = m.play_cue(PATH, &bank, cue, trigger, None).unwrap();
    (m, bank, channel.unwrap())
}

#[test]
fn a_bank_loads_with_its_cost() {
    let got = load(&bank_file()).unwrap();
    assert_eq!(
        got.song_bytes,
        serde_json::to_vec(&payload()).unwrap().len()
    );
    // The plain cue's file, 2 bytes a frame; the bank has no sample record.
    assert_eq!(got.sample_bytes, 2940);
    assert_eq!(got.bank.cues.len(), 5);
}

#[test]
fn cues_play_by_name_from_a_bank_loaded_once() {
    let mut d = state(vec![
        (PATH, bank_file()),
        ("cues/song.omc", song_file(&Song::new(1))),
        ("cues/bad.omc", b"not a container".to_vec()),
    ]);
    // One channel for each track, from the highest free run down.
    assert_eq!(d.cue("fx", "hit", None, None).unwrap(), Some(7));
    assert_eq!(d.cue("fx", "chord", None, None).unwrap(), Some(5));
    assert_eq!(d.cue("fx", "file", None, None).unwrap(), Some(4));
    assert_eq!(d.cue("fx", "hit", None, Some(2)).unwrap(), Some(2));
    let used = (d.audio.songs().used(), d.audio.samples().used());
    assert_eq!(used.1, 2940);
    assert_eq!(d.cue("fx", "hit", None, None).unwrap(), Some(3));
    assert_eq!((d.audio.songs().used(), d.audio.samples().used()), used);

    assert_eq!(
        d.cue("fx", "zap", None, None).unwrap_err().to_string(),
        "cue_not_found: cues/fx.omc has no cue \"zap\""
    );
    assert_eq!(
        d.cue("nope", "hit", None, None).unwrap_err().code(),
        "asset_not_found"
    );
    assert_eq!(
        d.cue("../x", "hit", None, None).unwrap_err().code(),
        "asset_invalid"
    );
    assert_eq!(
        d.cue("song", "hit", None, None).unwrap_err().to_string(),
        "song_error: cues/song.omc: no Open Module Cues bank in the file"
    );
    assert_eq!(
        d.cue("bad", "hit", None, None).unwrap_err().code(),
        "song_error"
    );
    // A cue that does not fit from the channel named, and a bad channel.
    assert_eq!(
        d.cue("fx", "chord", None, Some(7)).unwrap_err().to_string(),
        "audio_no_room: cues/fx.omc: cue \"chord\" has 2 channels, which do not fit from channel 7"
    );
    assert_eq!(
        d.cue("fx", "hit", None, Some(8)).unwrap_err().code(),
        "audio_bad_channel"
    );
    // A bank is not a song, and the other way round.
    assert_eq!(d.sfx("x", None).unwrap_err().code(), "asset_not_found");
}

#[test]
fn a_cue_alone_is_the_engines_rendering() {
    for name in ["hit", "chord", "file"] {
        let (mut m, bank, _) = playing(name, Some(Trigger::default()));
        assert_eq!(
            frames(&mut m, 40),
            engine(&bank, name, Trigger::default(), 40),
            "{name}"
        );
    }
}

#[test]
fn a_trigger_transposes_and_scales() {
    let t = Trigger {
        transpose: 3072,
        gain: 128,
    };
    for name in ["hit", "chord", "file"] {
        let (mut m, bank, _) = playing(name, Some(t));
        let got = frames(&mut m, 40);
        assert_eq!(got, engine(&bank, name, t, 40), "{name}");
        assert_ne!(got, engine(&bank, name, Trigger::default(), 40), "{name}");
    }
    // A trigger outside its limits is held to them.
    let wild = Trigger {
        transpose: 99_999,
        gain: 999,
    };
    let (mut m, bank, _) = playing("hit", Some(wild));
    assert_eq!(frames(&mut m, 40), engine(&bank, "hit", wild.clamped(), 40));
}

#[test]
fn the_channels_gain_and_the_triggers_are_one_number() {
    let half = Trigger {
        transpose: 0,
        gain: 128,
    };
    let quarter = Trigger {
        transpose: 0,
        gain: 64,
    };
    // A tracked cue and a plain one alike: half and half is a quarter,
    // on the voice, where the engine's own gain enters.
    for name in ["hit", "file"] {
        let (mut m, bank, channel) = playing(name, Some(half));
        m.set_volume(channel as i64, 128).unwrap();
        assert_eq!(
            frames(&mut m, 40),
            engine(&bank, name, quarter, 40),
            "{name}"
        );
    }
    // Each channel of a cue has its console channel's gain.
    let (mut m, bank, channel) = playing("chord", Some(Trigger::default()));
    m.set_volume(channel as i64 + 1, 0).unwrap();
    let got = frames(&mut m, 40);
    assert_ne!(got, engine(&bank, "chord", Trigger::default(), 40));
    assert_eq!(got, engine(&bank, "hit", Trigger::default(), 40));
}

#[test]
fn the_console_varies_a_cue_by_a_fixed_sequence() {
    // The first numbers of the sequence, and the triggers the cue's ranges
    // make of them.
    let mut x = VARIATION_SEED;
    let mut numbers = Vec::new();
    for _ in 0..2 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        numbers.push(x);
    }
    assert_eq!(numbers, [0xE124_B63A, 0x8B9A_74AB]);

    let bank = load(&bank_file()).unwrap().bank;
    let cue = bank.cue_named("varied").unwrap();
    let triggers: Vec<Trigger> = numbers.iter().map(|&u| bank.cues[cue].varied(u)).collect();
    assert_eq!(
        triggers,
        [
            Trigger {
                transpose: 109,
                gain: 241
            },
            Trigger {
                transpose: -23,
                gain: 198
            }
        ]
    );

    let mut m = Mixer::new();
    // A call that is refused steps nothing.
    assert!(m.play_cue(PATH, &bank, cue, None, Some(9)).is_err());
    for t in triggers {
        m.stop_all();
        m.play_cue(PATH, &bank, cue, None, None).unwrap();
        assert_eq!(frames(&mut m, 40), engine(&bank, "varied", t, 40));
    }
    // A trigger the cart gives is not a variation, and steps nothing.
    m.stop_all();
    m.play_cue(PATH, &bank, cue, Some(Trigger::default()), None)
        .unwrap();
    assert_eq!(m.variation, 0x8B9A_74AB);
    // Nor does a cue that is not played, every channel being held by a
    // sound placed by name.
    m.stop_all();
    for c in 0..8 {
        m.play_cue(PATH, &bank, cue, Some(Trigger::default()), Some(c))
            .unwrap();
    }
    assert_eq!(m.play_cue(PATH, &bank, cue, None, None).unwrap(), None);
    assert_eq!(m.variation, 0x8B9A_74AB);
    // A cue without ranges is the same for every number.
    let hit = bank.cue_named("hit").unwrap();
    m.stop_all();
    m.play_cue(PATH, &bank, hit, None, None).unwrap();
    assert_eq!(
        frames(&mut m, 40),
        engine(&bank, "hit", Trigger::default(), 40)
    );
}

#[test]
fn a_cue_holds_its_channels_until_it_ends() {
    // Six ticks of a frame each: the end is met in frame 7, as an effect's.
    let (mut m, bank, channel) = playing("hit", None);
    assert_eq!(channel, 7);
    for _ in 0..6 {
        m.render();
        assert!(m.is_playing(7));
    }
    m.render();
    assert!(!m.is_playing(7));
    let hit = bank.cue_named("hit").unwrap();
    assert_eq!(m.play_cue(PATH, &bank, hit, None, None).unwrap(), Some(7));

    // A plain cue ends when its file runs out. This one is two frames
    // long to the sample, and its voice is found at its end by the frame
    // after, which has nothing of it: the channel is free from the fourth.
    let (mut m, _, channel) = playing("file", None);
    for _ in 0..2 {
        assert!(m.render().iter().any(|&s| s != 0));
        assert!(m.is_playing(channel));
    }
    assert!(m.render().iter().all(|&s| s == 0));
    assert!(!m.is_playing(channel));
}

#[test]
fn stop_releases_a_looping_cue_and_cut_silences_it() {
    let (mut m, _, channel) = playing("hum", None);
    for _ in 0..40 {
        m.render();
    }
    assert!(m.is_playing(channel), "a looping cue goes on");
    assert!(m.render().iter().any(|&s| s != 0));

    // The instrument has no envelope, so a released voice is silent at
    // once and the cue has ended by the next frame.
    m.stop(channel as i64, false).unwrap();
    m.render();
    assert!(!m.is_playing(channel));
    assert!(m.render().iter().all(|&s| s == 0));

    let (mut m, _, channel) = playing("hum", None);
    m.render();
    m.stop(channel as i64, true).unwrap();
    assert!(!m.is_playing(channel));
    assert!(m.render().iter().all(|&s| s == 0));

    // Nothing holds the channel: nothing happens. A bad channel is one.
    m.stop(3, false).unwrap();
    assert_eq!(m.stop(8, false).unwrap_err().code(), "audio_bad_channel");

    // `stop` cuts a sample and a plain cue, which have no release.
    let (mut m, _, channel) = playing("file", None);
    m.stop(channel as i64, false).unwrap();
    assert!(m.render().iter().all(|&s| s == 0));
}

#[test]
fn a_named_channel_cuts_what_holds_the_run() {
    let (mut m, bank, _) = playing("hum", None);
    let chord = bank.cue_named("chord").unwrap();
    // The chord takes 6 and 7, cutting the hum on 7.
    assert_eq!(
        m.play_cue(PATH, &bank, chord, None, Some(6)).unwrap(),
        Some(6)
    );
    let mut alone = Mixer::new();
    alone.play_cue(PATH, &bank, chord, None, Some(6)).unwrap();
    assert_eq!(frames(&mut m, 20), frames(&mut alone, 20));
}

#[test]
fn a_file_that_is_not_a_bank_is_a_song_error() {
    assert_eq!(
        refused(b"not a container"),
        "song_error: cues/fx.omc: not an OMC file"
    );
    assert_eq!(
        refused(&song_file(&Song::new(1))),
        "song_error: cues/fx.omc: no Open Module Cues bank in the file"
    );
    let mut p = payload();
    p["omq"] = json!("0.1");
    assert_eq!(
        refused(&file(&p, &[("click", &click(10))])),
        "song_error: cues/fx.omc: unknown-version at omq"
    );
    let mut p = payload();
    p["cues"][0]["tracks"] = json!([7]);
    assert_eq!(
        refused(&file(&p, &[("click", &click(10))])),
        "song_error: cues/fx.omc: bad-reference at cues[0].tracks[0]"
    );
    let mut p = payload();
    p["cues"][1]["name"] = json!("hit");
    assert_eq!(
        refused(&file(&p, &[("click", &click(10))])),
        "song_error: cues/fx.omc: duplicate-name at cues[1].name"
    );
}

#[test]
fn a_bank_fits_the_kuula_profile_whichever_it_names() {
    let with = |change: fn(&mut Value)| {
        let mut p = payload();
        change(&mut p);
        refused(&file(&p, &[("click", &click(10))]))
    };
    assert_eq!(
        with(|p| p["cues"][1]["tracks"] = json!([0, 1, 0, 1, 0, 1, 0, 1, 0])),
        "song_error: cues/fx.omc: breaks the kuula profile: channels"
    );
    assert_eq!(
        with(|p| p["rate"] = json!(22050)),
        "song_error: cues/fx.omc: breaks the kuula profile: rate"
    );
    assert_eq!(
        with(|p| p["resampling"] = json!("linear")),
        "song_error: cues/fx.omc: breaks the kuula profile: resampling"
    );
    // The rule is named the same way when the bank promises the profile,
    // and when it names one the format does not have.
    assert_eq!(
        with(|p| {
            p["profile"] = json!("kuula");
            p["rate"] = json!(22050);
        }),
        "song_error: cues/fx.omc: breaks the kuula profile: rate"
    );
    assert_eq!(
        with(|p| {
            p["profile"] = json!("tracker");
            p["rate"] = json!(22050);
        }),
        "song_error: cues/fx.omc: breaks the kuula profile: rate"
    );
    let mut p = payload();
    p["profile"] = json!("kuula");
    assert!(load(&file(&p, &[("click", &click(10))])).is_ok());
    p["profile"] = json!("tracker");
    assert!(load(&file(&p, &[("click", &click(10))])).is_ok());

    // A stereo file is the profile's rule too, once it is within the
    // budget and so decoded.
    let stereo = encode_wav(44100, 16, 2, &[0; 400]);
    assert_eq!(
        refused(&file(&payload(), &[("click", &stereo)])),
        "song_error: cues/fx.omc: breaks the kuula profile: stereo-audio"
    );
    // A file the format does not play exactly makes the bank faithful:
    // one of another kind, and a FLAC that does not state its length.
    assert_eq!(
        refused(&file(&payload(), &[("click", b"OggS and so on")])),
        "song_error: cues/fx.omc: breaks the kuula profile: tier"
    );
    assert_eq!(
        refused(&file(&payload(), &[("click", &flac_header(1, 0))])),
        "song_error: cues/fx.omc: breaks the kuula profile: tier"
    );
}

/// A FLAC stream that is its header alone: the marker and a STREAMINFO
/// for 16-bit samples that says `total` frames, with no frame after it.
fn flac_header(channels: u64, total: u64) -> Vec<u8> {
    let mut info = Vec::new();
    info.extend_from_slice(&4096u16.to_be_bytes());
    info.extend_from_slice(&4096u16.to_be_bytes());
    info.extend_from_slice(&[0; 6]);
    let bits = (44100u64 << 44) | ((channels - 1) << 41) | (15 << 36) | total;
    info.extend_from_slice(&bits.to_be_bytes());
    info.extend_from_slice(&[0; 16]);
    let mut out = b"fLaC".to_vec();
    out.extend_from_slice(&[0x80, 0, 0, info.len() as u8]);
    out.extend_from_slice(&info);
    out
}

#[test]
fn a_plain_cues_file_is_bounded_before_it_is_decoded() {
    let p = payload();
    // A WAVE of more frames than the sample budget has room for.
    let long = click(SAMPLE_BUDGET / 2 + 1);
    assert_eq!(
        refused(&file(&p, &[("click", &long)])),
        format!(
            "sample_error: cues/fx.omc: cue audio \"click\" is damaged, or longer than the sample budget has room for; the sample budget has {SAMPLE_BUDGET} left of {SAMPLE_BUDGET}"
        )
    );
    assert!(load(&file(&p, &[("click", &click(SAMPLE_BUDGET / 2))])).is_ok());
    // What the bank's cart has loaded already counts.
    let two = click(1000);
    assert!(cues::load(PATH, &file(&p, &[("click", &two)]), SONG_BUDGET, 2000).is_ok());
    assert_eq!(
        cues::load(PATH, &file(&p, &[("click", &two)]), SONG_BUDGET, 1999)
            .unwrap_err()
            .code(),
        "sample_error"
    );
    // Two channels are twice the samples.
    let stereo = encode_wav(44100, 16, 2, &[0; 4000]);
    assert_eq!(
        cues::load(PATH, &file(&p, &[("click", &stereo)]), SONG_BUDGET, 3999)
            .unwrap_err()
            .to_string(),
        "sample_error: cues/fx.omc: cue audio \"click\" is 4000 bytes of samples; the sample budget has 3999 left of 2097152"
    );

    // A FLAC is taken at the length its header states, which is checked
    // before a frame of it is decoded: 32 MiB in 42 bytes.
    let huge = flac_header(1, 1 << 24);
    assert_eq!(
        refused(&file(&p, &[("click", &huge)])),
        format!(
            "sample_error: cues/fx.omc: cue audio \"click\" is 33554432 bytes of samples; the sample budget has {SAMPLE_BUDGET} left of {SAMPLE_BUDGET}"
        )
    );
    // One whose frames are not the length it states is damaged, to the
    // validator: it is decoded no further than that length.
    let empty = flac_header(1, 100);
    assert_eq!(
        refused(&file(&p, &[("click", &empty)])),
        "song_error: cues/fx.omc: bad-audio at cues[4].audio"
    );
}

#[test]
fn every_cue_of_a_bank_can_play() {
    // The cue names a file the bank's container does not have.
    assert_eq!(
        refused(&file(&payload(), &[])),
        "song_error: cues/fx.omc: bad-reference at cues[4].audio"
    );
    // The file's chunk is damaged: the bank is exact and fits the profile,
    // and its cue has nothing to play.
    let mut damaged = file(&payload(), &[("click", &click(10))]);
    let at = damaged.len() - 12;
    damaged[at] ^= 0xff;
    assert_eq!(
        refused(&damaged),
        "song_error: cues/fx.omc: cue \"file\" has nothing to play: its audio is damaged or lost"
    );
    // An audio file nothing names costs nothing and is not looked at.
    let spare = flac_header(2, 1 << 24);
    let got = load(&file(
        &payload(),
        &[("click", &click(10)), ("spare", &spare)],
    ))
    .unwrap();
    assert_eq!(got.sample_bytes, 20);
}

#[test]
fn a_refused_bank_stays_refused_and_charges_nothing() {
    let mut p = payload();
    p["cues"][0]["tracks"] = json!([9]);
    let mut d = state(vec![(PATH, file(&p, &[("click", &click(10))]))]);
    for _ in 0..2 {
        assert_eq!(
            d.cue("fx", "hit", None, None).unwrap_err().to_string(),
            "song_error: cues/fx.omc: bad-reference at cues[0].tracks[0]"
        );
    }
    assert_eq!(d.audio.songs().used(), 0);
    assert_eq!(d.audio.samples().used(), 0);
}

/// `docs/songs.md`, the public page on song files, shows a whole bank. It
/// loads as written, so the page cannot drift from what Kuula takes.
#[test]
fn the_bank_on_the_song_files_page_loads() {
    let page = include_str!("../../../../docs/songs.md").replace("\r\n", "\n");
    let banks: Vec<&str> = page
        .split("```json\n")
        .skip(1)
        .map(|rest| rest.split("```").next().unwrap())
        .filter(|text| text.contains("\"omq\": \"0.2\""))
        .collect();
    assert_eq!(banks.len(), 1, "the page has one example bank");
    let payload: Value = serde_json::from_str(banks[0]).unwrap();
    let got = load(&file(&payload, &[("click", &click(100))])).unwrap();
    let names: Vec<&str> = got.bank.cues.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["zap", "crash", "click"]);
    assert_eq!(got.sample_bytes, 200);
}
