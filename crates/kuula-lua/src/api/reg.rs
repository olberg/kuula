//! The registration helper: one call registers a binding under its
//! descriptor's name and records the descriptor. In describe mode there
//! is no Lua state, nothing is registered and no closure runs, so the
//! same install code yields the inventory offline.

use mlua::{FromLuaMulti, IntoLuaMulti, Lua, MaybeSend, Result, Table, UserDataMethods, Value};

use super::{Binding, Scope};

pub(crate) struct Reg<'a> {
    lua: Option<&'a Lua>,
    /// Descriptors in registration order.
    pub(crate) found: Vec<&'static Binding>,
}

impl<'a> Reg<'a> {
    /// Register into `lua`.
    pub(crate) fn lua(lua: &'a Lua) -> Reg<'a> {
        Reg {
            lua: Some(lua),
            found: Vec::new(),
        }
    }

    /// Collect descriptors only.
    pub(crate) fn describe() -> Reg<'static> {
        Reg {
            lua: None,
            found: Vec::new(),
        }
    }

    /// Setup that registers nothing a cart calls by a described name
    /// (removing globals, seeding, hooks). Skipped in describe mode.
    pub(crate) fn setup(&mut self, f: impl FnOnce(&Lua) -> Result<()>) -> Result<()> {
        match self.lua {
            Some(lua) => f(lua),
            None => Ok(()),
        }
    }

    /// The table a scope's entries go in.
    fn table(lua: &Lua, scope: Scope) -> Result<Table> {
        let g = lua.globals();
        match scope {
            Scope::Global | Scope::Std(None) => Ok(g),
            Scope::Sys => g.get("sys"),
            Scope::Net => g.get("net"),
            Scope::Std(Some(name)) => g.get(name),
            Scope::Method => Err(mlua::Error::runtime(
                "methods are registered through MethodSink",
            )),
        }
    }

    /// Register `f` as the function `b` describes.
    pub(crate) fn function<F, A, R>(&mut self, b: &'static Binding, f: F) -> Result<()>
    where
        F: Fn(&Lua, A) -> Result<R> + MaybeSend + 'static,
        A: FromLuaMulti,
        R: IntoLuaMulti,
    {
        self.found.push(b);
        if let Some(lua) = self.lua {
            Reg::table(lua, b.scope)?.set(b.name, lua.create_function(f)?)?;
        }
        Ok(())
    }

    /// Register the value `make` builds as the entry `b` describes.
    pub(crate) fn value(
        &mut self,
        b: &'static Binding,
        make: impl FnOnce(&Lua) -> Result<Value>,
    ) -> Result<()> {
        self.found.push(b);
        if let Some(lua) = self.lua {
            Reg::table(lua, b.scope)?.set(b.name, make(lua)?)?;
        }
        Ok(())
    }

    /// A described entry installed by hand into its scope's table,
    /// for wrappers that need the native they replace.
    pub(crate) fn custom(
        &mut self,
        b: &'static Binding,
        install: impl FnOnce(&Lua, &Table) -> Result<()>,
    ) -> Result<()> {
        self.found.push(b);
        if let Some(lua) = self.lua {
            install(lua, &Reg::table(lua, b.scope)?)?;
        }
        Ok(())
    }
}

/// Where `buf` methods go: mlua's method table, or the inventory.
pub(crate) trait MethodSink<T> {
    fn method<M, A, R>(&mut self, b: &'static Binding, method: M)
    where
        M: Fn(&Lua, &T, A) -> Result<R> + MaybeSend + 'static,
        A: FromLuaMulti,
        R: IntoLuaMulti;

    fn meta<M, A, R>(&mut self, b: &'static Binding, method: M)
    where
        M: Fn(&Lua, &T, A) -> Result<R> + MaybeSend + 'static,
        A: FromLuaMulti,
        R: IntoLuaMulti;
}

/// The live sink: mlua's `add_methods` argument.
pub(crate) struct LuaMethods<'m, M>(pub(crate) &'m mut M);

impl<T, U: UserDataMethods<T>> MethodSink<T> for LuaMethods<'_, U> {
    fn method<M, A, R>(&mut self, b: &'static Binding, method: M)
    where
        M: Fn(&Lua, &T, A) -> Result<R> + MaybeSend + 'static,
        A: FromLuaMulti,
        R: IntoLuaMulti,
    {
        self.0.add_method(b.name, method);
    }

    fn meta<M, A, R>(&mut self, b: &'static Binding, method: M)
    where
        M: Fn(&Lua, &T, A) -> Result<R> + MaybeSend + 'static,
        A: FromLuaMulti,
        R: IntoLuaMulti,
    {
        self.0.add_meta_method(b.name, method);
    }
}

/// The describe sink: records and registers nothing.
#[derive(Default)]
pub(crate) struct Describe {
    pub(crate) found: Vec<&'static Binding>,
}

impl<T> MethodSink<T> for Describe {
    fn method<M, A, R>(&mut self, b: &'static Binding, _method: M)
    where
        M: Fn(&Lua, &T, A) -> Result<R> + MaybeSend + 'static,
        A: FromLuaMulti,
        R: IntoLuaMulti,
    {
        self.found.push(b);
    }

    fn meta<M, A, R>(&mut self, b: &'static Binding, _method: M)
    where
        M: Fn(&Lua, &T, A) -> Result<R> + MaybeSend + 'static,
        A: FromLuaMulti,
        R: IntoLuaMulti,
    {
        self.found.push(b);
    }
}
