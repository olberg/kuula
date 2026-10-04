//! The mixer against its specification, with small in-memory songs.

use super::sample::{decode_wav, encode_wav, SAMPLE_BUDGET};
use super::songs::{self, PlayableSong, SONG_BUDGET};
use super::testsong::{omc, Song};
use super::*;

fn playable(song: &Song) -> PlayableSong {
    songs::load("sfx/t.omc", &omc(song), SONG_BUDGET, SAMPLE_BUDGET)
        .unwrap()
        .playable
}

/// An effect from `channel` (or the best run); the channel it took.
fn sfx(m: &mut Mixer, song: &Song, channel: Option<i64>) -> usize {
    m.play_effect("sfx/t.omc", &playable(song), channel)
        .unwrap()
}

fn music(m: &mut Mixer, song: &Song, fade: u32) {
    m.play_music(&playable(song), fade);
}

/// A song that outlasts any test: `n` channels, held notes.
fn long(n: usize) -> Song {
    Song::new(n).ticks(100_000)
}

fn frame(m: &mut Mixer) -> Vec<i16> {
    m.render().to_vec()
}

fn frames(m: &mut Mixer, n: usize) {
    for _ in 0..n {
        m.render();
    }
}

fn left(f: &[i16]) -> Vec<i16> {
    f.iter().step_by(2).copied().collect()
}

fn right(f: &[i16]) -> Vec<i16> {
    f.iter().skip(1).step_by(2).copied().collect()
}

fn energy(v: &[i16]) -> i64 {
    v.iter().map(|&s| (s as i64).abs()).sum()
}

fn extremes(f: &[i16]) -> (i16, i16) {
    (*f.iter().min().unwrap(), *f.iter().max().unwrap())
}

fn sample(data: &[u8]) -> Rc<Sample> {
    Rc::new(decode_wav("samples/t.wav", &encode_wav(44100, 8, 1, data)).unwrap())
}

#[test]
fn a_frame_is_735_stereo_sample_frames() {
    assert_eq!(SAMPLES_PER_FRAME, 735);
    assert_eq!(OUTPUT_CHANNELS, 2);
    assert_eq!(VALUES_PER_FRAME, 1470);
    let mut m = Mixer::new();
    let out = m.render();
    assert_eq!(out.len(), 1470);
    assert!(out.iter().all(|&s| s == 0));
    assert_eq!(m.output().len(), 1470);
}

#[test]
fn effects_go_to_the_highest_class_0_run_and_cut_only_when_every_run_is_held() {
    let mut m = Mixer::new();
    let one = long(1);
    // No music: every channel is class 0, highest first.
    let taken: Vec<usize> = (0..8).map(|_| sfx(&mut m, &one, None)).collect();
    assert_eq!(taken, [7, 6, 5, 4, 3, 2, 1, 0]);
    // Every channel is held (class 3): the highest is cut.
    assert_eq!(sfx(&mut m, &one, None), 7);
    assert_eq!(m.effects.len(), 8);
}

#[test]
fn the_four_classes_with_music_in_the_way() {
    let mut m = Mixer::new();
    // Channel 0 and 3 sound, 1 is quiet (no track), 2 is reserved.
    let song = long(4).silent(1).reserved(2).looping();
    music(&mut m, &song, 0);
    m.render();
    let one = long(1);
    // Class 0: beyond the music's channels (7 to 4), then the reserved 2.
    // Class 1: the quiet music channel 1. Class 2: sounding, highest first.
    // Then every channel is held: class 3.
    let taken: Vec<usize> = (0..9).map(|_| sfx(&mut m, &one, None)).collect();
    assert_eq!(taken, [7, 6, 5, 4, 2, 1, 3, 0, 7]);
}

