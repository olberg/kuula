//! The audio bindings through a real cart: names resolve through the
//! snapshot, errors carry their codes, the calls are priced, and what a
//! cart plays reaches the frame's stereo PCM (and so the conformance hash).

use kuula_core::audio::sample::encode_wav;
use kuula_core::audio::{OUTPUT_CHANNELS, SAMPLES_PER_FRAME, VALUES_PER_FRAME};
use kuula_core::{Console, FrameInput};
use omt_engine::omc::{write_bank, write_song, ResourceOut};

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

/// One channel, two versions that loop over rows of the same length: `low`
/// holds a note, `high` plays one an octave up.
fn versions() -> Vec<u8> {
    let json = r#"{"omt":"0.3","profile":"kuula","rate":44100,"tick":[1,60],
        "channels":[{}],
        "instruments":[{"number":1,"volume":16,"engine":{"kind":"wave","waveform":"pulse"}}],
        "tracks":[{"rows":4,"speed":2,"cells":[[0,"C-4 01"],[2,"C-4"]]},
                  {"rows":4,"speed":2,"cells":[[0,"C-5 01"],[1,"C-5"],[2,"C-5"],[3,"C-5"]]}],
        "arrangements":[{"name":"low","orders":[{"tracks":[0],"ticks":8,"next":0}]},
                        {"name":"high","orders":[{"tracks":[1],"ticks":8,"next":0}]}]}"#;
    write_song(json.as_bytes(), &[], None, "kuula tests")
}

