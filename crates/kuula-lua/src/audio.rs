//! The audio globals: `sfx`, `music`, `music_channels`, `sample`, `cue`,
//! `stop` and `volume`. Each costs one cycle; the mixer's own work is not
//! charged.
//! Errors carry the core's `AudioError` code into the fault.

use crate::api::reg::Reg;
use crate::api::{Group, Price, Scope, Sig};
use crate::bindings::with_ctx;
use crate::meter::charge;
use crate::FrameCtx;
use kuula_core::audio::{AudioError, Trigger, Version, GAIN_ONE};
use kuula_core::buf::to_int;
use kuula_core::Category;
use mlua::{Error, Lua, Result, Value};

/// One cycle, then the call, with an audio error crossing into Lua as an
/// external error carrying its code.
fn audio<R>(
    lua: &Lua,
    f: impl FnOnce(&mut FrameCtx) -> std::result::Result<R, AudioError>,
) -> Result<R> {
    charge(lua, Category::Api, 1)?;
    with_ctx(lua, f)?.map_err(Error::external)
}

fn channel(v: Option<f64>) -> Option<i64> {
    v.map(to_int)
}

/// What a call that plays a sound returns: the channel it took, or nil for
/// a sound that was not played.
fn played(channel: Option<usize>) -> Option<i64> {
    channel.map(|c| c as i64)
}

/// A volume 0..1 as 0..=256, where 256 is unity; out-of-range values
/// clamp.
fn gain(v: f64) -> u16 {
    if v.is_nan() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * GAIN_ONE as f64).round() as u16
}

/// A pitch multiplier as 16.16; out-of-range values clamp to 1/256..=16.
fn pitch(v: Option<f64>) -> u32 {
    let v = v.unwrap_or(1.0);
    if v.is_nan() {
        return 1 << 16;
    }
    (v.clamp(1.0 / 256.0, 16.0) * 65536.0).round() as u32
}

/// A transposition in semitones as pitch units, 256 to the semitone,
/// held to four octaves either way; nothing, or not a number, is 0.
fn pitch_units(v: Option<f64>) -> i32 {
    match v {
        Some(v) if !v.is_nan() => (v.clamp(-48.0, 48.0) * 256.0).round() as i32,
        _ => 0,
    }
}

/// The trigger a `cue` call gives: none when it names neither a
/// transposition nor a gain, and the console varies the cue.
fn trigger(transpose: Option<f64>, level: Option<f64>) -> Option<Trigger> {
    if transpose.is_none() && level.is_none() {
        return None;
    }
    Some(Trigger {
        transpose: pitch_units(transpose),
        gain: level.map_or(GAIN_ONE, gain) as i32,
    })
}

pub(crate) fn install(reg: &mut Reg<'_>) -> Result<()> {
    reg.function(&SFX, |lua, (name, ch): (String, Option<f64>)| {
        audio(lua, |ctx| ctx.state.sfx(&name, channel(ch))).map(played)
    })?;

    // `music(name, fade_frames?, version?)`; `music()` or `music(nil,
    // fade)` stops. A version is an arrangement's name or its number.
    reg.function(
        &MUSIC,
        |lua, (name, fade, version): (Option<String>, Option<f64>, Value)| {
            let fade = fade.map(to_int).unwrap_or(0).clamp(0, u32::MAX as i64) as u32;
            let named;
            let version = match &version {
                Value::Nil => None,
                Value::String(s) => {
                    named = s.to_str()?.to_string();
                    Some(Version::Name(&named))
                }
                Value::Integer(i) => Some(Version::Number(*i)),
                Value::Number(n) => Some(Version::Number(to_int(*n))),
                other => {
                    return Err(Error::runtime(format!(
                        "music: version is a name or a number, not a {}",
                        other.type_name()
                    )))
                }
            };
            audio(lua, |ctx| ctx.state.music(name.as_deref(), fade, version))
        },
    )?;

    reg.function(
        &SAMPLE,
        |lua, (name, ch, p): (String, Option<f64>, Option<f64>)| {
            audio(lua, |ctx| ctx.state.sample(&name, channel(ch), pitch(p))).map(played)
        },
    )?;

    reg.function(
        &CUE,
        |lua,
         (bank, name, transpose, level, ch): (
            String,
            String,
            Option<f64>,
            Option<f64>,
            Option<f64>,
        )| {
            let trigger = trigger(transpose, level);
            audio(lua, |ctx| ctx.state.cue(&bank, &name, trigger, channel(ch))).map(played)
        },
    )?;

    reg.function(&STOP, |lua, (ch, cut): (f64, Option<bool>)| {
        audio(lua, |ctx| ctx.state.stop(to_int(ch), cut.unwrap_or(false)))?;
        Ok(Value::Nil)
    })?;

    reg.function(&MUSIC_CHANNELS, |lua, n: Option<f64>| {
        audio(lua, |ctx| {
            ctx.state.music_channels(n.map(to_int));
            Ok(())
        })?;
        Ok(Value::Nil)
    })?;

    reg.function(&VOLUME, |lua, (ch, v): (f64, f64)| {
        audio(lua, |ctx| ctx.state.volume(to_int(ch), gain(v)))?;
        Ok(Value::Nil)
    })?;
    Ok(())
}

const SFX_ERRORS: &[&str] = &[
    "asset_not_found",
    "asset_invalid",
    "song_error",
    "sample_error",
    "audio_bad_channel",
    "audio_no_room",
];