#[test]
fn a_run_is_classed_by_its_worst_channel() {
    let mut m = Mixer::new();
    music(&mut m, &long(4).silent(1).reserved(2).looping(), 0);
    m.render();
    let two = long(2);
    assert_eq!(sfx(&mut m, &two, None), 6, "class 0");
    assert_eq!(sfx(&mut m, &two, None), 4, "class 0");
    // (0,1) and (2,3) are class 2, (1,2) class 1: the lowest wins.
    assert_eq!(sfx(&mut m, &two, None), 1);
    // Channels 0 and 3 are free but apart: every run of two is held, so the
    // highest run is taken and the effect on it cut.
    assert_eq!(sfx(&mut m, &two, None), 6);
    assert_eq!(m.effects.len(), 3);
}

#[test]
fn a_run_of_a_reserved_channel_takes_its_role_into_account_only_as_class() {
    // An effect takes one channel per channel of its song, whatever the
    // roles: a song with a reserved channel still takes both.
    let mut m = Mixer::new();
    let two = long(2).reserved(1);
    assert_eq!(sfx(&mut m, &two, Some(2)), 2);
    assert_eq!(
        m.held(),
        [false, false, true, true, false, false, false, false]
    );
}

#[test]
fn a_named_channel_cuts_every_holder_of_the_run() {
    let mut m = Mixer::new();
    let a = sfx(&mut m, &long(2), Some(2));
    let b = sfx(&mut m, &long(1), Some(4));
    let c = m
        .play_sample(sample(&[255; 1000]), Some(6), 1 << 16)
        .unwrap();
    assert_eq!((a, b, c), (2, 4, 6));
    // Channels 3 and 4: the effect on 2..3 and the one on 4 are cut, the
    // whole effect, so channel 2 is free again; the sample stays.
    assert_eq!(sfx(&mut m, &long(2), Some(3)), 3);
    assert_eq!(
        m.held(),
        [false, false, false, true, true, false, true, false]
    );
    // A sample on a held channel cuts its holder.
    m.play_sample(sample(&[255; 10]), Some(4), 1 << 16).unwrap();
    assert_eq!(
        m.held(),
        [false, false, false, false, true, false, true, false]
    );
    assert_eq!(m.effects.len(), 0);
}

#[test]
fn channels_are_checked_and_a_run_must_fit() {
    let mut m = Mixer::new();
    let song = playable(&long(1));
    for bad in [-1, 8, 100] {
        let e = m.play_effect("sfx/t.omc", &song, Some(bad)).unwrap_err();
        assert_eq!(e.code(), "audio_bad_channel");
        assert_eq!(
            m.set_volume(bad, 1).unwrap_err().code(),
            "audio_bad_channel"
        );
        assert_eq!(
            m.play_sample(sample(&[1]), Some(bad), 1 << 16)
                .unwrap_err()
                .code(),
            "audio_bad_channel"
        );
    }
    let three = playable(&long(3));
    let e = m.play_effect("sfx/t.omc", &three, Some(6)).unwrap_err();
    assert_eq!(e.code(), "audio_no_room");
    assert_eq!(
        e.to_string(),
        "audio_no_room: sfx/t.omc has 3 channels, which do not fit from channel 6"
    );
    assert!(m.effects.is_empty(), "a refused effect holds nothing");
    assert_eq!(m.play_effect("sfx/t.omc", &three, Some(5)).unwrap(), 5);
}

#[test]
fn an_effect_frees_its_channels_the_frame_after_it_ends() {
    // Three ticks of one frame each: the arrangement ends as the fourth
    // tick would start, in the fourth frame, and the channel is free from
    // the fifth.
    let mut m = Mixer::new();
    sfx(&mut m, &Song::new(1).ticks(3), Some(2));
    let held: Vec<bool> = (0..6)
        .map(|_| {
            let was = m.held()[2];
            m.render();
            was
        })
        .collect();
    assert_eq!(held, [true, true, true, true, false, false]);
    // The channel it freed is the best run again.
    assert_eq!(sfx(&mut m, &long(1), None), 7);
}