/// A bank of cues over the pulse note the songs use: `hit` on one track,
/// with a range to vary its pitch in, `pair` on two, and `hum`, looping.
fn bank() -> Vec<u8> {
    let json = r#"{"omq":"0.2","rate":44100,"tick":[1,60],
        "instruments":[{"number":1,"volume":16,"engine":{"kind":"wave","waveform":"pulse"}}],
        "tracks":[{"rows":1,"speed":4,"cells":[[0,"C-5 01"]]}],
        "cues":[{"name":"hit","tracks":[0],"vary":{"transpose":[-256,256]}},
                {"name":"pair","tracks":[0,0]},
                {"name":"hum","tracks":[0],"loop":true}]}"#;
    write_bank(json.as_bytes(), &[], &[], None, "kuula tests")
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
            ("cues/fx.omc", &bank()),
            ("music/two.omc", &versions()),
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
fn cues_are_triggered_by_name_and_stopped_by_channel() {
    let mut c = console_with(
        "assert(cue('fx', 'hit') == 7)\n\
         assert(cue('fx', 'pair') == 5)\n\
         assert(cue('fx', 'hit', 12) == 4)\n\
         assert(cue('fx', 'hit', nil, 0.5) == 3)\n\
         assert(cue('fx', 'hit', nil, nil, 0) == 0)\n\
         h = cue('fx', 'hum')\n\
         assert(h == 2, h)\n\
         stop(h)\n\
         stop(0, true)\n\
         stop(1)\n\
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
    let (code, message) = fault("cue('fx', 'zap')");
    assert_eq!(code, "cue_not_found");
    assert!(
        message.contains("cues/fx.omc has no cue \"zap\""),
        "{message}"
    );
    assert_eq!(fault("cue('nope', 'hit')").0, "asset_not_found");
    assert_eq!(fault("cue('fx', 'hit', 0, 1, 8)").0, "audio_bad_channel");
    let (code, message) = fault("cue('fx', 'pair', nil, nil, 7)");
    assert_eq!(code, "audio_no_room");
    assert!(message.contains("cue \"pair\" has 2 channels"), "{message}");
    // A bank's name is an asset name, as a song's is.
    let (code, message) = fault("cue('../sfx/blip', 'hit')");
    assert_eq!(code, "asset_invalid", "{message}");
    assert_eq!(fault("stop(8)").0, "audio_bad_channel");

    // What the arguments do is heard: an octave up is another sound, half
    // the gain half the level, and a looping cue sounds until it is stopped.
    let frame = |body: &str, frames: usize| {
        let mut c = console_with(&format!("function _init() {body} end"));
        run(&mut c, frames);
        assert_eq!(c.state().fault(), None, "{:?}", c.state());
        (c.output().audio.to_vec(), peak(&c))
    };
    let written = frame("cue('fx', 'hit', 0)", 2);
    assert_ne!(frame("cue('fx', 'hit', 12)", 2).0, written.0);
    assert_eq!(frame("cue('fx', 'hit', 0, 1)", 2), written);
    let half = frame("cue('fx', 'hit', 0, 0.5)", 2).1;
    assert_eq!(
        half,
        ((written.1 * 128) >> 8).max(((-written.1 * 128) >> 8).abs())
    );
    // Varied by the console: not as written, and the same every run.
    let varied = frame("cue('fx', 'hit')", 2);
    assert_ne!(varied.0, written.0);
    assert_eq!(frame("cue('fx', 'hit')", 2), varied);
    assert!(frame("cue('fx', 'hum')", 30).1 > 0);
    assert_eq!(frame("stop(cue('fx', 'hum'))", 2).1, 0);
}

#[test]
fn music_names_a_version_and_switches_to_it_where_it_is() {
    let frames_of = |init: &str, at: usize, then: &str, frames: usize| {
        let mut c = console_with(&format!(
            "n = 0\n\
             function _init() {init} end\n\
             function _update() n = n + 1; if n == {at} then {then} end end"
        ));
        let pcm = hash_of(&mut c, frames);
        assert_eq!(c.state().fault(), None, "{:?}", c.state());
        pcm
    };
    let low = frames_of("music('two')", 0, "", 12);
    let high = frames_of("music('two', 0, 'high')", 0, "", 12);
    assert_ne!(low, high);
    assert_eq!(frames_of("music('two', 0, 'low')", 0, "", 12), low);
    assert_eq!(frames_of("music('two', 0, 1)", 0, "", 12), high);
    assert_eq!(frames_of("music('two', 0, 0.0)", 0, "", 12), low);

    // A switch in the fifth update is neither version from the top, and
    // naming the version that plays is no switch at all.
    let switched = frames_of("music('two')", 5, "music('two', 0, 'high')", 12);
    assert_ne!(switched, low);
    assert_ne!(switched, high);
    assert_eq!(
        frames_of("music('two')", 5, "music('two', 0, 'low')", 12),
        low
    );

    let fault = |src: &str| {
        let mut c = console_with(src);
        run(&mut c, 1);
        let f = c.state().fault().cloned().expect("faulted");
        (f.code, f.message)
    };
    let (code, message) = fault("music('two', 0, 'nope')");
    assert_eq!(code, "version_not_found");
    assert!(
        message.contains("music/two.omc has no version \"nope\""),
        "{message}"
    );
    assert_eq!(fault("music('two', 0, 2)").0, "version_not_found");
    let (code, message) = fault("music('two', 0, {})");
    assert_eq!(code, "runtime_error", "{message}");
    assert!(
        message.contains("version is a name or a number"),
        "{message}"
    );
}

#[test]
fn the_music_keeps_its_channels_until_the_cart_lets_some_go() {
    // The song has two channels. Six effects fill the other six; the
    // seventh cuts the oldest of them and leaves the music whole.
    let mut c = console_with(
        "music('song')\n\
         for i = 1, 6 do assert(sfx('blip') == 8 - i) end\n\
         assert(sfx('blip') == 7)\n\
         music_channels(1)\n\
         assert(sfx('blip') == 1)\n\
         assert(sfx('blip') == 6)\n\
         music_channels(0)\n\
         assert(sfx('blip') == 0)\n\
         music_channels(99)\n\
         music_channels(-1)\n\
         music_channels()\n\
         function _draw() end",
    );
    run(&mut c, 2);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());

    // A sound put on a channel by name is not cut by the ones that follow.
    let mut c = console_with(
        "sample('tick', 7)\n\
         for i = 1, 7 do assert(sfx('blip') == 7 - i) end\n\
         assert(sfx('blip') == 6)\n\
         function _draw() end",
    );
    run(&mut c, 2);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());

    // With one on every channel the music leaves, a sound that names none
    // is not played: the call returns nil, and the cart goes on.
    let mut c = console_with(
        "music('song')\n\
         for ch = 2, 7 do sfx('blip', ch) end\n\
         assert(sfx('blip') == nil)\n\
         assert(sample('tick') == nil)\n\
         assert(cue('fx', 'hit') == nil)\n\
         stop(7, true)\n\
         assert(cue('fx', 'pair') == nil)\n\
         assert(sfx('blip') == 7)\n\
         function _draw() end",
    );
    run(&mut c, 2);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
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
    // Seven audio calls: seven cycles of API on top of whatever the frame
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
    let with = measure(
        "sfx('blip', 0) music('song') sample('tick') volume(0, 1) cue('fx', 'hit') stop(7) \
         music_channels(4)",
    );
    assert_eq!(with - base, 7, "{base} -> {with}");
}
