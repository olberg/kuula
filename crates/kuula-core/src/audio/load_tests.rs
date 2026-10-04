//! Loading songs: the bounds before allocation, the validator and profile
//! refusals with their messages, and the two budgets.

use std::cell::Cell;
use std::rc::Rc;

use serde_json::{json, Value};

use super::sample::{encode_wav, SAMPLE_BUDGET};
use super::songs::{self, LoadedSong, SONG_BUDGET};
use super::testsong::{container, container_of, file, omc, Song};
use super::AudioError;
use crate::draw::DrawState;
use crate::snapshot::{Snapshot, SnapshotLimits};
use crate::source::{CartSource, SourceError};

fn load(bytes: &[u8]) -> Result<LoadedSong, AudioError> {
    songs::load("sfx/hit.omc", bytes, SONG_BUDGET, SAMPLE_BUDGET)
}

/// The message of a refused load, `code: path: why`.
fn refused(bytes: &[u8]) -> String {
    load(bytes).expect_err("refused").to_string()
}

fn bytes(json: &Value) -> Vec<u8> {
    serde_json::to_vec(json).unwrap()
}

fn state(entries: Vec<(&str, Vec<u8>)>) -> DrawState {
    let cart: Rc<dyn CartSource> =
        Rc::new(Snapshot::from_entries(entries, SnapshotLimits::default()).unwrap());
    DrawState::new(8, 8, cart)
}

/// A song chunk that inflates to `n` zero bytes, as the engine's writer
/// stores it.
fn bomb(n: usize) -> Vec<u8> {
    let f = file(&vec![0u8; n], &[]);
    let chunks = omt_engine::omc::chunks(&f).unwrap();
    assert_eq!(&chunks[1].kind, b"SONZ");
    chunks[1].data.unwrap().to_vec()
}

#[test]
fn a_song_loads_with_its_cost() {
    let song = Song::new(2).sample(1000, 1);
    let got = load(&omc(&song)).unwrap();
    assert_eq!(got.song_bytes, bytes(&song.json()).len());
    assert_eq!(got.sample_bytes, 2000);
    assert_eq!(got.playable.arrangement, 0);
    assert_eq!(got.playable.song.channels.len(), 2);
}

#[test]
fn a_file_that_is_not_a_song_is_a_song_error() {
    assert_eq!(
        refused(b"not a container"),
        "song_error: sfx/hit.omc: not an OMC file"
    );
    // A container of another version than the one read, 0.7.
    for (version, text) in [([1, 0, 0, 0], "1.0"), ([0, 0, 8, 0], "0.8")] {
        let mut f = omc(&Song::new(1));
        assert_eq!(f[4..8], [0, 0, 7, 0]);
        f[4..8].copy_from_slice(&version);
        assert_eq!(
            refused(&f),
            format!("song_error: sfx/hit.omc: unknown container version {text}")
        );
    }
    let no_default = br#"{"songs":[{"chunk":1,"format":"other"}]}"#;
    let f = container(no_default, &[(b"SONG", &bytes(&Song::new(1).json()))]);
    assert_eq!(
        refused(&f),
        "song_error: sfx/hit.omc: no Open Module Track song in the file"
    );
    // A song chunk whose CRC does not match.
    let mut f = omc(&Song::new(1));
    let manifest_len = u32::from_le_bytes(f[12..16].try_into().unwrap()) as usize;
    let song_data = 12 + 8 + manifest_len + (4 - manifest_len % 4) % 4 + 4 + 8;
    f[song_data] ^= 0xff;
    assert_eq!(
        refused(&f),
        "song_error: sfx/hit.omc: the song chunk is damaged"
    );
    // A subsong the song does not have.
    let f = omc(&Song::new(1).arrangements(2, Some(5)));
    assert_eq!(
        refused(&f),
        "song_error: sfx/hit.omc: subsong 5 does not exist: the song has 2 arrangements"
    );
    // One it has.
    assert_eq!(
        load(&omc(&Song::new(1).arrangements(2, Some(1))))
            .unwrap()
            .playable
            .arrangement,
        1
    );
}

#[test]
fn the_validators_first_error_is_quoted_with_its_path() {
    let mut json = Song::new(1).json();
    json["tracks"][0]["speed"] = json!(0);
    assert_eq!(
        refused(&file(&bytes(&json), &[])),
        "song_error: sfx/hit.omc: bad-value at tracks[0].speed"
    );
    let mut json = Song::new(1).json();
    json.as_object_mut().unwrap().remove("tick");
    assert_eq!(
        refused(&file(&bytes(&json), &[])),
        "song_error: sfx/hit.omc: missing-member at tick"
    );
    // A reserved channel with a track is the validator's, not the profile's.
    let mut json = Song::new(1).json();
    json["channels"][0]["role"] = json!("reserved");
    assert!(refused(&file(&bytes(&json), &[])).contains("reserved-channel"));
    // Not JSON at all: no path to quote.
    assert_eq!(
        refused(&file(b"{", &[])),
        "song_error: sfx/hit.omc: not-json"
    );
}