#[test]
fn a_looping_effect_plays_until_it_is_cut() {
    let mut m = Mixer::new();
    sfx(&mut m, &Song::new(1).ticks(3).looping(), Some(1));
    frames(&mut m, 50);
    assert!(m.is_playing(1));
    assert!(m.held()[1]);
    sfx(&mut m, &long(1), Some(1));
    assert_eq!(m.effects.len(), 1);
}

#[test]
fn music_takes_channels_from_zero_and_effects_survive_it() {
    let mut m = Mixer::new();
    sfx(&mut m, &long(1), Some(0));
    sfx(&mut m, &long(1), Some(1));
    music(&mut m, &long(3).looping(), 0);
    frames(&mut m, 2);
    // The effects keep their channels; the music goes on under them.
    assert_eq!(m.effects.len(), 2);
    assert!(m.is_playing(2) && m.is_playing(0) && m.is_playing(1));
    assert!(!m.is_playing(3));
    // A new music cuts the old at once.
    music(&mut m, &long(1).looping(), 0);
    m.render();
    assert!(!m.is_playing(2));
    assert_eq!(m.effects.len(), 2);
}

#[test]
fn a_held_music_channel_goes_silent_and_comes_back() {
    let mut m = Mixer::new();
    // Channel 0 is hard left, channel 1 hard right.
    music(&mut m, &long(2).pan(0, -256).pan(1, 256).looping(), 0);
    let open = frame(&mut m);
    assert!(energy(&left(&open)) > 0 && energy(&right(&open)) > 0);
    // A silent effect holds channel 1 for four frames.
    sfx(&mut m, &Song::new(1).silent(0).ticks(3), Some(1));
    let mut ticks = Vec::new();
    for _ in 0..4 {
        let f = frame(&mut m);
        assert!(energy(&left(&f)) > 0, "the other channel goes on");
        assert_eq!(energy(&right(&f)), 0, "the held channel is silent");
        ticks.push(m.music.as_ref().unwrap().player.position().1);
    }
    // The music's clock went on while it was held.
    assert_eq!(ticks, [1, 2, 3, 4]);
    let back = frame(&mut m);
    assert!(
        energy(&right(&back)) > 0,
        "it sounds again when the holder ends"
    );
    assert_eq!(m.music.as_ref().unwrap().player.position().1, 5);
}

#[test]
fn a_reserved_channel_is_class_0_and_its_music_channels_are_not() {
    // The validator keeps a reserved channel free of notes, so the role
    // shows only in where effects go.
    let mut m = Mixer::new();
    music(&mut m, &long(2).reserved(1).looping(), 0);
    m.render();
    assert_eq!(sfx(&mut m, &long(1), None), 7);
    assert_eq!(sfx(&mut m, &long(1), None), 6);
    assert_eq!(sfx(&mut m, &long(1), None), 5);
    assert_eq!(sfx(&mut m, &long(1), None), 4);
    assert_eq!(sfx(&mut m, &long(1), None), 3);
    assert_eq!(sfx(&mut m, &long(1), None), 2);
    assert_eq!(sfx(&mut m, &long(1), None), 1, "reserved, class 0");
    assert_eq!(sfx(&mut m, &long(1), None), 0, "sounding music, class 2");
}

#[test]
fn music_loops_or_ends_like_an_effect() {
    let mut m = Mixer::new();
    music(&mut m, &Song::new(1).ticks(3).looping(), 0);
    frames(&mut m, 40);
    assert!(m.music_playing());
    music(&mut m, &Song::new(1).ticks(3), 0);
    for _ in 0..4 {
        assert!(m.music_playing());
        m.render();
    }
    assert!(!m.music_playing(), "ended in the fourth frame");
    assert!(m.render().iter().all(|&s| s == 0));
}

