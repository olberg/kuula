//! The cart API as Lua globals. Every function reaches the frame's
//! [`FrameCtx`] through the state's app data and touches nothing else.
//! Nothing here calls back into Lua while the context is borrowed, so a
//! collector run can never re-enter a binding.
//!
//! Every binding charges the meter: at least one
//! cycle, drawing by pixels touched after clipping. The charge happens
//! after the context borrow is released and refuses once the meter is
//! dead, so a cart past its budget cannot draw or read input.
//!
//! Each binding is registered through [`Reg`] under a descriptor
//! declared beside it (`binding!`), which is also what the reference in
//! `docs/api.md` is generated from.

mod draw;
pub(crate) mod net;
mod palette;
mod sprites;

use crate::api::reg::Reg;
use crate::api::{Group, Price, Scope, Sig};
use crate::meter::{charge, with_meter};
use crate::{bufs, require, FrameCtx, Graveyard, LUA_HEAP_SOFT_CAP};
use kuula_core::buf::to_int;
use kuula_core::meter::price;
use kuula_core::{Category, Colour, GfxError};
use mlua::{Error, Lua, Result, Value};

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

/// Install the sandbox and every cart binding, or in describe mode
/// collect their descriptors.
pub(crate) fn install(reg: &mut Reg<'_>, graveyard: Graveyard) -> Result<()> {
    // The sandbox: no files, no OS, no loading of further code except
    // through the cart-local `require`.
    reg.setup(|lua| {
        let g = lua.globals();
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
        // Deterministic `math.random`, see `RANDOM_SEED`.
        let math: mlua::Table = g.get("math")?;
        let randomseed: mlua::Function = math.get("randomseed")?;
        randomseed.call::<()>(RANDOM_SEED)
    })?;

    // A bare `math.randomseed()` would reseed from the wall clock and a
    // stack address, so the cart's version falls back to the fixed seed
    // instead.
    reg.custom(&RANDOMSEED, |lua, math| {
        let randomseed: mlua::Function = math.get("randomseed")?;
        math.set(
            "randomseed",
            lua.create_function(move |_, (a, b): (Option<Value>, Option<Value>)| {
                let a = a.unwrap_or(Value::Integer(RANDOM_SEED));
                randomseed.call::<mlua::MultiValue>((a, b))
            })?,
        )
    })?;
    // One libm on every platform: the transcendental functions.
    crate::numeric::install(reg)?;

    draw::install(reg)?;
    palette::install(reg)?;
    bufs::install(reg, graveyard.clone())?;
    sprites::install(reg)?;

    reg.function(&BTN, |lua, n: f64| {
        api(lua, |ctx| {
            let n = n.floor();
            (0.0..=255.0).contains(&n) && ctx.input.held(n as u8)
        })
    })?;

    // `stat(name)`: the meter and the caps, as numbers (architecture
    // 7.1: the profiler is exposed to the cart).
    reg.function(&STAT, |lua, name: String| {
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
            "net_sent" | "net_received" | "net_dropped" | "net_inbox" => {
                let v = with_ctx(lua, |ctx| {
                    ctx.state.net.as_ref().map(|n| match name.as_str() {
                        "net_sent" => n.sent,
                        "net_received" => n.received,
                        "net_dropped" => n.dropped,
                        _ => n.inbox.len() as u64,
                    })
                })?;
                match v {
                    Some(v) => int(v),
                    None => {
                        return Err(Error::runtime(format!(
                            "stat {name:?} needs the net service"
                        )))
                    }
                }
            }
            other => return Err(Error::runtime(format!("unknown stat {other:?}"))),
        };
        Ok(v)
    })?;

    crate::codec::install(reg, graveyard)?;
    require::install(reg)?;
    crate::audio::install(reg)?;
    Ok(())
}

binding!(RANDOMSEED {
    name: "randomseed",
    scope: Scope::Std(Some("math")),
    group: Group::Stdlib,
    sigs: &[Sig::new("math.randomseed([x, y])", "as Lua")],
    price: Price::Native,
    doc: "`math.random` is seeded to a fixed value every run, and \
          `math.randomseed()` with no argument reseeds to that same fixed \
          value instead of the clock. With arguments it seeds as Lua does.",
});

binding!(BTN {
    name: "btn",
    scope: Scope::Global,
    group: Group::Input,
    sigs: &[Sig::new(
        "btn(n)",
        "`true` while button `n` is held this frame; `false` for any other `n`",
    )],
    price: Price::One,
    doc: "There is no `btnp`; keep last frame's state yourself to detect \
          presses.",
});

binding!(STAT {
    name: "stat",
    scope: Scope::Global,
    group: Group::Logging,
    sigs: &[Sig::new("stat(name)", "a number, see below")],
    price: Price::One,
    doc: "Names: `\"cpu\"` (cycles used this frame divided by the budget, \
          a float that can pass 1 on frame 1), `\"cpu_cycles\"`, \
          `\"cpu_budget\"`, `\"mem\"` (Lua heap bytes), `\"mem_limit\"` \
          (16 MiB), `\"gfx_mem\"`, `\"gfx_limit\"` (8 MiB), `\"frame\"` \
          (the frame being run, 1-based), `\"width\"` and `\"height\"` \
          (the screen size the manifest chose). With the `net` service: \
          `\"net_sent\"`, `\"net_received\"`, `\"net_dropped\"` and \
          `\"net_inbox\"` (the counters and the unread events). Any other \
          name is a Lua error.",
});
