//! The `sys` table: installed only in the shell guest.
//! Every function reads or writes the [`SysState`] in the shell's draw
//! state; a cart guest has no `SysState` and no `sys` global.

use kuula_core::shell::{SysRequest, SysState};
use kuula_core::Category;
use mlua::{Error, Lua, Result, Table, Value};

use crate::bindings::with_ctx;
use crate::meter::charge;

fn with_sys<R>(lua: &Lua, f: impl FnOnce(&mut SysState) -> R) -> Result<R> {
    charge(lua, Category::Api, 1)?;
    with_ctx(lua, |ctx| {
        ctx.state
            .sys
            .as_mut()
            .map(f)
            .ok_or_else(|| Error::runtime("sys is only available to the shell"))
    })?
}

fn request(lua: &Lua, r: SysRequest) -> Result<()> {
    with_sys(lua, |s| s.request(r))
}

pub fn install(lua: &Lua) -> Result<()> {
    let sys = lua.create_table()?;

    sys.set(
        "carts",
        lua.create_function(|lua, ()| {
            let carts = with_sys(lua, |s| s.carts.clone())?;
            let out = lua.create_table()?;
            for (i, c) in carts.iter().enumerate() {
                let t = lua.create_table()?;
                t.set("name", c.name.as_str())?;
                t.set("title", c.title.as_str())?;
                out.set(i + 1, t)?;
            }
            Ok(out)
        })?,
    )?;
    sys.set(
        "run",
        lua.create_function(|lua, name: String| request(lua, SysRequest::Run(name)))?,
    )?;
    sys.set(
        "quit",
        lua.create_function(|lua, ()| request(lua, SysRequest::Quit))?,
    )?;
    sys.set(
        "restart",
        lua.create_function(|lua, ()| request(lua, SysRequest::Restart))?,
    )?;
    sys.set(
        "paused",
        lua.create_function(|lua, on: bool| request(lua, SysRequest::Paused(on)))?,
    )?;
    sys.set(
        "is_paused",
        lua.create_function(|lua, ()| with_sys(lua, |s| s.paused))?,
    )?;
    sys.set(
        "running",
        lua.create_function(|lua, ()| with_sys(lua, |s| s.running))?,
    )?;
    sys.set(
        "menu",
        lua.create_function(|lua, ()| with_sys(lua, |s| s.menu_pressed))?,
    )?;
    sys.set(
        "fault",
        lua.create_function(|lua, ()| {
            let fault = with_sys(lua, |s| s.fault.clone())?;
            match fault {
                None => Ok(Value::Nil),
                Some(f) => {
                    let t = lua.create_table()?;
                    t.set("code", f.code.as_str())?;
                    t.set("file", f.file.as_str())?;
                    match f.line {
                        Some(l) => t.set("line", l)?,
                        None => t.set("line", Value::Nil)?,
                    }
                    t.set("message", f.message.as_str())?;
                    Ok(Value::Table(t))
                }
            }
        })?,
    )?;
    sys.set(
        "settings",
        lua.create_function(|lua, ()| {
            let s = with_sys(lua, |s| s.settings)?;
            let t = lua.create_table()?;
            t.set("scale", s.scale)?;
            t.set("volume", s.volume)?;
            Ok(t)
        })?,
    )?;
    sys.set(
        "set_scale",
        lua.create_function(|lua, n: f64| {
            if !(1.0..=4.0).contains(&n) {
                return Err(Error::runtime("scale must be 1 to 4"));
            }
            request(lua, SysRequest::SetScale(n as u32))
        })?,
    )?;
    sys.set(
        "set_volume",
        lua.create_function(|lua, v: f64| {
            if !(0.0..=100.0).contains(&v) {
                return Err(Error::runtime("volume must be 0 to 100"));
            }
            request(lua, SysRequest::SetVolume(v as u32))
        })?,
    )?;

    let sys: Table = sys;
    lua.globals().set("sys", sys)?;
    Ok(())
}
