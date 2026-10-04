//! The audio bindings through a real cart: names resolve through the
//! snapshot, errors carry their codes, the calls are priced, and what a
//! cart plays reaches the frame's stereo PCM (and so the conformance hash).

use kuula_core::audio::sample::encode_wav;
use kuula_core::audio::{OUTPUT_CHANNELS, SAMPLES_PER_FRAME, VALUES_PER_FRAME};
use kuula_core::{Console, FrameInput};
use omt_engine::omc::{write_song, ResourceOut};

use super::{cart, run};
use crate::LuaGuest;

/// A song for the `kuula` profile: `channels` is the JSON of its channels,
/// `orders` of its order row, with one pulse note held on each track.
fn song(channels: &str, orders: &str, extra: &str) -> Vec<u8> {
    let json = format!(
        r#"{{"omt":"0.3","profile":"kuula","rate":44100,"tick":[1,60],
            "channels":{channels},
            "instruments":[{{"number":1,"volume":16,"engine":{{"kind":"wave","waveform":"pulse"}}}}],
            "tracks":[{{"rows":1,"speed":4,"cells":[[0,"C-5 01"]]}}{extra}],
            "arrangements":[{{"orders":[{orders}]}}]}}"#
    );
    write_song(json.as_bytes(), &[], None, "kuula tests")
}

/// One channel, four ticks.
fn blip() -> Vec<u8> {
    song("[{}]", r#"{"tracks":[0],"ticks":4}"#, "")
}

/// Two channels, the second panned right, looping.
fn tune() -> Vec<u8> {
    song(
        r#"[{}, {"pan": 256}]"#,
        r#"{"tracks":[0,0],"ticks":4,"next":0}"#,
        "",
    )
}

/// Three channels, four ticks.
fn wide() -> Vec<u8> {
    song("[{}, {}, {}]", r#"{"tracks":[0,0,0],"ticks":4}"#, "")
}

/// A song with a stereo sample, which the `kuula` profile refuses.
fn stereo() -> Vec<u8> {
    let json = r#"{"omt":"0.3","rate":44100,"tick":[1,60],"channels":[{}],
        "instruments":[{"number":1,"engine":{"kind":"sampler","sample":0}}],
        "samples":[{"resource":0,"encoding":"pcm16","rate":44100,"channels":2,"frames":2}],
        "tracks":[{"rows":1,"speed":1,"cells":[[0,"C-4 01"]]}],
        "arrangements":[{"orders":[{"tracks":[0],"ticks":1}]}]}"#;
    let pcm = [0u8; 8];
    write_song(
        json.as_bytes(),
        &[ResourceOut {
            data: &pcm,
            media_type: None,
        }],
        None,
        "kuula tests",
    )
}

/// A song whose sample record declares 64 MiB against four bytes.
fn huge() -> Vec<u8> {
    let json = r#"{"omt":"0.3","profile":"kuula","rate":44100,"tick":[1,60],"channels":[{}],
        "instruments":[{"number":1,"engine":{"kind":"sampler","sample":0}}],
        "samples":[{"resource":0,"encoding":"pcm16","rate":44100,"channels":2,"frames":16777216}],
        "tracks":[{"rows":1,"speed":1,"cells":[[0,"C-4 01"]]}],
        "arrangements":[{"orders":[{"tracks":[0],"ticks":1}]}]}"#;
    let pcm = [0u8; 4];
    write_song(
        json.as_bytes(),
        &[ResourceOut {
            data: &pcm,
            media_type: None,
        }],
        None,
        "kuula tests",
    )
}

/// A song with a track of speed 0.
fn broken() -> Vec<u8> {
    song(
        "[{}]",
        r#"{"tracks":[1],"ticks":4}"#,
        r#",{"rows":1,"speed":0}"#,
    )
}

fn console_with(src: &str) -> Console {
    let tick = encode_wav(22050, 8, 1, &[255, 0, 255, 0, 255, 0, 255, 0]);
    let (blip, tune, wide, stereo, huge, broken) =
        (blip(), tune(), wide(), stereo(), huge(), broken());
    Console::new(
        cart(&[
            ("main.lua", src.as_bytes()),
            ("sfx/blip.omc", &blip),
            ("sfx/wide.omc", &wide),
            ("music/song.omc", &tune),
            ("samples/tick.wav", &tick),
            ("sfx/broken.omc", &broken),
            ("samples/stereo.wav", &encode_wav(44100, 16, 2, &[0; 8])),
            ("sfx/needs.omc", &stereo),
            ("sfx/huge.omc", &huge),
        ]),
        LuaGuest::factory,
    )
}

/// What the headless hasher covers, concatenated: pixels, palette and
/// audio of every frame.
fn hash_of(c: &mut Console, frames: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for _ in 0..frames {
        let out = c.step(FrameInput::NONE);
        bytes.extend_from_slice(out.screen);
        bytes.extend(out.palette.iter().flatten());
        bytes.extend(out.audio.iter().flat_map(|s| s.to_le_bytes()));
    }
    bytes
}

fn peak(c: &Console) -> i32 {
    c.output()
        .audio
        .iter()
        .map(|&s| (s as i32).abs())
        .max()
        .unwrap()
}

