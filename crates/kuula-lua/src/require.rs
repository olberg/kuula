//! Cart-local `require`: `src/<name>.lua` only, text chunks, compiled
//! once, cycles refused. Lua's `package` machinery is never enabled
//!.

use std::fmt;

use crate::api::reg::Reg;
use crate::api::{Group, Price, Scope, Sig};
use crate::bindings::with_ctx;
use crate::meter::charge;
use kuula_core::meter::price;
use kuula_core::Category;
use mlua::chunk::ChunkMode;
use mlua::{Error, Lua, Result, Table, Value};

/// Most modules one cart may load.
pub const MAX_MODULES: usize = 256;

const LOADED: &str = "kuula.loaded";
const LOADING: &str = "kuula.loading";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequireError {
    InvalidName { name: String },
    NotFound { name: String, path: String },
    Cycle { name: String },
    Limit { name: String },
    NotUtf8 { path: String },
}

impl RequireError {
    pub fn code(&self) -> &'static str {
        match self {
            RequireError::InvalidName { .. } => "module_name_invalid",
            RequireError::NotFound { .. } => "module_not_found",
            RequireError::Cycle { .. } => "require_cycle",
            RequireError::Limit { .. } => "require_limit",
            RequireError::NotUtf8 { .. } => "module_not_utf8",
        }
    }
}

impl fmt::Display for RequireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            RequireError::InvalidName { name } => write!(
                f,
                "module name {name:?} must be letters, digits, '_' and '.' between components"
            ),
            RequireError::NotFound { name, path } => {
                write!(f, "module {name:?} is not in the cart ({path})")
            }
            RequireError::Cycle { name } => {
                write!(
                    f,
                    "module {name:?} requires itself, directly or through others"
                )
            }
            RequireError::Limit { name } => {
                write!(f, "cannot load {name:?}: more than {MAX_MODULES} modules")
            }
            RequireError::NotUtf8 { path } => write!(f, "{path} is not valid UTF-8"),
        }
    }
}

impl std::error::Error for RequireError {}

/// `a.b_c` to `src/a/b_c.lua`, or the reason it is not a module name.
pub fn module_path(name: &str) -> std::result::Result<String, RequireError> {
    let bad = || RequireError::InvalidName {
        name: name.to_string(),
    };
    if name.is_empty() || name.len() > 128 {
        return Err(bad());
    }
    for piece in name.split('.') {
        if piece.is_empty()
            || !piece
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(bad());
        }
    }
    Ok(format!("src/{}.lua", name.replace('.', "/")))
}

pub(crate) fn install(reg: &mut Reg<'_>) -> Result<()> {
    reg.setup(|lua| {
        lua.set_named_registry_value(LOADED, lua.create_table()?)?;
        lua.set_named_registry_value(LOADING, lua.create_table()?)
    })?;
    reg.function(&REQUIRE, |lua, name: String| require(lua, &name))
}

binding!(REQUIRE {
    name: "require",
    scope: Scope::Global,
    group: Group::Modules,
    sigs: &[Sig::new(
        "require(name)",
        "what `src/<name with . as />.lua` returned (or `true`), loaded once",
    )],
    price: Price::Compile,
    errors: &[
        "module_name_invalid",
        "module_not_found",
        "require_cycle",
        "require_limit",
        "module_not_utf8",
    ],
    doc: "Names are letters, digits and `_` joined by `.`; at most 256 \
          modules; a cycle is an error.",
});

/// Number of key/value pairs in a table, whatever the keys are.
fn entries(t: &Table) -> Result<usize> {
    let mut n = 0;
    for pair in t.pairs::<Value, Value>() {
        pair?;
        n += 1;
    }
    Ok(n)
}

fn require(lua: &Lua, name: &str) -> Result<Value> {
    let path = module_path(name).map_err(Error::external)?;
    let loaded: Table = lua.named_registry_value(LOADED)?;
    let cached: Value = loaded.get(name)?;
    if cached != Value::Nil {
        return Ok(cached);
    }
    let loading: Table = lua.named_registry_value(LOADING)?;
    if loading.get::<bool>(name)? {
        return Err(Error::external(RequireError::Cycle {
            name: name.to_string(),
        }));
    }
    // Both tables are keyed by module name, so the sequence length is
    // always zero; count the entries.
    if entries(&loaded)? + entries(&loading)? >= MAX_MODULES {
        return Err(Error::external(RequireError::Limit {
            name: name.to_string(),
        }));
    }
    let bytes = with_ctx(lua, |ctx| ctx.state.cart.read(&path))?.map_err(|_| {
        Error::external(RequireError::NotFound {
            name: name.to_string(),
            path: path.clone(),
        })
    })?;
    let source = String::from_utf8(bytes)
        .map_err(|_| Error::external(RequireError::NotUtf8 { path: path.clone() }))?;
    // Compilation is native work the hook cannot see; price it by source
    // size before doing it.
    charge(lua, Category::Asset, price::compile(source.len() as u64))?;
    let chunk = lua
        .load(&source)
        .set_name(format!("@{path}"))
        .set_mode(ChunkMode::Text)
        .into_function()?;
    loading.set(name, true)?;
    let result = chunk.call::<Value>(name);
    loading.set(name, Value::Nil)?;
    let value = match result? {
        Value::Nil => Value::Boolean(true),
        v => v,
    };
    loaded.set(name, value.clone())?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_names_map_to_src_paths() {
        assert_eq!(module_path("util").unwrap(), "src/util.lua");
        assert_eq!(
            module_path("game.enemies.bat").unwrap(),
            "src/game/enemies/bat.lua"
        );
        for bad in [
            "", ".", "a..b", ".a", "a.", "a/b", "a-b", "../x", "a b", "ä",
        ] {
            assert!(module_path(bad).is_err(), "{bad:?}");
        }
    }
}
