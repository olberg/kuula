//! The OMT project's canonical example through Kuula's mixer, and the
//! example carts' songs through the loader.

use std::path::PathBuf;

use kuula_core::audio::sample::SAMPLE_BUDGET;
use kuula_core::audio::songs::{self, SONG_BUDGET};
use kuula_core::audio::{Mixer, SAMPLES_PER_FRAME};
use omt_engine::sha256;
use omt_engine::song::{profile_violations, Role};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("omt")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn example(path: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join(path);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The hash and frame count the engine's own rendering of arrangement 0
/// gave, from the example's `.expected` file.
fn expected() -> (usize, String) {
    let text = String::from_utf8(fixture("canonical.expected")).unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("arrangement 0"));
    let frames = lines.next().unwrap().strip_prefix("frames ").unwrap();
    let hash = lines.next().unwrap().strip_prefix("sha256 ").unwrap();
    (frames.parse().unwrap(), hash.to_string())
}

#[test]
fn the_canonical_example_played_as_music_hashes_like_the_engines_rendering() {
    let (frames, hash) = expected();
    assert_eq!(frames, 105_840);
    assert_eq!(frames % SAMPLES_PER_FRAME, 0);
    let steps = frames / SAMPLES_PER_FRAME;
    assert_eq!(steps, 144);

    let loaded = songs::load(
        "music/canonical.omc",
        &fixture("canonical.omc"),
        SONG_BUDGET,
        SAMPLE_BUDGET,
    )
    .unwrap();
    let mut mixer = Mixer::new();
    mixer.set_master(100);
    let song = mixer.add_song("music/canonical.omc", &loaded);
    mixer.play_music(&song, 0);

    let mut pcm = Vec::with_capacity(frames * 2 * 2);
    for step in 0..steps {
        assert!(mixer.music_playing(), "still playing at frame {}", step + 1);
        for s in mixer.render() {
            pcm.extend_from_slice(&s.to_le_bytes());
        }
    }
    assert_eq!(pcm.len(), frames * 4, "16-bit stereo");
    assert_eq!(sha256(&pcm), hash);
    // The last voice ends with the 144th frame's last sample; the mixer
    // sees it end in the next frame, which is silence, and the music is over.
    assert!(mixer.render().iter().all(|&s| s == 0));
    assert!(!mixer.music_playing());
}

/// `examples/atomic`: a whole song on all eight channels, one of them a
/// sampled voice held as FLAC. It loads within the budgets a cart has, and
/// its first bar is what the song is written to open with: the voice at full
/// volume a little to the left, over the music.
#[test]
fn the_example_song_loads_within_the_budgets_and_opens_with_its_voice() {
    let path = "music/atomic.omc";
    let file = example("atomic/music/atomic.omc");
    let (checked, subsong) = omt_engine::load_omc(&file).unwrap();
    assert_eq!(subsong, 0);
    assert_eq!(checked.diags, []);

    let loaded = songs::load(path, &file, SONG_BUDGET, SAMPLE_BUDGET).unwrap();
    let song = loaded.playable.song.clone();
    assert_eq!(song.channels.len(), 8, "all of the console's channels");
    assert!(song.channels.iter().all(|c| c.role == Role::Music));
    assert!(
        loaded.song_bytes <= SONG_BUDGET / 8,
        "{}",
        loaded.song_bytes
    );
    assert_eq!(loaded.sample_bytes, 40_287 * 2);
    use omt_engine::song::{Encoding, Engine};
    assert_eq!(song.samples.len(), 1);
    let voice = &song.samples[0];
    assert_eq!(
        (voice.encoding, voice.rate, voice.channels, voice.frames),
        (Encoding::Flac, 22_000, 1, 40_287)
    );
    let samplers = song
        .instruments
        .iter()
        .flatten()
        .filter(|i| matches!(i.engine, Engine::Sampler(_)))
        .count();
    assert_eq!(samplers, 1, "only the voice is sampled");

    // The first bar, 1.74 s: the voice reaches full scale on the left and
    // is a tenth quieter on the right.
    let mut mixer = Mixer::new();
    let song = mixer.add_song(path, &loaded);
    mixer.play_music(&song, 0);
    let (mut left, mut right) = (0i32, 0i32);
    for _ in 0..104 {
        for pair in mixer.render().as_chunks::<2>().0 {
            left = left.max(pair[0].unsigned_abs() as i32);
            right = right.max(pair[1].unsigned_abs() as i32);
        }
    }
    assert!(left >= 32_767, "{left}");
    assert!((29_000..32_767).contains(&right), "{right}");
    assert!(mixer.music_playing());
}

#[test]
fn the_example_carts_songs_load_clean_and_cover_the_conformance_list() {
    for path in ["conformance/music/loop.omc", "conformance/sfx/blip.omc"] {
        let file = example(path);
        let (loaded, subsong) = omt_engine::load_omc(&file).unwrap();
        assert_eq!(subsong, 0);
        assert_eq!(loaded.diags, [], "{path}");
        let song = loaded.song.unwrap();
        assert_eq!(song.profile.as_deref(), Some("kuula"), "{path}");
        assert_eq!(profile_violations(&song, "kuula"), Some(vec![]), "{path}");
        songs::load(path, &file, SONG_BUDGET, SAMPLE_BUDGET).unwrap();
    }

    let (loaded, _) = omt_engine::load_omc(&example("conformance/music/loop.omc")).unwrap();
    let song = loaded.song.unwrap();
    let sounding = song
        .channels
        .iter()
        .filter(|c| c.role == Role::Music)
        .count();
    assert!(sounding >= 3);
    assert!(song.channels.iter().any(|c| c.pan != 0), "a panned channel");
    assert!(song.channels.iter().any(|c| c.role == Role::Reserved));
    assert_ne!(song.tick, (1, 60), "a tick that is not a frame");
    assert_eq!(song.tick, (1, 50));
    use omt_engine::song::Engine;
    let engines =
        |f: &dyn Fn(&Engine) -> bool| song.instruments.iter().flatten().any(|i| f(&i.engine));
    assert!(engines(&|e| matches!(e, Engine::Wave(_))));
    assert!(engines(&|e| matches!(e, Engine::Sampler(_))));
    assert_eq!(song.samples.len(), 1);
    assert_eq!(song.samples[0].channels, 1);
    assert!(
        song.samples[0].data.is_some(),
        "the sample is a resource of the file"
    );
    use omt_engine::song::Next;
    let arr = &song.arrangements[0];
    assert!(
        arr.orders.iter().enumerate().any(|(i, o)| match o.next {
            Next::Order(n) => n <= i,
            _ => false,
        }),
        "the arrangement loops"
    );

    let (blip, _) = omt_engine::load_omc(&example("conformance/sfx/blip.omc")).unwrap();
    assert_eq!(
        blip.song.unwrap().channels.len(),
        1,
        "the effect is one channel"
    );
}
