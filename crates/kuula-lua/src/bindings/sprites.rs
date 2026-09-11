//! The sheet, sprites and maps.

use super::{coord, draw_gfx, with_gfx};
use crate::api::reg::Reg;
use crate::api::{Group, Price, Scope, Sig};
use crate::bufs::BufHandle;
use crate::meter::charge;
use kuula_core::meter::price;
use kuula_core::Category;
use mlua::{Result, UserDataRef};

/// `spr(n, x, y, [w, h, flip_x, flip_y])`.
type SprArgs = (
    f64,
    f64,
    f64,
    Option<f64>,
    Option<f64>,
    Option<bool>,
    Option<bool>,
);

/// `sspr(sx, sy, sw, sh, dx, dy, [dw, dh, flip_x, flip_y])`.
type SsprArgs = (
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
    Option<f64>,
    Option<f64>,
    Option<bool>,
    Option<bool>,
);

/// `map(m, cx, cy, sx, sy, cw, ch, [layer])`.
type MapArgs = (
    UserDataRef<BufHandle>,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
    Option<f64>,
);

pub(super) fn install(reg: &mut Reg<'_>) -> Result<()> {
    reg.function(&SHEET, |lua, b: Option<UserDataRef<BufHandle>>| {
        let id = b.map(|b| b.id);
        charge(lua, Category::Api, 1)?;
        with_gfx(lua, |ctx| ctx.state.sheet(id))
    })?;

    reg.function(&SPR, |lua, (n, x, y, w, h, fx, fy): SprArgs| {
        draw_gfx(lua, |ctx| {
            ctx.state.spr(
                coord(n),
                coord(x),
                coord(y),
                coord(w.unwrap_or(1.0)),
                coord(h.unwrap_or(1.0)),
                fx.unwrap_or(false),
                fy.unwrap_or(false),
            )
        })
    })?;

    reg.function(
        &SSPR,
        |lua, (sx, sy, sw, sh, dx, dy, dw, dh, fx, fy): SsprArgs| {
            draw_gfx(lua, |ctx| {
                ctx.state.sspr(
                    coord(sx),
                    coord(sy),
                    coord(sw),
                    coord(sh),
                    coord(dx),
                    coord(dy),
                    coord(dw.unwrap_or(sw)),
                    coord(dh.unwrap_or(sh)),
                    fx.unwrap_or(false),
                    fy.unwrap_or(false),
                )
            })
        },
    )?;

    reg.function(&MAP, |lua, (m, cx, cy, sx, sy, cw, ch, layer): MapArgs| {
        let id = m.id;
        let (cw, ch) = (coord(cw), coord(ch));
        let cells = (cw.max(0) as u64).saturating_mul(ch.max(0) as u64);
        let touched = with_gfx(lua, |ctx| {
            ctx.state.map(
                id,
                coord(cx),
                coord(cy),
                coord(sx),
                coord(sy),
                cw,
                ch,
                coord(layer.unwrap_or(0.0)),
            )
        })?;
        charge(lua, Category::Draw, price::map(cells, touched))
    })?;
    Ok(())
}

binding!(SHEET {
    name: "sheet",
    scope: Scope::Global,
    group: Group::Sprites,
    sigs: &[
        Sig::new(
            "sheet(b)",
            "nothing; selects the `u8` buffer `b` as the sheet"
        ),
        Sig::new("sheet()", "nothing; clears the sheet"),
    ],
    price: Price::One,
    errors: &["buf_kind_mismatch", "buf_released"],
});

binding!(SPR {
    name: "spr",
    scope: Scope::Global,
    group: Group::Sprites,
    sigs: &[Sig::new(
        "spr(n, x, y, [w, h, flip_x, flip_y])",
        "nothing; draws `w` by `h` cells from cell `n`",
    )],
    price: Price::Pixels,
    defaults: &[
        ("w", "1"),
        ("h", "1"),
        ("flip_x", "false"),
        ("flip_y", "false"),
    ],
    errors: &["no_sheet"],
    doc: "A cell below the sheet draws nothing.",
});

binding!(SSPR {
    name: "sspr",
    scope: Scope::Global,
    group: Group::Sprites,
    sigs: &[Sig::new(
        "sspr(sx, sy, sw, sh, dx, dy, [dw, dh, flip_x, flip_y])",
        "nothing; scaled copy of a sheet rectangle",
    )],
    price: Price::Pixels,
    defaults: &[
        ("dw", "sw"),
        ("dh", "sh"),
        ("flip_x", "false"),
        ("flip_y", "false"),
    ],
    errors: &["no_sheet"],
});

binding!(MAP {
    name: "map",
    scope: Scope::Global,
    group: Group::Sprites,
    sigs: &[Sig::new(
        "map(m, cx, cy, sx, sy, cw, ch, [layer])",
        "nothing; draws `cw` by `ch` cells of map `m` from cell `(cx, cy)` \
         with its top-left at `(sx, sy)`, through the sheet",
    )],
    price: Price::Map,
    defaults: &[("layer", "0")],
    errors: &["no_sheet", "not_a_map", "buf_released"],
    doc: "Map cells are sprite numbers into the current sheet; a negative \
          cell draws nothing. A layer outside the map's layers draws \
          nothing. `not_a_map` when `m` was not made by `load_map`.",
});