#[test]
fn a_silent_cart_renders_zeros_and_sfx_changes_the_hash() {
    let mut silent = console_with("function _draw() end");
    let mut loud = console_with("function _init() sfx('blip') end");
    let quiet = hash_of(&mut silent, 4);
    let out = silent.output();
    assert_eq!(out.audio.len(), VALUES_PER_FRAME);
    assert_eq!(out.audio.len(), SAMPLES_PER_FRAME * OUTPUT_CHANNELS);
    assert!(out.audio.iter().all(|&s| s == 0));
    // Neither cart draws, so only the audio can separate the two.
    let noisy = hash_of(&mut loud, 4);
    assert_ne!(quiet, noisy);
    assert_eq!(loud.state().fault(), None);
    // Deterministic: the same cart twice gives the same PCM.
    let mut again = console_with("function _init() sfx('blip') end");
    assert_eq!(hash_of(&mut again, 4), noisy);
}

#[test]
fn every_call_returns_and_the_errors_keep_their_codes() {
    let mut c = console_with(
        "ch = sfx('blip', 3)\n\
         assert(ch == 3, ch)\n\
         assert(sfx('blip') == 7)\n\
         assert(sfx('wide') == 4)\n\
         music('song', 5)\n\
         s = sample('tick', 2, 2.0)\n\
         assert(s == 2, s)\n\
         volume(0, 0.5)\n\
         music()\n\
         function _draw() end",
    );
    run(&mut c, 2);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());

    let fault = |src: &str| {
        let mut c = console_with(src);
        run(&mut c, 1);
        let f = c.state().fault().cloned().expect("faulted");
        (f.code, f.message)
    };
    assert_eq!(fault("sfx('nope')").0, "asset_not_found");
    assert_eq!(fault("sfx('blip', 8)").0, "audio_bad_channel");
    assert_eq!(fault("volume(-1, 1)").0, "audio_bad_channel");
    let (code, message) = fault("sfx('wide', 6)");
    assert_eq!(code, "audio_no_room");
    assert!(message.contains("3 channels"), "{message}");
    let (code, message) = fault("sfx('broken')");
    assert_eq!(code, "song_error");
    assert!(
        message.contains("sfx/broken.omc: bad-value at tracks[1].speed"),
        "{message}"
    );
    let (code, message) = fault("sfx('needs')");
    assert_eq!(code, "song_error");
    assert!(
        message.contains("sfx/needs.omc: breaks the kuula profile: stereo-sample"),
        "{message}"
    );
    let (code, message) = fault("sfx('huge')");
    assert_eq!(code, "sample_error");
    assert!(message.contains("sample budget exceeded"), "{message}");
    let (code, message) = fault("sample('stereo')");
    assert_eq!(code, "sample_error");
    assert!(message.contains("mono"), "{message}");
    assert_eq!(fault("music('../x')").0, "asset_invalid");
}

#[test]
fn a_cart_hears_stereo() {
    let mut c = console_with("function _init() music('song') end");
    run(&mut c, 2);
    let pcm = c.output().audio;
    let side = |first: usize| -> i64 {
        pcm.iter()
            .skip(first)
            .step_by(2)
            .map(|&s| (s as i64).abs())
            .sum()
    };
    // The second channel is panned right, so the right side is louder.
    let (l, r) = (side(0), side(1));
    assert!(l > 0 && r > l, "{l} {r}");
}

#[test]
fn volume_is_a_gain_of_256_steps() {
    let level = |body: &str| {
        let mut c = console_with(&format!("function _init() {body} sfx('blip', 0) end"));
        run(&mut c, 2);
        assert_eq!(c.state().fault(), None);
        peak(&c)
    };
    let plain = level("");
    let full = level("volume(0, 1.0)");
    let half = level("volume(0, 0.5)");
    let off = level("volume(0, 0)");
    let over = level("volume(0, 7)");
    assert_eq!(full, plain, "1.0 is exactly unity");
    assert_eq!(over, plain, "clamped");
    // The pulse swings both ways and `>>` rounds down, so the negative
    // peak is the larger by up to one.
    assert_eq!(half, ((plain * 128) >> 8).max(((-plain * 128) >> 8).abs()));
    assert_eq!(off, 0);
}

#[test]
fn a_fault_silences_the_cart() {
    let mut c = console_with(
        "music('song')\n\
         n = 0\n\
         function _update() n = n + 1; if n == 3 then error('bang') end end",
    );
    run(&mut c, 2);
    assert!(c.output().audio.iter().any(|&s| s != 0));
    run(&mut c, 2);
    assert_eq!(c.state().fault().unwrap().code, "runtime_error");
    assert!(c.output().audio.iter().all(|&s| s == 0));
    run(&mut c, 1);
    assert!(c.output().audio.iter().all(|&s| s == 0));
}

#[test]
fn the_calls_cost_one_cycle_each() {
    // Four audio calls: four cycles of API on top of whatever the frame
    // otherwise spends, measured against a control.
    let measure = |body: &str| {
        let mut c = console_with(&format!(
            "function _draw() {body} end\n\
             function _update() end"
        ));
        run(&mut c, 2);
        assert_eq!(c.state().fault(), None, "{:?}", c.state());
        c.output().profile.cycles[kuula_core::Category::Api as usize]
    };
    let base = measure("");
    let with = measure("sfx('blip', 0) music('song') sample('tick') volume(0, 1)");
    assert_eq!(with - base, 4, "{base} -> {with}");
}