#[test]
fn the_kuula_profile_is_kept_whichever_profile_the_song_names() {
    let rule = |song: Song, tweak: &dyn Fn(&mut Value)| {
        let mut json = song.json();
        tweak(&mut json);
        let f = file(&bytes(&json), &song.resources());
        refused(&f)
    };
    let why = |rule: &str| format!("song_error: sfx/hit.omc: breaks the kuula profile: {rule}");
    for name in [None, Some("kuula"), Some("tracker")] {
        let stereo = Song::new(1).sample(100, 2).profile(name);
        assert_eq!(rule(stereo, &|_| {}), why("stereo-sample"), "{name:?}");
    }
    let song = || Song::new(1).profile(Some("tracker"));
    assert_eq!(rule(song(), &|j| j["rate"] = json!(48000)), why("rate"));
    assert_eq!(
        rule(Song::new(9).profile(Some("tracker")), &|_| {}),
        why("channels")
    );
    assert_eq!(
        rule(song(), &|j| j["resampling"] = json!("linear")),
        why("resampling")
    );
    // Pan is allowed: the console is stereo.
    let panned = Song::new(2).pan(0, -256).pan(1, 256);
    assert!(load(&omc(&panned)).is_ok());
}

#[test]
fn the_bounds_hold_before_anything_is_allocated() {
    let song = bytes(&Song::new(1).json());
    let manifest = br#"{"songs":[{"chunk":1}]}"#;

    // A manifest over 1 MiB, stored.
    let mut big = br#"{"songs":[{"chunk":1}],"pad":""#.to_vec();
    big.resize(1024 * 1024 + 1, b'x');
    big.extend_from_slice(b"\"}");
    let f = container(&big, &[(b"SONG", &song)]);
    assert_eq!(
        refused(&f),
        "song_error: sfx/hit.omc: the manifest is larger than 1 MiB"
    );

    // A compressed manifest that inflates past it.
    let f = container_of(b"JSNZ", &bomb(2 << 20), &[(b"SONG", &song)]);
    let e = load(&f).unwrap_err();
    assert_eq!(e.code(), "song_error");
    assert!(
        e.to_string().contains("the manifest doesn't inflate"),
        "{e}"
    );

    // A SONG chunk larger than the room the song budget has.
    let f = container(manifest, &[(b"SONG", &song)]);
    let e = songs::load("sfx/hit.omc", &f, song.len() - 1, SAMPLE_BUDGET).unwrap_err();
    assert_eq!(e.code(), "song_error");
    assert!(e.to_string().contains("SONG chunk 1"), "{e}");
    assert!(songs::load("sfx/hit.omc", &f, song.len(), SAMPLE_BUDGET).is_ok());

    // A SONZ chunk that inflates past it.
    let f = container(manifest, &[(b"SONZ", &bomb(4096))]);
    let e = songs::load("sfx/hit.omc", &f, 1000, SAMPLE_BUDGET).unwrap_err();
    assert_eq!(e.code(), "song_error");
    assert!(
        e.to_string()
            .contains("SONZ chunk 1 does not inflate within the 1000 bytes"),
        "{e}"
    );
    let f = container(manifest, &[(b"SONZ", &bomb(2 << 20))]);
    assert_eq!(load(&f).unwrap_err().code(), "song_error");

    // The rule is on every such chunk, not only the one that plays.
    let f = container(manifest, &[(b"SONG", &song), (b"SONZ", &bomb(2 << 20))]);
    let e = load(&f).unwrap_err();
    assert!(e.to_string().contains("SONZ chunk 2"), "{e}");

    // And on their sum: a chunk gets what the ones before it left, so
    // chunks that each fit are refused when together they do not, and a
    // load never inflates more than the room.
    let rest = SONG_BUDGET - song.len();
    let f = container(manifest, &[(b"SONG", &song), (b"SONZ", &bomb(rest))]);
    assert!(load(&f).is_ok());
    let f = container(manifest, &[(b"SONG", &song), (b"SONZ", &bomb(rest + 1))]);
    let e = load(&f).unwrap_err();
    assert_eq!(e.code(), "song_error");
    assert!(
        e.to_string().contains(&format!(
            "SONZ chunk 2 does not inflate within the {rest} bytes"
        )),
        "{e}"
    );
    let half = bomb(SONG_BUDGET / 2 + 1);
    let f = container(manifest, &[(b"SONZ", &half), (b"SONZ", &half)]);
    let e = load(&f).unwrap_err();
    assert!(e.to_string().contains("SONZ chunk 2"), "{e}");
    let f = container(manifest, &[(b"SONZ", &bomb(rest + 1)), (b"SONG", &song)]);
    let e = load(&f).unwrap_err();
    assert!(
        e.to_string().contains(&format!(
            "SONG chunk 2 is {} bytes; the song budget has {} left for it",
            song.len(),
            song.len() - 1
        )),
        "{e}"
    );
}