#[test]
fn music_fades_in_by_a_step_each_frame() {
    // The unfaded extremes of the song, then the same under each gain.
    let song = long(1).looping();
    let mut r = Mixer::new();
    music(&mut r, &song, 0);
    let (lo, hi) = extremes(&frame(&mut r));
    assert!(hi > 1000 && lo < -1000, "{lo} {hi}");
    let at = |g: i32| (((lo as i32 * g) >> 8) as i16, ((hi as i32 * g) >> 8) as i16);

    // Over 4 frames: 64, 128, 192, then 256 and it stays.
    let mut m = Mixer::new();
    music(&mut m, &song, 4);
    for g in [64, 128, 192, 256, 256] {
        assert_eq!(extremes(&frame(&mut m)), at(g), "gain {g}");
    }
    // A fade of 1 frame, or none, is unity at once.
    for fade in [0, 1] {
        let mut m = Mixer::new();
        music(&mut m, &song, fade);
        assert_eq!(extremes(&frame(&mut m)), at(256), "fade {fade}");
    }
    // A fade longer than 256 frames steps by 1.
    let mut m = Mixer::new();
    music(&mut m, &song, 1000);
    for g in [1, 2, 3] {
        assert_eq!(extremes(&frame(&mut m)), at(g), "gain {g}");
    }
    // 3 frames: 256 / 3 = 85.
    let mut m = Mixer::new();
    music(&mut m, &song, 3);
    for g in [85, 170, 255, 256] {
        assert_eq!(extremes(&frame(&mut m)), at(g), "gain {g}");
    }
}

#[test]
fn music_fades_out_and_is_cut_on_the_frame_it_reaches_zero() {
    let song = long(1).looping();
    let mut r = Mixer::new();
    music(&mut r, &song, 0);
    let (lo, hi) = extremes(&frame(&mut r));
    let at = |g: i32| (((lo as i32 * g) >> 8) as i16, ((hi as i32 * g) >> 8) as i16);

    let mut m = Mixer::new();
    music(&mut m, &song, 0);
    m.render();
    m.stop_music(4);
    for g in [192, 128, 64] {
        assert_eq!(extremes(&frame(&mut m)), at(g), "gain {g}");
        assert!(m.music_playing());
    }
    assert!(m.render().iter().all(|&s| s == 0), "cut as it reaches 0");
    assert!(!m.music_playing());

    // Out from where a fade-in had got to: 128, then 64, then 0.
    let mut m = Mixer::new();
    music(&mut m, &song, 4);
    frames(&mut m, 2);
    m.stop_music(4);
    assert_eq!(extremes(&frame(&mut m)), at(64));
    assert!(m.render().iter().all(|&s| s == 0));
    assert!(!m.music_playing());

    // 0 cuts now; so does a fade of 1 frame, on the next frame.
    let mut m = Mixer::new();
    music(&mut m, &song, 0);
    m.render();
    m.stop_music(0);
    assert!(!m.music_playing());
    music(&mut m, &song, 0);
    m.stop_music(1);
    assert!(m.render().iter().all(|&s| s == 0));
    // Nothing to stop is fine.
    m.stop_music(5);
    m.stop_music(0);
}

#[test]
fn a_new_music_cuts_the_old_without_a_tail() {
    let mut m = Mixer::new();
    music(&mut m, &long(2).looping(), 0);
    frames(&mut m, 3);
    let song = long(1).looping();
    music(&mut m, &song, 0);
    let mut fresh = Mixer::new();
    music(&mut fresh, &song, 0);
    assert_eq!(frame(&mut m), frame(&mut fresh));
}

