//! The cart API as Lua globals. Every function reaches the frame's
//! [`FrameCtx`] through the state's app data and touches nothing else.
//! Nothing here calls back into Lua while the context is borrowed, so a
//! collector run can never re-enter a binding.
//!
//! Every binding charges the meter: at least one
//! cycle, drawing by pixels touched after clipping. The charge happens
//! after the context borrow is released and refuses once the meter is
//! dead, so a cart past its budget cannot draw or read input.

use crate::meter::{charge, with_meter};
use crate::{bufs, require, FrameCtx, Graveyard, LUA_HEAP_SOFT_CAP};
use kuula_core::buf::{to_int, Rect};
use kuula_core::meter::price;
use kuula_core::{Category, Colour, Fillp, GfxError};
use mlua::{Error, Lua, Result, Value, Variadic};

/// Seed for the cart's `math.random`. Lua seeds its generator from the
/// wall clock and a stack address at state creation, which a cart may
/// not observe; every cart starts from the same fixed seed instead.
pub const RANDOM_SEED: i64 = 0;

/// Colour used by `print` when none is given.
const DEFAULT_INK: u8 = 7;

/// Lua numbers arrive as `f64`; coordinates floor towards negative
/// infinity, and saturate rather than wrap.
pub(crate) fn coord(v: f64) -> i32 {
    to_int(v).clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// A colour argument: low 7 bits primary, bits 8 to 14 secondary.
pub(crate) fn colour(v: Option<f64>, default: u8) -> Colour {
    match v {
        Some(c) => Colour::from_i64(to_int(c)),
        None => Colour::solid(default),
    }
}

fn ink(v: Option<f64>, default: u8) -> u8 {
    colour(v, default).primary
}

/// Borrow the frame context for one binding.
pub(crate) fn with_ctx<R>(lua: &Lua, f: impl FnOnce(&mut FrameCtx) -> R) -> Result<R> {
    let mut ctx = lua
        .try_app_data_mut::<FrameCtx>()
        .map_err(|_| Error::runtime("cart API re-entered while a call is in progress"))?
        .ok_or_else(|| Error::runtime("cart API called outside a frame"))?;
    Ok(f(&mut ctx))
}

/// Like [`with_ctx`] for bindings that can fail with a graphics error,
/// which crosses into Lua as an external error carrying its code.
pub(crate) fn with_gfx<R>(
    lua: &Lua,
    f: impl FnOnce(&mut FrameCtx) -> std::result::Result<R, GfxError>,
) -> Result<R> {
    with_ctx(lua, f)?.map_err(Error::external)
}

/// A shape: run it, then charge the pixels it touched.
fn draw(lua: &Lua, f: impl FnOnce(&mut FrameCtx) -> u64) -> Result<()> {
    let touched = with_ctx(lua, f)?;
    charge(lua, Category::Draw, price::pixels(touched))
}

/// A sprite blit: the same, through the graphics error path.
fn draw_gfx(
    lua: &Lua,
    f: impl FnOnce(&mut FrameCtx) -> std::result::Result<u64, GfxError>,
) -> Result<()> {
    let touched = with_gfx(lua, f)?;
    charge(lua, Category::Draw, price::pixels(touched))
}

/// A state change or a read: one cycle.
fn api<R>(lua: &Lua, f: impl FnOnce(&mut FrameCtx) -> R) -> Result<R> {
    charge(lua, Category::Api, 1)?;
    with_ctx(lua, f)
}

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

pub fn install(lua: &Lua, graveyard: Graveyard) -> Result<()> {
    let g = lua.globals();

    // The sandbox: no files, no OS, no loading of further code except
    // through the cart-local `require`.
    for name in [
        "os",
        "io",
        "package",
        "debug",
        "require",
        "dofile",
        "loadfile",
        "load",
        "loadstring",
    ] {
        g.set(name, Value::Nil)?;
    }
    let string: mlua::Table = g.get("string")?;
    string.set("dump", Value::Nil)?;

    // Deterministic `math.random`, see `RANDOM_SEED`. A bare
    // `math.randomseed()` would reseed from the wall clock and a stack
    // address, so the cart's version falls back to the fixed seed instead.
    let math: mlua::Table = g.get("math")?;
    let randomseed: mlua::Function = math.get("randomseed")?;
    randomseed.call::<()>(RANDOM_SEED)?;
    math.set(
        "randomseed",
        lua.create_function(move |_, (a, b): (Option<Value>, Option<Value>)| {
            let a = a.unwrap_or(Value::Integer(RANDOM_SEED));
            randomseed.call::<mlua::MultiValue>((a, b))
        })?,
    )?;
    // One libm on every platform: the transcendental functions.
    crate::numeric::install(lua)?;

    g.set(
        "cls",
        lua.create_function(|lua, c: Option<f64>| {
            let n = with_ctx(lua, |ctx| ctx.state.cls(ink(c, 0)))?;
            charge(lua, Category::Draw, price::cls(n))
        })?,
    )?;

    g.set(
        "pset",
        lua.create_function(|lua, (x, y, c): (f64, f64, Option<f64>)| {
            draw(lua, |ctx| {
                ctx.state.pset(coord(x), coord(y), colour(c, DEFAULT_INK))
            })
        })?,
    )?;

    g.set(
        "pget",
        lua.create_function(|lua, (x, y): (f64, f64)| {
            api(lua, |ctx| ctx.state.pget(coord(x), coord(y)))
        })?,
    )?;

    g.set(
        "line",
        lua.create_function(
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
        )?,
    )?;

    g.set(
        "rect",
        lua.create_function(
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
        )?,
    )?;

    g.set(
        "rectfill",
        lua.create_function(
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
        )?,
    )?;

    g.set(
        "circ",
        lua.create_function(|lua, (x, y, r, c): (f64, f64, f64, Option<f64>)| {
            draw(lua, |ctx| {
                ctx.state
                    .circ(coord(x), coord(y), coord(r), colour(c, DEFAULT_INK))
            })
        })?,
    )?;

    g.set(
        "circfill",
        lua.create_function(|lua, (x, y, r, c): (f64, f64, f64, Option<f64>)| {
            draw(lua, |ctx| {
                ctx.state
                    .circfill(coord(x), coord(y), coord(r), colour(c, DEFAULT_INK))
            })
        })?,
    )?;

    // `print(text, x, y, colour)` draws with the system font and returns
    // the x after the last glyph. Any other call is the ordinary Lua
    // `print`: every argument is stringified, tab-joined and appended to
    // the frame log, which is what a cart author expects from
    // `print('x =', x)` while debugging.
    g.set(
        "print",
        lua.create_function(|lua, args: Variadic<Value>| {
            let is_num =
                |v: Option<&Value>| matches!(v, Some(Value::Integer(_) | Value::Number(_)));
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
        })?,
    )?;

    g.set(
        "clip",
        lua.create_function(
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
        )?,
    )?;

    g.set(
        "camera",
        lua.create_function(|lua, (x, y): (Option<f64>, Option<f64>)| {
            let (x, y) = (coord(x.unwrap_or(0.0)), coord(y.unwrap_or(0.0)));
            api(lua, |ctx| ctx.state.camera(x, y))
        })?,
    )?;

    g.set(
        "fillp",
        lua.create_function(|lua, (p, transparent): (Option<f64>, Option<bool>)| {
            let f = Fillp {
                pattern: (to_int(p.unwrap_or(0.0)) & 0xffff) as u16,
                transparent: transparent.unwrap_or(false),
            };
            api(lua, |ctx| ctx.state.fillp(f))
        })?,
    )?;

    // `pal(i, r, g, b)`, `pal(i, 0xRRGGBB)` or `pal()` to restore.
    g.set(
        "pal",
        lua.create_function(
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
        )?,
    )?;

    g.set(
        "palt",
        lua.create_function(|lua, (i, t): (Option<f64>, Option<bool>)| {
            api(lua, |ctx| match i {
                Some(i) => ctx.state.palt(ink(Some(i), 0), t.unwrap_or(true)),
                None => ctx.state.palt_clear(),
            })
        })?,
    )?;

    g.set(
        "pal_map",
        lua.create_function(|lua, (from, to): (Option<f64>, Option<f64>)| {
            api(lua, |ctx| match (from, to) {
                (Some(f), Some(t)) => ctx.state.pal_map(ink(Some(f), 0), ink(Some(t), 0)),
                _ => ctx.state.pal_map_clear(),
            })
        })?,
    )?;

    g.set(
        "pal_reset",
        lua.create_function(|lua, ()| api(lua, |ctx| ctx.state.pal_reset()))?,
    )?;

    g.set(
        "spr",
        lua.create_function(|lua, (n, x, y, w, h, fx, fy): SprArgs| {
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
        })?,
    )?;

    g.set(
        "sspr",
        lua.create_function(|lua, (sx, sy, sw, sh, dx, dy, dw, dh, fx, fy): SsprArgs| {
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
        })?,
    )?;

    g.set(
        "btn",
        lua.create_function(|lua, n: f64| {
            api(lua, |ctx| {
                let n = n.floor();
                (0.0..=255.0).contains(&n) && ctx.input.held(n as u8)
            })
        })?,
    )?;

    // `stat(name)`: the meter and the caps, as numbers (architecture
    // 7.1: the profiler is exposed to the cart).
    g.set(
        "stat",
        lua.create_function(|lua, name: String| {
            charge(lua, Category::Api, 1)?;
            let int = |v: u64| Value::Integer(v.min(i64::MAX as u64) as i64);
            let v = match name.as_str() {
                "cpu" => Value::Number(with_meter(lua, |m| {
                    m.total() as f64 / m.budget().max(1) as f64
                })?),
                "cpu_cycles" => int(with_meter(lua, |m| m.total())?),
                "cpu_budget" => int(with_meter(lua, |m| m.budget())?),
                "mem" => int(lua.used_memory() as u64),
                "mem_limit" => int(LUA_HEAP_SOFT_CAP as u64),
                "gfx_mem" => int(with_ctx(lua, |ctx| ctx.state.res.ledger().used() as u64)?),
                "gfx_limit" => int(with_ctx(lua, |ctx| ctx.state.res.ledger().cap() as u64)?),
                "frame" => int(with_ctx(lua, |ctx| ctx.frame)?),
                "width" => int(with_ctx(lua, |ctx| ctx.state.width() as u64)?),
                "height" => int(with_ctx(lua, |ctx| ctx.state.height() as u64)?),
                other => return Err(Error::runtime(format!("unknown stat {other:?}"))),
            };
            Ok(v)
        })?,
    )?;

    crate::codec::install(lua, graveyard.clone())?;
    bufs::install(lua, graveyard)?;
    require::install(lua)?;
    crate::audio::install(lua)?;
    Ok(())
}