const MUSIC_ERRORS: &[&str] = &[
    "asset_not_found",
    "asset_invalid",
    "song_error",
    "sample_error",
    "version_not_found",
];

binding!(SFX {
    name: "sfx",
    scope: Scope::Global,
    group: Group::Audio,
    sigs: &[Sig::new(
        "sfx(name, [channel])",
        "the channel it plays on, or nil when it is not played",
    )],
    price: Price::One,
    defaults: &[("channel", "a free one")],
    errors: SFX_ERRORS,
    doc: "`name` is the stem of `sfx/<name>.omc`.",
});

binding!(MUSIC {
    name: "music",
    scope: Scope::Global,
    group: Group::Audio,
    sigs: &[
        Sig::new(
            "music(name, [fade, version])",
            "nothing; starts the song, fading in over `fade` frames",
        ),
        Sig::new("music()", "nothing; stops the music"),
        Sig::new(
            "music(nil, fade)",
            "nothing; fades the music out over `fade` frames"
        ),
    ],
    price: Price::One,
    defaults: &[("fade", "0"), ("version", "the one the file selects")],
    errors: MUSIC_ERRORS,
    doc: "`name` is the stem of `music/<name>.omc`. `version` is the name or \
          the number of one of the song's arrangements. When the song is \
          the music already, it carries on in that version from the same \
          row and tick instead of starting again.",
});

binding!(MUSIC_CHANNELS {
    name: "music_channels",
    scope: Scope::Global,
    group: Group::Audio,
    sigs: &[Sig::new(
        "music_channels([n])",
        "nothing; the music keeps its first `n` channels to itself",
    )],
    price: Price::One,
    defaults: &[("n", "all of the music's channels")],
    doc: "A sound that names no channel is never put on a channel the \
          music keeps. `n` is 0 to 8, clamped. A cart starts with the music \
          keeping all of its channels; a lower `n` lets effects take its \
          upper channels when nothing else is free.",
});

binding!(SAMPLE {
    name: "sample",
    scope: Scope::Global,
    group: Group::Audio,
    sigs: &[Sig::new(
        "sample(name, [channel, pitch])",
        "the channel it plays on, or nil when it is not played",
    )],
    price: Price::One,
    defaults: &[("channel", "a free one"), ("pitch", "1.0")],
    errors: &[
        "asset_not_found",
        "asset_invalid",
        "sample_error",
        "audio_bad_channel",
        "audio_no_room",
    ],
    doc: "`name` is the stem of `samples/<name>.wav`. `pitch` 1.0 is \
          native, clamped to 1/256 to 16.",
});

binding!(CUE {
    name: "cue",
    scope: Scope::Global,
    group: Group::Audio,
    sigs: &[Sig::new(
        "cue(bank, name, [transpose, gain, channel])",
        "the first channel it plays on, or nil when it is not played",
    )],
    price: Price::One,
    defaults: &[
        ("transpose", "0 once `gain` is given"),
        ("gain", "1.0 once `transpose` is given"),
        ("channel", "a free run"),
    ],
    errors: &[
        "asset_not_found",
        "asset_invalid",
        "song_error",
        "sample_error",
        "cue_not_found",
        "audio_bad_channel",
        "audio_no_room",
    ],
    doc: "`bank` is the stem of `cues/<bank>.omc`, `name` a cue of that \
          bank. `transpose` is in semitones, clamped to 48 either way, and \
          `gain` 0.0 to 1.0. With neither, the console varies the cue \
          within the ranges its bank gives it.",
});

binding!(STOP {
    name: "stop",
    scope: Scope::Global,
    group: Group::Audio,
    sigs: &[Sig::new(
        "stop(channel, [cut])",
        "nothing; ends the cue, effect or sample holding the channel",
    )],
    price: Price::One,
    defaults: &[("cut", "false")],
    errors: &["audio_bad_channel"],
    doc: "A cue or an effect is released and its tail plays out; with \
          `cut` it is silent at once. The music is left alone.",
});

binding!(VOLUME {
    name: "volume",
    scope: Scope::Global,
    group: Group::Audio,
    sigs: &[Sig::new(
        "volume(channel, v)",
        "nothing; channel gain 0.0 to 1.0, clamped",
    )],
    price: Price::One,
    errors: &["audio_bad_channel"],
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_scaled_and_clamped() {
        assert_eq!(gain(1.0), 256);
        assert_eq!(gain(0.5), 128);
        assert_eq!(gain(0.3), 77);
        assert_eq!(gain(0.0), 0);
        assert_eq!(gain(-3.0), 0);
        assert_eq!(gain(f64::NAN), 0);
        assert_eq!(pitch(None), 65536);
        assert_eq!(pitch(Some(2.0)), 131072);
        assert_eq!(pitch(Some(0.0)), 256);
        assert_eq!(pitch(Some(1e9)), 16 << 16);
        assert_eq!(channel(Some(2.9)), Some(2));
        assert_eq!(channel(None), None);
        assert_eq!(pitch_units(Some(12.0)), 3072);
        assert_eq!(pitch_units(Some(-0.5)), -128);
        assert_eq!(pitch_units(Some(1e9)), 12288);
        assert_eq!(pitch_units(Some(f64::NAN)), 0);
        assert_eq!(trigger(None, None), None);
        assert_eq!(
            trigger(Some(1.0), None),
            Some(Trigger {
                transpose: 256,
                gain: 256
            })
        );
        assert_eq!(
            trigger(None, Some(0.5)),
            Some(Trigger {
                transpose: 0,
                gain: 128
            })
        );
    }
}