#[test]
fn channel_gain_applies_to_song_channels_and_stays_through_new_sounds() {
    let song = long(1).looping();
    let mut r = Mixer::new();
    music(&mut r, &song, 0);
    let (lo, hi) = extremes(&frame(&mut r));
    let half = |x: i16| ((x as i32 * 128) >> 8) as i16;

    // On the music's channel, set before the music starts.
    let mut m = Mixer::new();
    m.set_volume(0, 128).unwrap();
    music(&mut m, &song, 0);
    assert_eq!(extremes(&frame(&mut m)), (half(lo), half(hi)));
    // On an effect's channel, and it stays through the next effect.
    let mut m = Mixer::new();
    m.set_volume(3, 128).unwrap();
    sfx(&mut m, &song, Some(3));
    assert_eq!(extremes(&frame(&mut m)), (half(lo), half(hi)));
    sfx(&mut m, &song, Some(3));
    assert_eq!(extremes(&frame(&mut m)), (half(lo), half(hi)));
    // A change applies to the next frame as a whole.
    m.set_volume(3, 0).unwrap();
    assert!(m.render().iter().all(|&s| s == 0));
    // Above 256 is 256.
    m.set_volume(3, 9999).unwrap();
    assert_eq!(extremes(&frame(&mut m)), (lo, hi));
}

#[test]
fn a_sample_voice_is_centred_and_scaled_by_its_channel_gain() {
    let mut m = Mixer::new();
    // 255 widens to 32512.
    m.play_sample(sample(&[255; 2000]), Some(3), 1 << 16)
        .unwrap();
    let f = frame(&mut m);
    assert_eq!(&f[..4], &[32512, 32512, 32512, 32512]);
    m.set_volume(3, 128).unwrap();
    let f = frame(&mut m);
    assert_eq!(&f[..2], &[16256, 16256]);
    assert!(m.held()[3]);
    m.set_volume(3, 0).unwrap();
    assert!(m.render().iter().all(|&s| s == 0));
    // The voice ends in the third frame, within its 2000 sample frames,
    // and its channel is free from the next.
    assert!(!m.held()[3]);
}

#[test]
fn pan_gives_different_left_and_right() {
    for (pan, quieter_left) in [(256, true), (-256, false), (0, false)] {
        let mut m = Mixer::new();
        music(&mut m, &long(1).pan(0, pan).looping(), 0);
        let f = frame(&mut m);
        let (l, r) = (energy(&left(&f)), energy(&right(&f)));
        if pan == 0 {
            assert_eq!(left(&f), right(&f));
        } else if quieter_left {
            assert!(l < r, "{l} {r}");
        } else {
            assert!(r < l, "{l} {r}");
        }
    }
}

#[test]
fn the_frame_sums_music_effects_and_samples_then_scales_and_clips() {
    let song = long(1).looping();
    let mut r = Mixer::new();
    sfx(&mut r, &song, Some(7));
    let one = frame(&mut r);
    // The music and an effect of the same song, a frame each.
    let mut m = Mixer::new();
    music(&mut m, &song, 0);
    sfx(&mut m, &song, Some(7));
    let two = frame(&mut m);
    assert!(one
        .iter()
        .zip(&two)
        .all(|(&a, &b)| 2 * a as i32 == b as i32));
    // And a sample voice on top: its centred value joins.
    let mut m = Mixer::new();
    music(&mut m, &song, 0);
    m.play_sample(sample(&[192; 1000]), Some(5), 1 << 16)
        .unwrap();
    let three = frame(&mut m);
    assert!(one
        .iter()
        .zip(&three)
        .all(|(&a, &b)| a as i32 + 16384 == b as i32));

    // Eight loud effects clip rather than wrap, at both ends.
    let loud = long(1);
    let mut m = Mixer::new();
    for c in 0..8 {
        sfx(&mut m, &loud, Some(c));
    }
    let clipped = frame(&mut m);
    assert!(clipped.contains(&32767) && clipped.contains(&-32768));
    // The master volume scales the sum before it clips, towards zero.
    let mut single = Mixer::new();
    sfx(&mut single, &loud, Some(0));
    single.render();
    let unit = frame(&mut single);
    m.set_master(10);
    assert_eq!(m.master(), 10);
    let tenth = frame(&mut m);
    for (&u, &t) in unit.iter().zip(&tenth) {
        assert_eq!(t as i32, (8 * u as i32 * 10 / 100).clamp(-32768, 32767));
    }
    m.set_master(250);
    assert_eq!(m.master(), 100);
}

