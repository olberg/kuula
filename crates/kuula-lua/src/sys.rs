//! The `sys` table: installed only in the shell guest.
//! Every function reads or writes the [`SysState`] in the shell's draw
//! state; a cart guest has no `SysState` and no `sys` global.

use kuula_core::shell::{SysRequest, SysState};
use kuula_core::Category;
use mlua::{Error, Lua, Result, Value};

use crate::api::reg::Reg;
use crate::api::{Group, Price, Scope, Sig};
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

pub(crate) fn install(reg: &mut Reg<'_>) -> Result<()> {
    reg.setup(|lua| lua.globals().set("sys", lua.create_table()?))?;

    reg.function(&CARTS, |lua, ()| {
        let carts = with_sys(lua, |s| s.carts.clone())?;
        let out = lua.create_table()?;
        for (i, c) in carts.iter().enumerate() {
            let t = lua.create_table()?;
            t.set("name", c.name.as_str())?;
            t.set("title", c.title.as_str())?;
            out.set(i + 1, t)?;
        }
        Ok(out)
    })?;
    reg.function(&RUN, |lua, name: String| {
        request(lua, SysRequest::Run(name))
    })?;
    reg.function(&QUIT, |lua, ()| request(lua, SysRequest::Quit))?;
    reg.function(&RESTART, |lua, ()| request(lua, SysRequest::Restart))?;
    reg.function(&PAUSED, |lua, on: bool| {
        request(lua, SysRequest::Paused(on))
    })?;
    reg.function(&IS_PAUSED, |lua, ()| with_sys(lua, |s| s.paused))?;
    reg.function(&RUNNING, |lua, ()| with_sys(lua, |s| s.running))?;
    reg.function(&MENU, |lua, ()| with_sys(lua, |s| s.menu_pressed))?;
    reg.function(&FAULT, |lua, ()| {
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
    })?;
    reg.function(&SETTINGS, |lua, ()| {
        let s = with_sys(lua, |s| s.settings)?;
        let t = lua.create_table()?;
        t.set("scale", s.scale)?;
        t.set("volume", s.volume)?;
        t.set("net", s.net)?;
        Ok(t)
    })?;
    reg.function(&SET_NET, |lua, on: bool| {
        request(lua, SysRequest::SetNet(on))
    })?;
    reg.function(&SET_SCALE, |lua, n: f64| {
        if !(1.0..=4.0).contains(&n) {
            return Err(Error::runtime("scale must be 1 to 4"));
        }
        request(lua, SysRequest::SetScale(n as u32))
    })?;
    reg.function(&SET_VOLUME, |lua, v: f64| {
        if !(0.0..=100.0).contains(&v) {
            return Err(Error::runtime("volume must be 0 to 100"));
        }
        request(lua, SysRequest::SetVolume(v as u32))
    })?;
    Ok(())
}

binding!(CARTS {
    name: "carts",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new(
        "sys.carts()",
        "a list of `{name, title}` tables, one per installed cart",
    )],
    price: Price::One,
});

binding!(RUN {
    name: "run",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new(
        "sys.run(name)",
        "nothing; asks the host to start the named cart",
    )],
    price: Price::One,
});

binding!(QUIT {
    name: "quit",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new(
        "sys.quit()",
        "nothing; asks the host to stop the running cart",
    )],
    price: Price::One,
});

binding!(RESTART {
    name: "restart",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new(
        "sys.restart()",
        "nothing; asks the host to restart the running cart",
    )],
    price: Price::One,
});

binding!(PAUSED {
    name: "paused",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new(
        "sys.paused(on)",
        "nothing; pauses or resumes the running cart",
    )],
    price: Price::One,
});

binding!(IS_PAUSED {
    name: "is_paused",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new("sys.is_paused()", "whether the cart is paused")],
    price: Price::One,
});

binding!(RUNNING {
    name: "running",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new("sys.running()", "whether a cart is running")],
    price: Price::One,
});

binding!(MENU {
    name: "menu",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new(
        "sys.menu()",
        "whether the menu button was pressed this frame",
    )],
    price: Price::One,
});

binding!(FAULT {
    name: "fault",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new(
        "sys.fault()",
        "the running cart's fault as `{code, file, line, message}`, or `nil`",
    )],
    price: Price::One,
});

binding!(SETTINGS {
    name: "settings",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new("sys.settings()", "`{scale, volume, net}`")],
    price: Price::One,
});

binding!(SET_NET {
    name: "set_net",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new(
        "sys.set_net(on)",
        "nothing; grants or withdraws networking for carts the shell runs",
    )],
    price: Price::One,
    doc: "Withdrawing it while a cart has a session ends the session; the \
          cart sees a `permission` event with `granted = false`.",
});

binding!(SET_SCALE {
    name: "set_scale",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new(
        "sys.set_scale(n)",
        "nothing; asks the host for window scale `n`, 1 to 4",
    )],
    price: Price::One,
    doc: "A scale outside 1 to 4 is a Lua error.",
});

binding!(SET_VOLUME {
    name: "set_volume",
    scope: Scope::Sys,
    group: Group::Shell,
    sigs: &[Sig::new(
        "sys.set_volume(v)",
        "nothing; asks the host for volume `v`, 0 to 100",
    )],
    price: Price::One,
    doc: "A volume outside 0 to 100 is a Lua error.",
});
