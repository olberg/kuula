//! The audio bindings through a real cart: names resolve through the
//! snapshot, errors carry their codes, the calls are priced, and what a
//! cart plays reaches the frame's PCM (and so the conformance hash).

use kuula_core::audio::sample::encode_wav;
use kuula_core::{Console, FrameInput};

use super::{cart, run};
use crate::LuaGuest;

const BLIP: &[u8] = b"tempo 2\ninst 1 pulse adsr=0,0,100,1\nC-4 1 vf\n===\n";
const SONG: &[u8] = b"tempo 1\nloop 0\ninst 1 saw\nC-3 1 | E-3 1\n";

fn console_with(src: &str) -> Console {
    let tick = encode_wav(22050, 8, 1, &[255, 0, 255, 0, 255, 0, 255, 0]);
    let src_owned = src.as_bytes();
    Console::new(
        cart(&[
            ("main.lua", src_owned),
            ("sfx/blip.trk", BLIP),
            ("music/song.trk", SONG),
            ("samples/tick.wav", &tick),
            ("sfx/broken.trk", b"inst 1 flute\nC-4 1\n"),
            ("samples/stereo.wav", &encode_wav(44100, 16, 2, &[0; 8])),
            ("sfx/needs.trk", b"inst 1 sample=stereo\nC-4 1\n"),
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

#[test]
fn a_silent_cart_renders_zeros_and_sfx_changes_the_hash() {
    let mut silent = console_with("function _draw() end");
    let mut loud = console_with("function _init() sfx('blip') end");
    let quiet = hash_of(&mut silent, 4);
    let out = silent.output();
    assert_eq!(out.audio.len(), kuula_core::audio::SAMPLES_PER_FRAME);
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
    let (code, message) = fault("sfx('broken')");
    assert_eq!(code, "track_error");
    assert!(message.contains("sfx/broken.trk:1"), "{message}");
    let (code, message) = fault("sample('stereo')");
    assert_eq!(code, "sample_error");
    assert!(message.contains("mono"), "{message}");
    assert_eq!(fault("sfx('needs')").0, "sample_error");
    assert_eq!(fault("music('../x')").0, "asset_invalid");
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