#[test]
fn an_external_frame_replaces_one_mix_at_the_master_volume() {
    let mut m = Mixer::new();
    music(&mut m, &long(1).looping(), 0);
    let mut ext = vec![0i16; VALUES_PER_FRAME];
    ext[0] = 3;
    ext[1] = -3;
    ext[2] = 1000;
    m.set_master(50);
    m.set_external(&ext[..3]);
    let out = frame(&mut m);
    assert_eq!(
        &out[..4],
        &[1, -1, 500, 0],
        "towards zero, short input padded"
    );
    assert_eq!(out.len(), VALUES_PER_FRAME);
    // One frame only: the next mixes, and the music did not advance.
    assert!(energy(&frame(&mut m)) > 0);
    assert_eq!(m.music.as_ref().unwrap().player.position().1, 0);
}

#[test]
fn stop_all_silences_everything_and_keeps_the_assets_and_gains() {
    let mut m = Mixer::new();
    let song = playable(&long(1));
    m.play_music(&song, 0);
    m.play_effect("sfx/t.omc", &song, Some(2)).unwrap();
    m.play_sample(sample(&[255; 100]), Some(3), 1 << 16)
        .unwrap();
    m.set_volume(1, 7).unwrap();
    m.render();
    m.stop_all();
    assert!(m.render().iter().all(|&s| s == 0));
    assert!(!m.music_playing());
    assert!((0..CART_CHANNELS).all(|c| !m.is_playing(c)));
    assert_eq!(m.held(), [false; 8]);
    assert_eq!(m.gains[1], 7);
}

#[test]
fn a_subsong_selects_the_arrangement_that_plays() {
    // Arrangement i lasts 2 * (i + 1) ticks of one frame.
    let song = Song::new(1).ticks(2).arrangements(2, Some(1));
    let mut m = Mixer::new();
    sfx(&mut m, &song, Some(0));
    let held: Vec<bool> = (0..6)
        .map(|_| {
            m.render();
            m.held()[0]
        })
        .collect();
    assert_eq!(held, [true, true, true, true, false, false]);
}

#[test]
fn a_song_keeps_the_clock_of_its_own_tick() {
    // Six ticks of 1/50 s are 5292 samples: the arrangement ends inside the
    // eighth frame, where six ticks of 1/60 s end at the seventh's start.
    let mut m = Mixer::new();
    sfx(&mut m, &Song::new(1).tick(1, 50).ticks(6), Some(0));
    let mut held = Vec::new();
    for _ in 0..9 {
        held.push(m.held()[0]);
        m.render();
    }
    assert_eq!(
        held,
        [true, true, true, true, true, true, true, true, false]
    );
    let mut m = Mixer::new();
    sfx(&mut m, &Song::new(1).ticks(6), Some(0));
    frames(&mut m, 7);
    assert!(!m.held()[0]);
}

#[test]
fn a_sampler_instrument_plays_the_sample_of_its_song() {
    let mut m = Mixer::new();
    sfx(
        &mut m,
        &Song::new(1).sample(3000, 1).sampler().ticks(60),
        Some(2),
    );
    let f = frame(&mut m);
    assert!(energy(&f) > 0);
    assert_eq!(left(&f), right(&f), "a centred voice");
    assert!(m.is_playing(2));
}

#[test]
fn the_songs_own_volume_scales_its_output() {
    let mut a = Mixer::new();
    sfx(&mut a, &long(1), Some(0));
    let unit = frame(&mut a);
    let mut b = Mixer::new();
    sfx(&mut b, &long(1).volume(128), Some(0));
    let half = frame(&mut b);
    assert!(unit
        .iter()
        .zip(&half)
        .all(|(&u, &h)| (u as i32 * 128) >> 8 == h as i32));
}
