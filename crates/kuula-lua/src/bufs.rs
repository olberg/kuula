//! `buf` userdata and the globals that hand buffers out: `buf`,
//! `load_sheet`, `load_map`, `sheet`, `draw_target`, `map` and the
//! `screen` handle. A handle is only a `BufId`; the bytes live in the
//! core's slab. Handles are counted per id; when Lua collects the last
//! handle of a buffer its id goes to the graveyard.
//!
//! Prices: element access one cycle, memory moved or allocated by bytes,
//! decoding by decoded bytes, a cached load one cycle.

use crate::bindings::{coord, with_ctx, with_gfx};
use crate::meter::charge;
use crate::{FrameCtx, Graveyard};
use kuula_core::meter::price;
use kuula_core::{assets, BufId, BufKind, Category, GfxError};
use mlua::{Error, Lua, Result, UserData, UserDataMethods, UserDataRef, Value};

pub(crate) struct BufHandle {
    pub id: BufId,
    graveyard: Graveyard,
}

impl Drop for BufHandle {
    fn drop(&mut self) {
        if self.id != BufId::SCREEN {
            self.graveyard.borrow_mut().release(self.id);
        }
    }
}

/// Bytes of a live buffer, for pricing.
fn buf_bytes(ctx: &FrameCtx, id: BufId) -> std::result::Result<u64, GfxError> {
    Ok(ctx.state.res.get(id)?.bytes() as u64)
}

impl UserData for BufHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        // Integer kinds come back as Lua integers, `f32` as a float.
        m.add_method("get", |lua, this, (x, y): (f64, f64)| {
            let id = this.id;
            charge(lua, Category::Buf, 1)?;
            let (kind, v) = with_gfx(lua, |ctx| {
                let (kind, ..) = ctx.state.buf_info(id)?;
                Ok((kind, ctx.state.buf_get(id, coord(x), coord(y))?))
            })?;
            let v = v.unwrap_or(0.0);
            Ok(match kind {
                BufKind::F32 => Value::Number(v),
                _ => Value::Integer(v as i64),
            })
        });
        m.add_method("set", |lua, this, (x, y, v): (f64, f64, f64)| {
            let id = this.id;
            charge(lua, Category::Buf, 1)?;
            with_gfx(lua, |ctx| ctx.state.buf_set(id, coord(x), coord(y), v))
        });
        m.add_method("fill", |lua, this, v: f64| {
            let id = this.id;
            let bytes = with_gfx(lua, |ctx| {
                let b = buf_bytes(ctx, id)?;
                ctx.state.buf_fill(id, v)?;
                Ok(b)
            })?;
            charge(lua, Category::Buf, price::bytes128(bytes))
        });
        m.add_method("width", |lua, this, ()| {
            let id = this.id;
            charge(lua, Category::Api, 1)?;
            with_gfx(lua, |ctx| ctx.state.buf_info(id)).map(|(_, w, _, _)| w)
        });
        m.add_method("height", |lua, this, ()| {
            let id = this.id;
            charge(lua, Category::Api, 1)?;
            with_gfx(lua, |ctx| ctx.state.buf_info(id)).map(|(_, _, h, m)| match m {
                Some(m) => m.rows,
                None => h,
            })
        });
        m.add_method("kind", |lua, this, ()| {
            let id = this.id;
            charge(lua, Category::Api, 1)?;
            with_gfx(lua, |ctx| ctx.state.buf_info(id)).map(|(k, _, _, _)| k.name())
        });
        m.add_method("layers", |lua, this, ()| {
            let id = this.id;
            charge(lua, Category::Api, 1)?;
            with_gfx(lua, |ctx| ctx.state.buf_info(id))
                .map(|(_, _, _, m)| m.map(|m| m.layers).unwrap_or(1))
        });
        m.add_method("tile_size", |lua, this, ()| {
            let id = this.id;
            charge(lua, Category::Api, 1)?;
            with_gfx(lua, |ctx| ctx.state.buf_info(id))
                .map(|(_, _, _, m)| m.map(|m| m.tile_size).unwrap_or(0))
        });
        m.add_method("copy", |lua, this, args: CopyArgs| {
            copy(lua, this.id, args, false)
        });
        m.add_method("blit", |lua, this, args: CopyArgs| {
            copy(lua, this.id, args, true)
        });
        m.add_method("release", |lua, this, ()| {
            let id = this.id;
            charge(lua, Category::Api, 1)?;
            with_gfx(lua, |ctx| ctx.state.buf_release(id))
        });
        m.add_meta_method("__tostring", |lua, this, ()| {
            let id = this.id;
            Ok(match with_ctx(lua, |ctx| ctx.state.buf_info(id))? {
                Ok((k, w, h, _)) => format!("buf({} {}x{})", k.name(), w, h),
                Err(_) => "buf(released)".to_string(),
            })
        });
    }
}

type CopyArgs = (UserDataRef<BufHandle>, f64, f64, f64, f64, f64, f64);

