//! Versions of a song: `music` naming an arrangement, and the switch that
//! carries the music on in another version from the row and tick it is at
//! while the notes of the one it leaves ring out.

use std::rc::Rc;
use std::sync::Arc;

use omt_engine::player::{Mode, Player};
use omt_engine::song::Song;
use serde_json::{json, Value};

use super::testsong::file;
use super::{Fade, Version, VALUES_PER_FRAME};
use crate::draw::DrawState;
use crate::snapshot::{Snapshot, SnapshotLimits};
use crate::source::CartSource;

/// A song of four versions on two channels, a tick a frame, its one
/// instrument with a release of 60 ms, so a released note rings for three
/// frames and a half. `calm` and `wild` have the same two order rows of
/// 12 ticks and loop; `short` is one row of 6 ticks; `once` ends after its
/// row.
fn payload() -> Value {
    json!({
        "omt": "0.3",
        "rate": 44100,
        "tick": [1, 60],
        "channels": [{}, {}],
        "instruments": [
            {"number": 1, "volume": 32,
             "envelope": {"attack": 0, "decay": 0, "sustain": 64, "release": 60},
             "engine": {"kind": "wave", "waveform": "pulse"}},
        ],
        "tracks": [
            {"rows": 4, "speed": 3, "cells": [[0, "C-4 01"], [2, "E-4"]]},
            {"rows": 4, "speed": 3,
             "cells": [[0, "G-4 01"], [1, "A-4"], [2, "B-4"], [3, "C-5"]]},
            {"rows": 2, "speed": 3, "cells": [[0, "C-3 01"]]},
        ],
        "arrangements": [
            {"name": "calm", "orders": [
                {"tracks": [0, null], "ticks": 12},
                {"tracks": [0, 2], "ticks": 12, "next": 0}]},
            {"name": "wild", "orders": [
                {"tracks": [1, null], "ticks": 12},
                {"tracks": [1, 2], "ticks": 12, "next": 0}]},
            {"name": "short", "orders": [{"tracks": [2, null], "ticks": 6, "next": 0}]},
            {"name": "once", "orders": [{"tracks": [2, null], "ticks": 6}]},
        ],
    })
}

const CALM: usize = 0;
const WILD: usize = 1;

fn song_file() -> Vec<u8> {
    file(&serde_json::to_vec(&payload()).unwrap(), &[])
}

/// A cart with the song as `music/s.omc` and again as `music/t.omc`.
fn state() -> DrawState {
    let entries = vec![("music/s.omc", song_file()), ("music/t.omc", song_file())];
    let cart: Rc<dyn CartSource> =
        Rc::new(Snapshot::from_entries(entries, SnapshotLimits::default()).unwrap());
    DrawState::new(8, 8, cart)
}

fn frames(d: &mut DrawState, frames: usize) -> Vec<i16> {
    let mut out = Vec::with_capacity(frames * VALUES_PER_FRAME);
    for _ in 0..frames {
        out.extend_from_slice(d.audio.render());
    }
    out
}

