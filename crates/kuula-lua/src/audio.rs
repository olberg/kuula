//! The audio globals: `sfx`, `music`, `sample` and `volume`. Each costs
//! one cycle; the mixer's own work is not charged.
//! Errors carry the core's `AudioError` code into the fault.

use crate::api::reg::Reg;
use crate::api::{Group, Price, Scope, Sig};
use crate::bindings::with_ctx;
use crate::meter::charge;
use crate::FrameCtx;
use kuula_core::audio::AudioError;
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

/// A volume 0..1 as 0..=255; out-of-range values clamp.
fn gain(v: f64) -> u8 {
    if v.is_nan() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A pitch multiplier as 16.16; out-of-range values clamp to 1/256..=16.
fn pitch(v: Option<f64>) -> u32 {
    let v = v.unwrap_or(1.0);
    if v.is_nan() {
        return 1 << 16;
    }
    (v.clamp(1.0 / 256.0, 16.0) * 65536.0).round() as u32
}

pub(crate) fn install(reg: &mut Reg<'_>) -> Result<()> {
    reg.function(&SFX, |lua, (name, ch): (String, Option<f64>)| {
        audio(lua, |ctx| ctx.state.sfx(&name, channel(ch))).map(|c| c as i64)
    })?;

    // `music(name, fade_frames?)`; `music()` or `music(nil, fade)` stops.
    reg.function(
        &MUSIC,
        |lua, (name, fade): (Option<String>, Option<f64>)| {
            let fade = fade.map(to_int).unwrap_or(0).clamp(0, u32::MAX as i64) as u32;
            audio(lua, |ctx| ctx.state.music(name.as_deref(), fade))
        },
    )?;

    reg.function(
        &SAMPLE,
        |lua, (name, ch, p): (String, Option<f64>, Option<f64>)| {
            audio(lua, |ctx| ctx.state.sample(&name, channel(ch), pitch(p))).map(|c| c as i64)
        },
    )?;

    reg.function(&VOLUME, |lua, (ch, v): (f64, f64)| {
        audio(lua, |ctx| ctx.state.volume(to_int(ch), gain(v)))?;
        Ok(Value::Nil)
    })?;
    Ok(())
}

const AUDIO_ERRORS: &[&str] = &[
    "asset_not_found",
    "asset_invalid",
    "track_error",
    "audio_bad_channel",
    "audio_no_room",
];

binding!(SFX {
    name: "sfx",
    scope: Scope::Global,
    group: Group::Audio,
    sigs: &[Sig::new("sfx(name, [channel])", "the channel it plays on",)],
    price: Price::One,
    defaults: &[("channel", "a free one")],
    errors: AUDIO_ERRORS,
    doc: "`name` is the stem of `sfx/<name>.trk`.",
});

binding!(MUSIC {
    name: "music",
    scope: Scope::Global,
    group: Group::Audio,
    sigs: &[
        Sig::new(
            "music(name, [fade])",
            "nothing; starts the track, fading over `fade` frames",
        ),
        Sig::new("music()", "nothing; stops the track"),
        Sig::new(
            "music(nil, fade)",
            "nothing; stops the track over `fade` frames"
        ),
    ],
    price: Price::One,
    defaults: &[("fade", "0")],
    errors: AUDIO_ERRORS,
    doc: "`name` is the stem of `music/<name>.trk`.",
});

binding!(SAMPLE {
    name: "sample",
    scope: Scope::Global,
    group: Group::Audio,
    sigs: &[Sig::new(
        "sample(name, [channel, pitch])",
        "the channel it plays on",
    )],
    price: Price::One,
    defaults: &[("channel", "a free one"), ("pitch", "1.0")],
    errors: &[
        "asset_not_found",
        "asset_invalid",
        "sample_error",
        "audio_bad_channel",
    ],
    doc: "`name` is the stem of `samples/<name>.wav`. `pitch` 1.0 is \
          native, clamped to 1/256 to 16.",
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
        assert_eq!(gain(1.0), 255);
        assert_eq!(gain(0.5), 128);
        assert_eq!(gain(-3.0), 0);
        assert_eq!(gain(f64::NAN), 0);
        assert_eq!(pitch(None), 65536);
        assert_eq!(pitch(Some(2.0)), 131072);
        assert_eq!(pitch(Some(0.0)), 256);
        assert_eq!(pitch(Some(1e9)), 16 << 16);
        assert_eq!(channel(Some(2.9)), Some(2));
        assert_eq!(channel(None), None);
    }
}