fn copy(lua: &Lua, dst: BufId, args: CopyArgs, honour_clip: bool) -> Result<()> {
    let (src, sx, sy, w, h, dx, dy) = args;
    let src = src.id;
    let (w, h) = (coord(w), coord(h));
    let elements = (w.max(0) as u64).saturating_mul(h.max(0) as u64);
    let elem = with_gfx(lua, |ctx| {
        ctx.state.buf_copy(
            dst,
            src,
            coord(sx),
            coord(sy),
            w,
            h,
            coord(dx),
            coord(dy),
            honour_clip,
        )?;
        Ok(ctx.state.res.get(dst)?.kind().element_bytes() as u64)
    })?;
    charge(
        lua,
        Category::Buf,
        price::bytes128(elements.saturating_mul(elem)),
    )
}

pub(crate) fn handle(lua: &Lua, id: BufId, graveyard: &Graveyard) -> Result<Value> {
    if id != BufId::SCREEN {
        graveyard.borrow_mut().acquire(id);
    }
    let ud = lua.create_userdata(BufHandle {
        id,
        graveyard: graveyard.clone(),
    })?;
    Ok(Value::UserData(ud))
}

/// Run `load` and, if the budget is short, collect and free what Lua let
/// go of, then try once more.
fn load_with_retry(
    lua: &Lua,
    graveyard: &Graveyard,
    load: impl Fn(&mut FrameCtx) -> std::result::Result<BufId, GfxError>,
) -> Result<BufId> {
    match with_ctx(lua, &load)? {
        Ok(id) => Ok(id),
        Err(GfxError::BudgetExceeded { .. }) => {
            lua.gc_collect()?;
            let dead = graveyard.borrow_mut().drain_dead();
            with_gfx(lua, |ctx| {
                ctx.state.reap(dead);
                load(ctx)
            })
        }
        Err(e) => Err(Error::external(e)),
    }
}

/// `load_sheet` and `load_map`: one cycle when the name is already
/// live, decoded bytes / 8 otherwise.
fn load_asset(
    lua: &Lua,
    graveyard: &Graveyard,
    path: String,
    load: impl Fn(&mut FrameCtx) -> std::result::Result<BufId, GfxError>,
) -> Result<Value> {
    let cached = with_ctx(lua, |ctx| ctx.state.res.named(&path).is_some())?;
    charge(lua, Category::Asset, 1)?;
    let id = load_with_retry(lua, graveyard, load)?;
    if !cached {
        let bytes = with_gfx(lua, |ctx| buf_bytes(ctx, id))?;
        charge(lua, Category::Asset, price::bytes8(bytes))?;
    }
    handle(lua, id, graveyard)
}

pub fn install(lua: &Lua, graveyard: Graveyard) -> Result<()> {
    let g = lua.globals();

    g.set("screen", handle(lua, BufId::SCREEN, &graveyard)?)?;

    let gy = graveyard.clone();
    g.set(
        "buf",
        lua.create_function(move |lua, (kind, w, h): (String, f64, f64)| {
            let kind = BufKind::parse(&kind).ok_or_else(|| {
                Error::runtime(format!(
                    "buf kind must be u8, i16, i32 or f32, got {kind:?}"
                ))
            })?;
            let dim = |v: f64| u32::try_from(coord(v)).unwrap_or(u32::MAX);
            let (w, h) = (dim(w), dim(h));
            // Priced after the ledger accepted it, so a refused request
            // stays a recoverable graphics error rather than tripping
            // the meter on bytes that were never allocated.
            charge(lua, Category::Buf, 1)?;
            let id = load_with_retry(lua, &gy, |ctx| ctx.state.buf_alloc(kind, w, h))?;
            let bytes = with_gfx(lua, |ctx| buf_bytes(ctx, id))?;
            charge(lua, Category::Buf, price::bytes128(bytes))?;
            handle(lua, id, &gy)
        })?,
    )?;

    let gy = graveyard.clone();
    g.set(
        "load_sheet",
        lua.create_function(move |lua, name: String| {
            load_asset(lua, &gy, assets::sheet_path(&name), |ctx| {
                ctx.state.load_sheet(&name)
            })
        })?,
    )?;

    let gy = graveyard.clone();
    g.set(
        "load_map",
        lua.create_function(move |lua, name: String| {
            load_asset(lua, &gy, assets::map_path(&name), |ctx| {
                ctx.state.load_map(&name)
            })
        })?,
    )?;

    g.set(
        "sheet",
        lua.create_function(|lua, b: Option<UserDataRef<BufHandle>>| {
            let id = b.map(|b| b.id);
            charge(lua, Category::Api, 1)?;
            with_gfx(lua, |ctx| ctx.state.sheet(id))
        })?,
    )?;

    g.set(
        "draw_target",
        lua.create_function(|lua, b: Option<UserDataRef<BufHandle>>| {
            let id = b.map(|b| b.id);
            charge(lua, Category::Api, 1)?;
            with_gfx(lua, |ctx| ctx.state.draw_target(id))
        })?,
    )?;

    g.set(
        "map",
        lua.create_function(
            |lua,
             (m, cx, cy, sx, sy, cw, ch, layer): (
                UserDataRef<BufHandle>,
                f64,
                f64,
                f64,
                f64,
                f64,
                f64,
                Option<f64>,
            )| {
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
            },
        )?,
    )?;

    Ok(())
}
