//! Shapes, text, clip, camera and the fill pattern.

use super::{api, colour, coord, draw, ink, with_ctx, DEFAULT_INK};
use crate::api::reg::Reg;
use crate::api::{Group, Price, Scope, Sig};
use crate::meter::charge;
use kuula_core::buf::{to_int, Rect};
use kuula_core::meter::price;
use kuula_core::{Category, Fillp};
use mlua::{Error, Result, Value, Variadic};

pub(super) fn install(reg: &mut Reg<'_>) -> Result<()> {
    reg.function(&CLS, |lua, c: Option<f64>| {
        let n = with_ctx(lua, |ctx| ctx.state.cls(ink(c, 0)))?;
        charge(lua, Category::Draw, price::cls(n))
    })?;

    reg.function(&PSET, |lua, (x, y, c): (f64, f64, Option<f64>)| {
        draw(lua, |ctx| {
            ctx.state.pset(coord(x), coord(y), colour(c, DEFAULT_INK))
        })
    })?;

    reg.function(&PGET, |lua, (x, y): (f64, f64)| {
        api(lua, |ctx| ctx.state.pget(coord(x), coord(y)))
    })?;

    reg.function(
        &LINE,
        |lua, (x0, y0, x1, y1, c): (f64, f64, f64, f64, Option<f64>)| {
            draw(lua, |ctx| {
                ctx.state.line(
                    coord(x0),
                    coord(y0),
                    coord(x1),
                    coord(y1),
                    colour(c, DEFAULT_INK),
                )
            })
        },
    )?;

    reg.function(
        &RECT,
        |lua, (x0, y0, x1, y1, c): (f64, f64, f64, f64, Option<f64>)| {
            draw(lua, |ctx| {
                ctx.state.rect(
                    coord(x0),
                    coord(y0),
                    coord(x1),
                    coord(y1),
                    colour(c, DEFAULT_INK),
                )
            })
        },
    )?;

    reg.function(
        &RECTFILL,
        |lua, (x0, y0, x1, y1, c): (f64, f64, f64, f64, Option<f64>)| {
            draw(lua, |ctx| {
                ctx.state.rectfill(
                    coord(x0),
                    coord(y0),
                    coord(x1),
                    coord(y1),
                    colour(c, DEFAULT_INK),
                )
            })
        },
    )?;

    reg.function(&CIRC, |lua, (x, y, r, c): (f64, f64, f64, Option<f64>)| {
        draw(lua, |ctx| {
            ctx.state
                .circ(coord(x), coord(y), coord(r), colour(c, DEFAULT_INK))
        })
    })?;

    reg.function(
        &CIRCFILL,
        |lua, (x, y, r, c): (f64, f64, f64, Option<f64>)| {
            draw(lua, |ctx| {
                ctx.state
                    .circfill(coord(x), coord(y), coord(r), colour(c, DEFAULT_INK))
            })
        },
    )?;

    // `print(text, x, y, colour)` draws with the system font and returns
    // the x after the last glyph. Any other call is the ordinary Lua
    // `print`: every argument is stringified, tab-joined and appended to
    // the frame log, which is what a cart author expects from
    // `print('x =', x)` while debugging.
    reg.function(&PRINT, |lua, args: Variadic<Value>| {
        let is_num = |v: Option<&Value>| matches!(v, Some(Value::Integer(_) | Value::Number(_)));
        if is_num(args.get(1)) && is_num(args.get(2)) {
            let text = args[0].to_string()?;
            let x: f64 = lua.unpack(args[1].clone())?;
            let y: f64 = lua.unpack(args[2].clone())?;
            let c: Option<f64> = lua.unpack(args.get(3).cloned().unwrap_or(Value::Nil))?;
            let chars = text.chars().count() as u64;
            let (end, touched) = with_ctx(lua, |ctx| {
                ctx.state
                    .print(&text, coord(x), coord(y), ink(c, DEFAULT_INK))
            })?;
            charge(lua, Category::Text, price::text(chars, touched))?;
            return Ok(Value::Integer(end as i64));
        }
        let parts = args
            .iter()
            .map(|v| v.to_string())
            .collect::<Result<Vec<_>>>()?;
        let line = parts.join("\t");
        charge(lua, Category::Api, price::bytes8(line.len() as u64))?;
        with_ctx(lua, |ctx| {
            ctx.state.log_line(line);
            Value::Nil
        })
    })?;

    reg.function(
        &CLIP,
        |lua, (x, y, w, h): (Option<f64>, Option<f64>, Option<f64>, Option<f64>)| {
            let rect = match (x, y, w, h) {
                (Some(x), Some(y), Some(w), Some(h)) => {
                    Some(Rect::new(coord(x), coord(y), coord(w), coord(h)))
                }
                (None, None, None, None) => None,
                _ => return Err(Error::runtime("clip takes x, y, w, h or nothing")),
            };
            api(lua, |ctx| ctx.state.clip(rect))
        },
    )?;

    reg.function(&CAMERA, |lua, (x, y): (Option<f64>, Option<f64>)| {
        let (x, y) = (coord(x.unwrap_or(0.0)), coord(y.unwrap_or(0.0)));
        api(lua, |ctx| ctx.state.camera(x, y))
    })?;

    reg.function(
        &FILLP,
        |lua, (p, transparent): (Option<f64>, Option<bool>)| {
            let f = Fillp {
                pattern: (to_int(p.unwrap_or(0.0)) & 0xffff) as u16,
                transparent: transparent.unwrap_or(false),
            };
            api(lua, |ctx| ctx.state.fillp(f))
        },
    )?;
    Ok(())
}