/// The engine's own players through `steps`, each a version and the
/// frames it plays for. The first starts at the top. Each later one is a
/// new player at the tick after the one that was sounding (at the tick
/// itself when the one before it has rendered nothing), while the player
/// it replaces is finished and rings out beside it, the one tail there is.
fn reference(steps: &[(usize, usize)]) -> Vec<i16> {
    let song: Arc<Song> = Arc::new(
        omt_engine::load(&serde_json::to_vec(&payload()).unwrap(), &[])
            .song
            .unwrap(),
    );
    let mut player = Player::new(song.clone());
    let mut tail: Option<Player> = None;
    let mut fresh = true;
    let mut out = Vec::new();
    for (i, &(version, frames)) in steps.iter().enumerate() {
        if i == 0 {
            player.play(version, 0, 0, Mode::Song);
        } else {
            let (order, sounding) = player.position();
            let tick = if fresh { sounding } else { sounding + 1 };
            let mut next = Player::new(song.clone());
            next.play(version, order, tick, Mode::Song);
            let mut left = std::mem::replace(&mut player, next);
            left.finish();
            tail = Some(left);
            fresh = true;
        }
        for _ in 0..frames {
            let mut pcm = vec![0i16; VALUES_PER_FRAME];
            player.render(&mut pcm);
            fresh = false;
            if let Some(t) = &mut tail {
                let mut ringing = vec![0i16; VALUES_PER_FRAME];
                t.render(&mut ringing);
                for (a, &b) in pcm.iter_mut().zip(&ringing) {
                    *a = (*a as i32 + b as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                }
                if t.silent() {
                    tail = None;
                }
            }
            out.extend(pcm);
        }
    }
    out
}

/// The version named alone from where a switch after `played` frames of
/// `from` puts it: what a switch that cut the old notes would give.
fn cut_switch(from: usize, played: usize, to: usize, frames: usize) -> Vec<i16> {
    let song: Arc<Song> = Arc::new(
        omt_engine::load(&serde_json::to_vec(&payload()).unwrap(), &[])
            .song
            .unwrap(),
    );
    let mut player = Player::new(song);
    player.play(from, 0, 0, Mode::Song);
    let mut pcm = vec![0i16; played * VALUES_PER_FRAME];
    player.render(&mut pcm);
    let (order, sounding) = player.position();
    player.play(to, order, sounding + 1, Mode::Song);
    let mut pcm = vec![0i16; frames * VALUES_PER_FRAME];
    player.render(&mut pcm);
    pcm
}

fn position(d: &DrawState) -> (usize, u64) {
    d.audio.music.as_ref().unwrap().player.position()
}

#[test]
fn a_version_is_an_arrangement_by_name_or_by_number() {
    for version in [Version::Name("wild"), Version::Number(1)] {
        let mut d = state();
        d.music(Some("s"), 0, Some(version)).unwrap();
        assert_eq!(frames(&mut d, 30), reference(&[(WILD, 30)]));
    }
    // Without one, the arrangement the file selects: the first.
    for version in [None, Some(Version::Name("calm")), Some(Version::Number(0))] {
        let mut d = state();
        d.music(Some("s"), 0, version).unwrap();
        assert_eq!(frames(&mut d, 30), reference(&[(CALM, 30)]));
    }
    assert_ne!(reference(&[(CALM, 30)]), reference(&[(WILD, 30)]));
}

#[test]
fn a_switch_carries_on_at_the_same_row_and_tick() {
    let mut d = state();
    d.music(Some("s"), 0, None).unwrap();
    let mut got = frames(&mut d, 17);
    // Seventeen ticks have sounded: the last was the fifth of the second
    // order row, and the version starts at its sixth.
    assert_eq!(position(&d), (1, 4));
    d.music(Some("s"), 0, Some(Version::Name("wild"))).unwrap();
    assert_eq!(position(&d), (1, 5));
    got.extend(frames(&mut d, 20));
    assert_eq!(got, reference(&[(CALM, 17), (WILD, 20)]));

    // It is where a console that played the other version all along is.
    let mut all_along = state();
    all_along
        .music(Some("s"), 0, Some(Version::Name("wild")))
        .unwrap();
    frames(&mut all_along, 37);
    assert_eq!(position(&d), position(&all_along));

    // And back, as often as a cart likes.
    d.music(Some("s"), 0, Some(Version::Name("calm"))).unwrap();
    got.extend(frames(&mut d, 9));
    assert_eq!(got, reference(&[(CALM, 17), (WILD, 20), (CALM, 9)]));
}

#[test]
fn naming_the_version_that_plays_changes_nothing() {
    let mut d = state();
    d.music(Some("s"), 0, Some(Version::Name("wild"))).unwrap();
    let mut got = frames(&mut d, 7);
    d.music(Some("s"), 0, Some(Version::Number(1))).unwrap();
    got.extend(frames(&mut d, 20));
    assert_eq!(got, reference(&[(WILD, 27)]));
}

#[test]
fn without_a_version_the_song_starts_again() {
    let mut d = state();
    d.music(Some("s"), 0, None).unwrap();
    frames(&mut d, 7);
    d.music(Some("s"), 0, None).unwrap();
    assert_eq!(frames(&mut d, 20), reference(&[(CALM, 20)]));
}

#[test]
fn a_position_the_version_lacks_starts_it_at_its_beginning() {
    // `short` has one order row of 6 ticks: no second row, and no ninth
    // tick in its first.
    for played in [17, 8] {
        let mut d = state();
        d.music(Some("s"), 0, None).unwrap();
        frames(&mut d, played);
        d.music(Some("s"), 0, Some(Version::Name("short"))).unwrap();
        assert_eq!(position(&d), (0, 0), "after {played} frames");
        // The version from its top, once the note it left has rung out.
        let rung = 5 * VALUES_PER_FRAME;
        assert_eq!(frames(&mut d, 12)[rung..], reference(&[(2, 12)])[rung..]);
    }
    // A tick it has is kept, and so is the end of its row, from which it
    // goes round to its beginning by itself.
    for (played, tick) in [(4, 4), (6, 6)] {
        let mut d = state();
        d.music(Some("s"), 0, None).unwrap();
        frames(&mut d, played);
        d.music(Some("s"), 0, Some(Version::Name("short"))).unwrap();
        assert_eq!(position(&d), (0, tick));
        frames(&mut d, 1);
        assert_eq!(position(&d), (0, tick % 6));
    }
}

#[test]
fn a_song_that_has_not_sounded_yet_switches_at_the_tick_it_is_at() {
    // Started and switched in one frame: nothing of the first tick is lost.
    let mut d = state();
    d.music(Some("s"), 0, None).unwrap();
    d.music(Some("s"), 0, Some(Version::Name("wild"))).unwrap();
    assert_eq!(position(&d), (0, 0));
    assert_eq!(frames(&mut d, 20), reference(&[(WILD, 20)]));

    // Switched twice in one frame: the second switch does not skip a tick.
    // The notes were released all the same, and the second switch cut
    // their tail, so it is not the song undisturbed.
    let mut d = state();
    d.music(Some("s"), 0, None).unwrap();
    let mut got = frames(&mut d, 5);
    d.music(Some("s"), 0, Some(Version::Name("wild"))).unwrap();
    d.music(Some("s"), 0, Some(Version::Name("calm"))).unwrap();
    assert_eq!(position(&d), (0, 5));
    got.extend(frames(&mut d, 20));
    assert_eq!(got, reference(&[(CALM, 5), (WILD, 0), (CALM, 20)]));
    assert_ne!(got, reference(&[(CALM, 25)]));
}

#[test]
fn the_version_it_leaves_rings_out_under_the_new_one() {
    let mut d = state();
    d.music(Some("s"), 0, None).unwrap();
    frames(&mut d, 4);
    d.music(Some("s"), 0, Some(Version::Name("wild"))).unwrap();
    let got = frames(&mut d, 12);
    // Not the new version alone: the old note is in the first frames,
    // and gone after its release.
    let alone = cut_switch(CALM, 4, WILD, 12);
    assert_ne!(got[..VALUES_PER_FRAME], alone[..VALUES_PER_FRAME]);
    assert_eq!(got[5 * VALUES_PER_FRAME..], alone[5 * VALUES_PER_FRAME..]);
    assert!(d.audio.music.as_ref().unwrap().tail.is_none());

    // The tail has the channel's gain and the music's fade, and goes when
    // the music is stopped.
    let mut d = state();
    d.music(Some("s"), 0, None).unwrap();
    frames(&mut d, 4);
    d.music(Some("s"), 0, Some(Version::Name("wild"))).unwrap();
    d.audio.set_volume(0, 0).unwrap();
    assert!(frames(&mut d, 1).iter().all(|&s| s == 0));
    d.audio.set_volume(0, super::GAIN_ONE).unwrap();
    assert!(d.audio.music.as_ref().unwrap().tail.is_some());
    d.music(None, 0, None).unwrap();
    assert!(frames(&mut d, 2).iter().all(|&s| s == 0));
}

#[test]
fn a_switch_keeps_the_fade_and_does_not_read_its_own() {
    let mut d = state();
    d.music(Some("s"), 8, None).unwrap();
    frames(&mut d, 3);
    assert_eq!(d.audio.music.as_ref().unwrap().gain, 96);
    d.music(Some("s"), 100, Some(Version::Name("wild")))
        .unwrap();
    let m = d.audio.music.as_ref().unwrap();
    assert_eq!(m.gain, 96);
    assert!(matches!(m.fade, Fade::In(32)));
    frames(&mut d, 1);
    assert_eq!(d.audio.music.as_ref().unwrap().gain, 128);
}

#[test]
fn music_that_is_fading_out_starts_again_instead_of_switching() {
    // A run ends and the music fades out; the next run begins before the
    // fade is over and names a version, the other one or the same.
    for version in ["wild", "calm"] {
        let mut d = state();
        d.music(Some("s"), 0, None).unwrap();
        frames(&mut d, 7);
        d.music(None, 60, None).unwrap();
        frames(&mut d, 5);
        assert!(matches!(d.audio.music.as_ref().unwrap().fade, Fade::Out(_)));
        d.music(Some("s"), 0, Some(Version::Name(version))).unwrap();
        let m = d.audio.music.as_ref().unwrap();
        assert_eq!((m.gain, m.fade), (super::GAIN_ONE, Fade::None));
        assert_eq!(position(&d), (0, 0));
        let from_the_top = if version == "wild" { WILD } else { CALM };
        assert_eq!(frames(&mut d, 70), reference(&[(from_the_top, 70)]));
        assert!(d.audio.music_playing(), "the fade out is gone");
    }
    // Its own fade is read, as for any song that starts.
    let mut d = state();
    d.music(Some("s"), 0, None).unwrap();
    d.music(None, 60, None).unwrap();
    d.music(Some("s"), 8, Some(Version::Name("wild"))).unwrap();
    let m = d.audio.music.as_ref().unwrap();
    assert_eq!((m.gain, m.fade), (0, Fade::In(32)));
}

#[test]
fn another_song_or_one_that_has_ended_starts_at_its_beginning() {
    let mut d = state();
    d.music(Some("s"), 0, None).unwrap();
    frames(&mut d, 7);
    // The same notes in another file are another song.
    d.music(Some("t"), 0, Some(Version::Name("wild"))).unwrap();
    assert_eq!(frames(&mut d, 20), reference(&[(WILD, 20)]));

    // `once` ends after six ticks; the music is gone by then.
    let mut d = state();
    d.music(Some("s"), 0, Some(Version::Name("once"))).unwrap();
    frames(&mut d, 10);
    assert!(!d.audio.music_playing());
    d.music(Some("s"), 0, Some(Version::Name("wild"))).unwrap();
    assert_eq!(frames(&mut d, 20), reference(&[(WILD, 20)]));
}

#[test]
fn a_version_the_song_lacks_is_an_error_that_refuses_nothing() {
    let mut d = state();
    assert_eq!(
        d.music(Some("s"), 0, Some(Version::Name("nope")))
            .unwrap_err()
            .to_string(),
        "version_not_found: music/s.omc has no version \"nope\""
    );
    assert!(!d.audio.music_playing());
    for n in [4, -1, i64::MAX] {
        assert_eq!(
            d.music(Some("s"), 0, Some(Version::Number(n)))
                .unwrap_err()
                .to_string(),
            format!("version_not_found: music/s.omc has no version {n}")
        );
    }
    // The music that plays goes on through a call that fails.
    d.music(Some("s"), 0, None).unwrap();
    let mut got = frames(&mut d, 5);
    assert!(d.music(Some("s"), 0, Some(Version::Name("nope"))).is_err());
    got.extend(frames(&mut d, 5));
    assert_eq!(got, reference(&[(CALM, 10)]));
    d.music(Some("s"), 0, Some(Version::Name("wild"))).unwrap();
    assert_eq!(position(&d), (0, 10));
}