/// A cart source that counts the files read from it.
struct Counting {
    files: Snapshot,
    reads: Cell<usize>,
}

impl CartSource for Counting {
    fn read(&self, path: &str) -> Result<Vec<u8>, SourceError> {
        self.reads.set(self.reads.get() + 1);
        self.files.read(path)
    }
}

#[test]
fn a_refused_file_is_refused_for_good_and_read_once() {
    let source = Rc::new(Counting {
        files: Snapshot::from_entries(
            vec![
                ("music/bad.omc", b"not a container".to_vec()),
                ("samples/bad.wav", b"not a wav".to_vec()),
                ("sfx/ok.omc", omc(&Song::new(1))),
            ],
            SnapshotLimits::default(),
        )
        .unwrap(),
        reads: Cell::new(0),
    });
    let mut d = DrawState::new(8, 8, source.clone() as Rc<dyn CartSource>);

    let first = d.music(Some("bad"), 0).unwrap_err();
    assert_eq!(first.code(), "song_error");
    assert_eq!(source.reads.get(), 1);
    for _ in 0..3 {
        assert_eq!(d.music(Some("bad"), 0).unwrap_err(), first);
    }
    assert_eq!(source.reads.get(), 1, "the song is not read again");
    assert_eq!(d.audio.songs().used(), 0);

    let first = d.sample("bad", None, 1 << 16).unwrap_err();
    assert_eq!(first.code(), "sample_error");
    for _ in 0..3 {
        assert_eq!(d.sample("bad", None, 1 << 16).unwrap_err(), first);
    }
    assert_eq!(source.reads.get(), 2, "nor is the sample");

    // A name with no file keeps nothing, and what loads still loads.
    for _ in 0..2 {
        assert_eq!(
            d.music(Some("nope"), 0).unwrap_err().code(),
            "asset_not_found"
        );
    }
    assert_eq!(source.reads.get(), 4);
    d.sfx("ok", None).unwrap();

    // The reason that is kept is bounded, cut between characters, and it
    // is the one returned.
    let long = AudioError::Song {
        path: "music/x.omc".into(),
        why: format!("a{}", "é".repeat(200)),
    };
    let kept = d.audio.refuse("music/x.omc", long);
    let AudioError::Song { why, .. } = &kept else {
        unreachable!()
    };
    assert_eq!(why.len(), super::REFUSAL_REASON - 1);
    assert_eq!(d.audio.refused("music/x.omc"), Some(kept));
}

#[test]
fn sample_records_must_fit_the_budget_before_any_sample_is_decoded() {
    // 64 MiB declared against four bytes of resource: refused as a sample
    // budget error, not as a resource that does not match its record.
    let song = Song::new(1).sample(1 << 24, 2);
    let f = file(&bytes(&song.json()), &[vec![0; 4]]);
    let e = load(&f).unwrap_err();
    assert_eq!(e.code(), "sample_error");
    assert!(e.to_string().contains("sample budget exceeded"), "{e}");

    // The validator reads an integer by its value, so a size spelled as a
    // float is the same size: it must not slip past as "no size".
    let mut spelled = song.json();
    spelled["samples"][0]["frames"] = json!(1.6e7);
    spelled["samples"][0]["channels"] = json!(2.0);
    let text = bytes(&spelled);
    assert!(std::str::from_utf8(&text).unwrap().contains("16000000.0"));
    let e = load(&file(&text, &[vec![0; 4]])).unwrap_err();
    assert_eq!(e.code(), "sample_error", "{e}");
    assert!(e.to_string().contains("declare 64000000 bytes"), "{e}");

    // The sizes of the records add up against what is left.
    let two = Song::new(1).sample(100, 1).sample(100, 1);
    let f = omc(&two);
    assert!(songs::load("sfx/hit.omc", &f, SONG_BUDGET, 400).is_ok());
    let e = songs::load("sfx/hit.omc", &f, SONG_BUDGET, 399).unwrap_err();
    assert_eq!(e.code(), "sample_error");
}

