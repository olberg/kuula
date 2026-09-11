//! `buf` userdata and the globals that hand buffers out: `buf`,
//! `load_sheet`, `load_map`, `draw_target` and the `screen` handle
//! (`sheet` and `map` are in `bindings::sprites`). A handle is only a
//! `BufId`; the bytes live in the core's slab. Handles are counted per
//! id; when Lua collects the last handle of a buffer its id goes to the
//! graveyard.
//!
//! Prices: element access one cycle, memory moved or allocated by bytes,
//! decoding by decoded bytes, a cached load one cycle.

use crate::api::reg::{LuaMethods, MethodSink, Reg};
use crate::api::{Group, Price, Scope, Sig};
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
        methods(&mut LuaMethods(m));
    }
}

/// Every `buf` method, into a live method table or the inventory.
pub(crate) fn methods<S: MethodSink<BufHandle>>(m: &mut S) {
    // Integer kinds come back as Lua integers, `f32` as a float.
    m.method(&GET, |lua, this, (x, y): (f64, f64)| {
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
    m.method(&SET, |lua, this, (x, y, v): (f64, f64, f64)| {
        let id = this.id;
        charge(lua, Category::Buf, 1)?;
        with_gfx(lua, |ctx| ctx.state.buf_set(id, coord(x), coord(y), v))
    });
    m.method(&FILL, |lua, this, v: f64| {
        let id = this.id;
        let bytes = with_gfx(lua, |ctx| {
            let b = buf_bytes(ctx, id)?;
            ctx.state.buf_fill(id, v)?;
            Ok(b)
        })?;
        charge(lua, Category::Buf, price::bytes128(bytes))
    });
    m.method(&WIDTH, |lua, this, ()| {
        let id = this.id;
        charge(lua, Category::Api, 1)?;
        with_gfx(lua, |ctx| ctx.state.buf_info(id)).map(|(_, w, _, _)| w)
    });
    m.method(&HEIGHT, |lua, this, ()| {
        let id = this.id;
        charge(lua, Category::Api, 1)?;
        with_gfx(lua, |ctx| ctx.state.buf_info(id)).map(|(_, _, h, m)| match m {
            Some(m) => m.rows,
            None => h,
        })
    });
    m.method(&KIND, |lua, this, ()| {
        let id = this.id;
        charge(lua, Category::Api, 1)?;
        with_gfx(lua, |ctx| ctx.state.buf_info(id)).map(|(k, _, _, _)| k.name())
    });
    m.method(&LAYERS, |lua, this, ()| {
        let id = this.id;
        charge(lua, Category::Api, 1)?;
        with_gfx(lua, |ctx| ctx.state.buf_info(id))
            .map(|(_, _, _, m)| m.map(|m| m.layers).unwrap_or(1))
    });
    m.method(&TILE_SIZE, |lua, this, ()| {
        let id = this.id;
        charge(lua, Category::Api, 1)?;
        with_gfx(lua, |ctx| ctx.state.buf_info(id))
            .map(|(_, _, _, m)| m.map(|m| m.tile_size).unwrap_or(0))
    });
    m.method(&COPY, |lua, this, args: CopyArgs| {
        copy(lua, this.id, args, false)
    });
    m.method(&BLIT, |lua, this, args: CopyArgs| {
        copy(lua, this.id, args, true)
    });
    m.method(&RELEASE, |lua, this, ()| {
        let id = this.id;
        charge(lua, Category::Api, 1)?;
        with_gfx(lua, |ctx| ctx.state.buf_release(id))
    });
    m.meta(&TOSTRING, |lua, this, ()| {
        let id = this.id;
        Ok(match with_ctx(lua, |ctx| ctx.state.buf_info(id))? {
            Ok((k, w, h, _)) => format!("buf({} {}x{})", k.name(), w, h),
            Err(_) => "buf(released)".to_string(),
        })
    });
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

pub(crate) fn install(reg: &mut Reg<'_>, graveyard: Graveyard) -> Result<()> {
    let gy = graveyard.clone();
    reg.value(&SCREEN, move |lua| handle(lua, BufId::SCREEN, &gy))?;

    let gy = graveyard.clone();
    reg.function(&BUF, move |lua, (kind, w, h): (String, f64, f64)| {
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
    })?;

    let gy = graveyard.clone();
    reg.function(&LOAD_SHEET, move |lua, name: String| {
        load_asset(lua, &gy, assets::sheet_path(&name), |ctx| {
            ctx.state.load_sheet(&name)
        })
    })?;

    let gy = graveyard.clone();
    reg.function(&LOAD_MAP, move |lua, name: String| {
        load_asset(lua, &gy, assets::map_path(&name), |ctx| {
            ctx.state.load_map(&name)
        })
    })?;

    reg.function(&DRAW_TARGET, |lua, b: Option<UserDataRef<BufHandle>>| {
        let id = b.map(|b| b.id);
        charge(lua, Category::Api, 1)?;
        with_gfx(lua, |ctx| ctx.state.draw_target(id))
    })?;

    Ok(())
}

binding!(SCREEN {
    name: "screen",
    scope: Scope::Global,
    group: Group::Buffers,
    sigs: &[Sig::new(
        "screen",
        "the screen buffer handle (a value, not a call)",
    )],
    price: Price::Value,
    doc: "Cannot be released.",
});

binding!(BUF {
    name: "buf",
    scope: Scope::Global,
    group: Group::Buffers,
    sigs: &[Sig::new(
        "buf(kind, w, h)",
        "a new zeroed buffer; `kind` is `\"u8\"`, `\"i16\"`, `\"i32\"` or \
         `\"f32\"`; `w, h` in 1..=4096",
    )],
    price: Price::Alloc,
    errors: &["buf_bad_dimensions", "graphics_budget_exceeded"],
    doc: "An unknown kind is a Lua error. Priced after the ledger accepted \
          the request, so a refused buffer costs nothing.",
});

binding!(LOAD_SHEET {
    name: "load_sheet",
    scope: Scope::Global,
    group: Group::Buffers,
    sigs: &[Sig::new(
        "load_sheet(name)",
        "the `u8` buffer of `gfx/<name>.png`, decoded once and cached by name",
    )],
    price: Price::Asset,
    errors: &[
        "asset_not_found",
        "asset_invalid",
        "asset_too_large",
        "graphics_budget_exceeded",
    ],
});

binding!(LOAD_MAP {
    name: "load_map",
    scope: Scope::Global,
    group: Group::Buffers,
    sigs: &[Sig::new(
        "load_map(name)",
        "the `i16` map buffer of `map/<name>.json`, decoded once and cached \
         by name",
    )],
    price: Price::Asset,
    errors: &[
        "asset_not_found",
        "asset_invalid",
        "asset_too_large",
        "graphics_budget_exceeded",
    ],
});

binding!(DRAW_TARGET {
    name: "draw_target",
    scope: Scope::Global,
    group: Group::Buffers,
    sigs: &[
        Sig::new(
            "draw_target(b)",
            "nothing; draws into `b` (a `u8` buffer) and resets the clip",
        ),
        Sig::new("draw_target()", "nothing; draws to the screen again"),
    ],
    price: Price::One,
    errors: &["buf_kind_mismatch", "buf_released"],
});

binding!(GET {
    name: "get",
    scope: Scope::Method,
    group: Group::BufMethods,
    sigs: &[Sig::new(
        "b:get(x, y)",
        "the element, 0 outside; an integer for integer kinds, a float for `f32`",
    )],
    price: Price::One,
    errors: &["buf_released"],
});

binding!(SET {
    name: "set",
    scope: Scope::Method,
    group: Group::BufMethods,
    sigs: &[Sig::new("b:set(x, y, v)", "nothing; ignored outside")],
    price: Price::One,
    errors: &["buf_released"],
});

binding!(FILL {
    name: "fill",
    scope: Scope::Method,
    group: Group::BufMethods,
    sigs: &[Sig::new("b:fill(v)", "nothing")],
    price: Price::Bytes128("bytes"),
    errors: &["buf_released"],
});

binding!(WIDTH {
    name: "width",
    scope: Scope::Method,
    group: Group::BufMethods,
    sigs: &[Sig::new("b:width()", "width in elements")],
    price: Price::One,
    errors: &["buf_released"],
});

binding!(HEIGHT {
    name: "height",
    scope: Scope::Method,
    group: Group::BufMethods,
    sigs: &[Sig::new(
        "b:height()",
        "height in elements; for a map, rows per layer",
    )],
    price: Price::One,
    errors: &["buf_released"],
});

binding!(KIND {
    name: "kind",
    scope: Scope::Method,
    group: Group::BufMethods,
    sigs: &[Sig::new(
        "b:kind()",
        "`\"u8\"`, `\"i16\"`, `\"i32\"` or `\"f32\"`",
    )],
    price: Price::One,
    errors: &["buf_released"],
});

binding!(LAYERS {
    name: "layers",
    scope: Scope::Method,
    group: Group::BufMethods,
    sigs: &[Sig::new("b:layers()", "map layers (1 for a plain buffer)")],
    price: Price::One,
    errors: &["buf_released"],
});

binding!(TILE_SIZE {
    name: "tile_size",
    scope: Scope::Method,
    group: Group::BufMethods,
    sigs: &[Sig::new("b:tile_size()", "8 or 16 for a map, 0 otherwise")],
    price: Price::One,
    errors: &["buf_released"],
});

binding!(COPY {
    name: "copy",
    scope: Scope::Method,
    group: Group::BufMethods,
    sigs: &[Sig::new(
        "b:copy(src, sx, sy, w, h, dx, dy)",
        "nothing; raw copy of same-kind elements, no clip, no colour table",
    )],
    price: Price::Bytes128("bytes"),
    errors: &["buf_aliased", "buf_kind_mismatch", "buf_released"],
});

binding!(BLIT {
    name: "blit",
    scope: Scope::Method,
    group: Group::BufMethods,
    sigs: &[Sig::new(
        "b:blit(src, sx, sy, w, h, dx, dy)",
        "nothing; like `copy` but honours the clip when `b` is the draw target",
    )],
    price: Price::Bytes128("bytes"),
    errors: &["buf_aliased", "buf_kind_mismatch", "buf_released"],
});

binding!(RELEASE {
    name: "release",
    scope: Scope::Method,
    group: Group::BufMethods,
    sigs: &[Sig::new("b:release()", "nothing; frees the buffer now")],
    price: Price::One,
    errors: &["buf_protected", "buf_released"],
});

binding!(TOSTRING {
    name: "__tostring",
    scope: Scope::Method,
    group: Group::BufMethods,
    sigs: &[Sig::new(
        "tostring(b)",
        "`buf(u8 64x64)` or `buf(released)`",
    )],
    price: Price::Value,
});
