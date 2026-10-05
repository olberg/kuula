//! The sheet, sprites and maps.

use super::{coord, draw_gfx, with_gfx};
use crate::api::reg::Reg;
use crate::api::{Group, Price, Scope, Sig};
use crate::bufs::BufHandle;
use crate::meter::charge;
use kuula_core::meter::price;
use kuula_core::tline::{to_fixed, Texture};
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

/// `tline(x0, y0, x1, y1, u, v, du, dv, [sx, sy, sw, sh, thick])`.
type TlineArgs = (
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
    f64,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
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

    reg.function(
        &TLINE,
        |lua, (x0, y0, x1, y1, u, v, du, dv, sx, sy, sw, sh, thick): TlineArgs| {
            draw_gfx(lua, |ctx| {
                ctx.state.tline(
                    (coord(x0), coord(y0), coord(x1), coord(y1)),
                    Texture {
                        u: to_fixed(u),
                        v: to_fixed(v),
                        du: to_fixed(du),
                        dv: to_fixed(dv),
                        x: coord(sx.unwrap_or(0.0)),
                        y: coord(sy.unwrap_or(0.0)),
                        w: sw.map_or(i32::MAX, coord),
                        h: sh.map_or(i32::MAX, coord),
                        thick: coord(thick.unwrap_or(1.0)),
                    },
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

binding!(TLINE {
    name: "tline",
    scope: Scope::Global,
    group: Group::Sprites,
    sigs: &[Sig::new(
        "tline(x0, y0, x1, y1, u, v, du, dv, [sx, sy, sw, sh, thick])",
        "nothing; a line of pixels sampling the sheet at `(u, v)` and \
         stepping `(du, dv)` texels a pixel",
    )],
    price: Price::Pixels,
    defaults: &[
        ("sx", "0"),
        ("sy", "0"),
        ("sw", "the sheet's width from `sx` on"),
        ("sh", "the sheet's height from `sy` on"),
        ("thick", "1"),
    ],
    errors: &["no_sheet"],
    doc: "Both ends are drawn, along the same line `line` would draw, and \
          the sheet is sampled the same way for a row, a column or a slant: \
          pixel `i` of the line, counting from the first end, reads texel \
          `(u + i * du, v + i * dv)`, rounded down. That is a perspective \
          floor drawn a row at a time, and a textured wall drawn a column \
          at a time with `dv` the texels a pixel of the column covers. \
          `sx, sy, sw, sh` name the rectangle of the sheet the texels are \
          in; the position wraps round its edges, whatever its size, so a \
          floor tile repeats and a column longer than its texture starts \
          it again. A rectangle not wholly in the sheet is cut to the \
          sheet, and `u, v` count from the corner of the cut one; a \
          rectangle with no sheet in it draws nothing. `u, v, du, dv` are \
          fixed point with sixteen fraction bits, so a step finer than \
          1/65536 of a texel is lost, and their magnitudes saturate at \
          2^32. Only the part of the line in the clip is walked and paid \
          for, with a camera offset and the colour table applied as for \
          `sspr` and no fill pattern. With `thick` above 1 each step of the \
          line draws that many pixels of its texel side by side: down from \
          a row, to the right of a column (a slanted line counts as a row \
          when it is at least as wide as it is tall). A pair of columns drawn as \
          one, or a floor drawn two rows at a time, is half the calls for \
          half the detail across. A line is paid like any draw call, by \
          the pixels it touches.",
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