/// `docs/songs.md`, the public page on song files, shows two whole songs.
/// They load as written, so the page cannot drift from what Kuula takes.
#[test]
fn the_songs_on_the_song_files_page_load() {
    let page = include_str!("../../../../docs/songs.md").replace("\r\n", "\n");
    let songs: Vec<&str> = page
        .split("```json\n")
        .skip(1)
        .map(|rest| rest.split("```").next().unwrap())
        .filter(|text| text.contains("\"omt\""))
        .collect();
    assert_eq!(songs.len(), 2, "the page has two example songs");

    // The effect: one channel, no samples.
    let blip = load(&file(songs[0].as_bytes(), &[])).unwrap();
    assert_eq!(blip.playable.song.channels.len(), 1);
    assert_eq!(blip.sample_bytes, 0);

    // The music: three channels, and the sample the page describes, 2205
    // frames of 16-bit mono.
    let beat = load(&file(songs[1].as_bytes(), &[vec![0; 2205 * 2]])).unwrap();
    assert_eq!(beat.playable.song.channels.len(), 3);
    assert_eq!(beat.sample_bytes, 2205 * 2);
}

#[test]
fn the_song_budget_counts_every_song_and_a_refusal_changes_nothing() {
    // Three songs of about 400 KB of payload: two fit in 1 MiB.
    let big = |n: &str| (format!("sfx/{n}.omc"), omc(&Song::new(1).padded(400_000)));
    let (a, b, c) = (big("a"), big("b"), big("c"));
    let mut d = state(vec![
        (a.0.as_str(), a.1),
        (b.0.as_str(), b.1),
        (c.0.as_str(), c.1),
        ("sfx/small.omc", omc(&Song::new(1))),
    ]);
    d.sfx("a", Some(0)).unwrap();
    d.sfx("b", Some(1)).unwrap();
    let used = d.audio.songs().used();
    assert!(used > 800_000 && used < SONG_BUDGET, "{used}");
    // Asking again is the cache, not a second charge.
    d.sfx("a", Some(2)).unwrap();
    assert_eq!(d.audio.songs().used(), used);

    let e = d.sfx("c", Some(3)).unwrap_err();
    assert_eq!(e.code(), "song_error");
    assert_eq!(d.audio.songs().used(), used);
    assert!(d.audio.cached_song("sfx/c.omc").is_none());
    assert_eq!(d.audio.samples().used(), 0);
    // Nothing started either.
    assert!(!d.audio.is_playing(3));
    // And what still fits loads.
    d.sfx("small", Some(4)).unwrap();
}

#[test]
fn the_sample_budget_is_shared_by_wav_files_and_songs() {
    let wav = |n: usize| encode_wav(44100, 8, 1, &vec![128; n]);
    let mut d = state(vec![
        ("music/a.omc", omc(&Song::new(1).sample(700_000, 1))),
        ("music/b.omc", omc(&Song::new(1).sample(400_000, 1))),
        ("samples/big.wav", wav(1 << 20)),
        ("samples/fits.wav", wav(600_000)),
    ]);
    d.music(Some("a"), 0).unwrap();
    assert_eq!(d.audio.samples().used(), 1_400_000);
    let song_used = d.audio.songs().used();

    // 1 MiB of WAV does not fit in what is left (697 152 bytes).
    let e = d.sample("big", None, 1 << 16).unwrap_err();
    assert_eq!(e.code(), "sample_error");
    assert_eq!(d.audio.samples().used(), 1_400_000);
    // Nor does a second song's 800 000 bytes of records.
    let e = d.music(Some("b"), 0).unwrap_err();
    assert_eq!(e.code(), "sample_error");
    assert_eq!(d.audio.samples().used(), 1_400_000);
    assert_eq!(d.audio.songs().used(), song_used);
    assert!(d.audio.cached_song("music/b.omc").is_none());
    // 600 000 bytes do.
    d.sample("fits", None, 1 << 16).unwrap();
    assert_eq!(d.audio.samples().used(), 2_000_000);

    // And the other way round: WAV files first leave a song less room.
    let mut d = state(vec![
        ("music/b.omc", omc(&Song::new(1).sample(400_000, 1))),
        ("samples/big.wav", wav(SAMPLE_BUDGET - 700_000)),
    ]);
    d.sample("big", None, 1 << 16).unwrap();
    let e = d.music(Some("b"), 0).unwrap_err();
    assert_eq!(e.code(), "sample_error");
    assert_eq!(d.audio.samples().used(), SAMPLE_BUDGET - 700_000);
    assert_eq!(d.audio.songs().used(), 0);
}
