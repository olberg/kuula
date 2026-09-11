//! Palette entries, transparency and the draw-time remap.

use super::{api, ink, with_gfx};
use crate::api::reg::Reg;
use crate::api::{Group, Price, Scope, Sig};
use crate::meter::charge;
use kuula_core::buf::to_int;
use kuula_core::Category;
use mlua::{Error, Result};

pub(super) fn install(reg: &mut Reg<'_>) -> Result<()> {
    reg.function(
        &PAL,
        |lua, (i, r, gg, b): (Option<f64>, Option<f64>, Option<f64>, Option<f64>)| {
            let Some(i) = i else {
                return api(lua, |ctx| ctx.state.pal_restore());
            };
            let index = to_int(i);
            let index = usize::try_from(index).unwrap_or(usize::MAX);
            let rgb = match (r, gg, b) {
                (Some(rgb), None, None) => {
                    let v = to_int(rgb);
                    [
                        ((v >> 16) & 0xff) as u8,
                        ((v >> 8) & 0xff) as u8,
                        (v & 0xff) as u8,
                    ]
                }
                (Some(r), Some(g), Some(b)) => {
                    let ch = |v: f64| to_int(v).clamp(0, 255) as u8;
                    [ch(r), ch(g), ch(b)]
                }
                _ => {
                    return Err(Error::runtime(
                        "pal takes (i, r, g, b), (i, rgb) or nothing",
                    ))
                }
            };
            charge(lua, Category::Api, 1)?;
            with_gfx(lua, |ctx| ctx.state.pal(index, rgb))
        },
    )?;

    reg.function(&PALT, |lua, (i, t): (Option<f64>, Option<bool>)| {
        api(lua, |ctx| match i {
            Some(i) => ctx.state.palt(ink(Some(i), 0), t.unwrap_or(true)),
            None => ctx.state.palt_clear(),
        })
    })?;

    reg.function(&PAL_MAP, |lua, (from, to): (Option<f64>, Option<f64>)| {
        api(lua, |ctx| match (from, to) {
            (Some(f), Some(t)) => ctx.state.pal_map(ink(Some(f), 0), ink(Some(t), 0)),
            _ => ctx.state.pal_map_clear(),
        })
    })?;

    reg.function(&PAL_RESET, |lua, ()| api(lua, |ctx| ctx.state.pal_reset()))?;
    Ok(())
}

binding!(PAL {
    name: "pal",
    scope: Scope::Global,
    group: Group::Palette,
    sigs: &[
        Sig::new(
            "pal(i, r, g, b)",
            "nothing; sets palette entry `i` (16 to 127)",
        ),
        Sig::new("pal(i, 0xRRGGBB)", "nothing; the same from one number"),
        Sig::new("pal()", "nothing; restores the default palette"),
    ],
    price: Price::One,
    errors: &["palette_index_out_of_range", "palette_index_locked"],
    doc: "`i >= 128` is out of range and `i < 16` is locked. Channels \
          clamp to 0 to 255; any other argument shape is a Lua error. \
          Transparency and remap are part of the draw state, not the \
          palette; `pal()` does not clear them.",
});

binding!(PALT {
    name: "palt",
    scope: Scope::Global,
    group: Group::Palette,
    sigs: &[
        Sig::new(
            "palt(i, [transparent])",
            "nothing; source colour `i` is skipped when blitting",
        ),
        Sig::new("palt()", "nothing; clears all transparency"),
    ],
    price: Price::One,
    defaults: &[("transparent", "true")],
});

binding!(PAL_MAP {
    name: "pal_map",
    scope: Scope::Global,
    group: Group::Palette,
    sigs: &[
        Sig::new(
            "pal_map(from, to)",
            "nothing; draws colour `from` as `to` (draw-time remap)",
        ),
        Sig::new("pal_map()", "nothing; clears the remap"),
    ],
    price: Price::One,
});

binding!(PAL_RESET {
    name: "pal_reset",
    scope: Scope::Global,
    group: Group::Palette,
    sigs: &[Sig::new(
        "pal_reset()",
        "nothing; clears transparency and remap",
    )],
    price: Price::One,
});