binding!(CLS {
    name: "cls",
    scope: Scope::Global,
    group: Group::Drawing,
    sigs: &[Sig::new("cls([c])", "nothing")],
    price: Price::Cls,
    defaults: &[("c", "0")],
    doc: "Fills the clip rectangle of the draw target.",
});

binding!(PSET {
    name: "pset",
    scope: Scope::Global,
    group: Group::Drawing,
    sigs: &[Sig::new("pset(x, y, [c])", "nothing")],
    price: Price::Pixels,
    defaults: &[("c", "7")],
});

binding!(PGET {
    name: "pget",
    scope: Scope::Global,
    group: Group::Drawing,
    sigs: &[Sig::new(
        "pget(x, y)",
        "colour index at `(x, y)` of the target in raw target coordinates \
         (no camera offset), 0 outside",
    )],
    price: Price::One,
});

binding!(LINE {
    name: "line",
    scope: Scope::Global,
    group: Group::Drawing,
    sigs: &[Sig::new("line(x0, y0, x1, y1, [c])", "nothing")],
    price: Price::Pixels,
    defaults: &[("c", "7")],
});

binding!(RECT {
    name: "rect",
    scope: Scope::Global,
    group: Group::Drawing,
    sigs: &[Sig::new(
        "rect(x0, y0, x1, y1, [c])",
        "nothing (outline, inclusive corners)",
    )],
    price: Price::Pixels,
    defaults: &[("c", "7")],
});

binding!(RECTFILL {
    name: "rectfill",
    scope: Scope::Global,
    group: Group::Drawing,
    sigs: &[Sig::new("rectfill(x0, y0, x1, y1, [c])", "nothing")],
    price: Price::Pixels,
    defaults: &[("c", "7")],
});

binding!(CIRC {
    name: "circ",
    scope: Scope::Global,
    group: Group::Drawing,
    sigs: &[Sig::new("circ(x, y, r, [c])", "nothing (outline)")],
    price: Price::Circle,
    defaults: &[("c", "7")],
    doc: "Circles cost by their radius even when mostly clipped; do not \
          draw huge circles off screen.",
});

binding!(CIRCFILL {
    name: "circfill",
    scope: Scope::Global,
    group: Group::Drawing,
    sigs: &[Sig::new("circfill(x, y, r, [c])", "nothing")],
    price: Price::Circle,
    defaults: &[("c", "7")],
    doc: "Priced like `circ`: by the radius even when mostly clipped.",
});

binding!(PRINT {
    name: "print",
    scope: Scope::Global,
    group: Group::Drawing,
    sigs: &[
        Sig::new("print(text, x, y, [c])", "x after the last glyph"),
        Sig::new(
            "print(...)",
            "nothing; stringifies the arguments, joins them with a tab and \
             appends the line to this frame's log",
        )
        .priced(Price::Bytes8("bytes"))
        .grouped(Group::Logging),
    ],
    price: Price::Text,
    defaults: &[("c", "7")],
    doc: "The drawing form needs numeric `x` and `y`. The system font is \
          4x6 pixels per glyph; text ignores `fillp`. Any other call shape, \
          such as `print(\"x =\", x)`, is the logging form.",
});

binding!(CLIP {
    name: "clip",
    scope: Scope::Global,
    group: Group::Drawing,
    sigs: &[
        Sig::new("clip(x, y, w, h)", "nothing"),
        Sig::new("clip()", "nothing; resets to the whole target"),
    ],
    price: Price::One,
    doc: "Any other number of arguments is a Lua error.",
});

binding!(CAMERA {
    name: "camera",
    scope: Scope::Global,
    group: Group::Drawing,
    sigs: &[Sig::new(
        "camera([x, y])",
        "nothing; `camera()` resets to `(0, 0)`",
    )],
    price: Price::One,
    defaults: &[("x", "0"), ("y", "0")],
});

binding!(FILLP {
    name: "fillp",
    scope: Scope::Global,
    group: Group::Drawing,
    sigs: &[Sig::new("fillp([pattern, transparent])", "nothing")],
    price: Price::One,
    defaults: &[("pattern", "0"), ("transparent", "false")],
    doc: "`pattern` is a 16-bit 4x4 dither; bit `(y % 4) * 4 + (x % 4)` set \
          selects the secondary colour, or skips the pixel when \
          `transparent` is true. `fillp()` clears it. Circles and lines \
          honour it; text does not.",
});
