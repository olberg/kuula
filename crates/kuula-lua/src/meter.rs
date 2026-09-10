//! Metering the guest.
//!
//! The core's [`Meter`] lives in the Lua state's app data. Bindings
//! charge through [`charge`]; a count hook charges [`HOOK_CHARGE`] every
//! [`HOOK_INTERVAL`] instructions. The first charge past the budget
//! records a `budget_exceeded` fault, marks the meter dead, and raises.
//!
//! Containment after that point does not depend on the cart's
//! cooperation:
//!
//! - every binding refuses with the same fault while the meter is dead;
//! - the hook is re-armed to fire on every instruction, so a `pcall`
//!   that swallowed the error lets at most one instruction run before it
//!   is raised again, one frame further out, until it reaches the host;
//! - the guest reads the stored fault back at the end of the step, so a
//!   cart that replaced the error object (an `xpcall` handler, a string
//!   concatenation) still reports the original code;
//! - creating a coroutine costs one hook interval, because a new thread
//!   starts with a reset instruction count.
//!
//! Native functions whose cost is not in instructions are wrapped with a
//! price in their arguments. Errors, not yields, are used because an
//! error crosses non-yieldable C boundaries (`table.sort` comparators,
//! `__gc`, `gsub` callbacks) where a hook yield would be skipped.

use std::fmt;

use kuula_core::meter::{price, HOOK_CHARGE, HOOK_INTERVAL};
use kuula_core::{BudgetExceeded, Category, Fault, Meter};
use mlua::debug::Debug;
use mlua::{Error, Function, HookTriggers, Lua, MultiValue, Result, Table, Value, VmState};

/// The fault as a Lua error value, so `pcall` sees something and
/// `fault_from_lua` can read the code back.
#[derive(Debug, Clone)]
pub struct MeterError(pub Fault);

impl fmt::Display for MeterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}: {}",
            self.0.location(),
            self.0.code,
            self.0.message
        )
    }
}

impl std::error::Error for MeterError {}

/// Put a fresh meter in the state and arm the hook.
pub fn install(lua: &Lua) -> Result<()> {
    lua.set_app_data(Meter::new());
    arm(lua, HOOK_INTERVAL)?;
    wrap_natives(lua)?;
    Ok(())
}

fn arm(lua: &Lua, every: u32) -> Result<()> {
    lua.set_global_hook(HookTriggers::new().every_nth_instruction(every), hook)
}

/// Run `f` on the meter. Fails only if a binding is somehow re-entered
/// while it holds the meter, which no binding does.
pub fn with_meter<R>(lua: &Lua, f: impl FnOnce(&mut Meter) -> R) -> Result<R> {
    let mut m = lua
        .try_app_data_mut::<Meter>()
        .map_err(|_| Error::runtime("meter re-entered"))?
        .ok_or_else(|| Error::runtime("no meter in this state"))?;
    Ok(f(&mut m))
}

/// Spend `n` cycles in `cat` from a binding. The fault's location is the
/// Lua function that called the binding.
pub fn charge(lua: &Lua, cat: Category, n: u64) -> Result<()> {
    let outcome = with_meter(lua, |m| {
        if let Some(f) = m.fault() {
            return Err(Error::external(MeterError(f.clone())));
        }
        Ok(m.charge(cat, n))
    })?;
    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(over)) => {
            let loc = lua.inspect_stack(1, location).flatten();
            trip(lua, over, loc)
        }
        Err(e) => Err(e),
    }
}

/// Record the fault, re-arm the hook at every instruction and raise.
fn trip(lua: &Lua, over: BudgetExceeded, loc: Option<(String, u32)>) -> Result<()> {
    let (file, line) = match loc {
        Some((f, l)) => (f, Some(l)),
        None => ("main.lua".to_string(), None),
    };
    let fault = Fault::new(Fault::BUDGET_EXCEEDED, &file, line, over.message());
    with_meter(lua, |m| m.kill(fault.clone()))?;
    arm(lua, 1)?;
    Err(Error::external(MeterError(fault)))
}

/// `file` and line of a Lua frame, if it is one.
fn location(d: &Debug) -> Option<(String, u32)> {
    let src = d.source();
    let file = src.short_src.as_deref()?.to_string();
    if !file.ends_with(".lua") {
        return None;
    }
    Some((file, d.current_line()? as u32))
}

fn hook(lua: &Lua, d: &Debug) -> Result<VmState> {
    let outcome = with_meter(lua, |m| {
        if let Some(f) = m.fault() {
            return Err(Error::external(MeterError(f.clone())));
        }
        Ok(m.charge(Category::Lua, HOOK_CHARGE))
    })?;
    match outcome {
        Ok(Ok(())) => Ok(VmState::Continue),
        Ok(Err(over)) => trip(lua, over, location(d)).map(|_| VmState::Continue),
        Err(e) => Err(e),
    }
}

// ----- priced natives ------------------------------------------------------

/// An integer argument the way `luaL_checkinteger` reads it: numbers
/// and numeric strings both count, anything else is 0 (the native will
/// raise its own argument error).
fn count_arg(v: Option<&Value>) -> u64 {
    let n = match v {
        Some(Value::Integer(n)) => *n as f64,
        Some(Value::Number(n)) => *n,
        Some(Value::String(s)) => s
            .to_str()
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .unwrap_or(0.0),
        _ => 0.0,
    };
    if n.is_finite() && n > 0.0 {
        n as u64
    } else {
        0
    }
}

fn byte_len(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::String(s)) => s.as_bytes().len() as u64,
        Some(Value::Integer(_)) | Some(Value::Number(_)) => 24,
        _ => 0,
    }
}

fn replace(
    lua: &Lua,
    table: &Table,
    name: &str,
    cat: Category,
    before: impl Fn(&MultiValue) -> u64 + 'static,
    after: impl Fn(&MultiValue) -> u64 + 'static,
) -> Result<()> {
    let original: Function = table.get(name)?;
    let wrapped = lua.create_function(move |lua, args: MultiValue| {
        charge(lua, cat, before(&args))?;
        let out = original.call::<MultiValue>(args)?;
        let extra = after(&out);
        if extra > 0 {
            charge(lua, cat, extra)?;
        }
        Ok(out)
    })?;
    table.set(name, wrapped)
}

/// Replace the natives whose cost is not visible to the instruction
/// hook with priced versions (audit or wrap costly
/// operations).
fn wrap_natives(lua: &Lua) -> Result<()> {
    let g = lua.globals();
    let string: Table = g.get("string")?;
    let table: Table = g.get("table")?;
    let coroutine: Table = g.get("coroutine")?;

    for name in ["find", "match", "gmatch", "gsub"] {
        let plain_find = name == "find";
        replace(
            lua,
            &string,
            name,
            Category::Str,
            move |a| {
                let subject = byte_len(a.front());
                let (pattern, (quantifiers, anchored)) = match a.get(1) {
                    Some(Value::String(p)) => (
                        p.as_bytes().len() as u64,
                        price::pattern_shape(&p.as_bytes()),
                    ),
                    other => (byte_len(other), (0, false)),
                };
                // `find(s, p, init, true)` is a plain substring search.
                let plain = plain_find && matches!(a.get(3), Some(Value::Boolean(true)));
                if plain {
                    price::pattern(subject, pattern, 0, false)
                } else {
                    price::pattern(subject, pattern, quantifiers, anchored)
                }
            },
            // `gsub` builds a result; the others return the subject's
            // own bytes or captures of it.
            move |out| {
                if name == "gsub" {
                    price::bytes8(byte_len(out.front()))
                } else {
                    0
                }
            },
        )?;
    }
    replace(
        lua,
        &string,
        "rep",
        Category::Str,
        |a| {
            let n = count_arg(a.get(1));
            let each = byte_len(a.front()).saturating_add(byte_len(a.get(2)));
            price::bytes8(each.saturating_mul(n))
        },
        |_| 0,
    )?;
    // Linear in their input or output, and cheap per byte.
    for name in ["upper", "lower", "reverse", "sub"] {
        replace(
            lua,
            &string,
            name,
            Category::Str,
            |_| 1,
            |out| price::bytes8(byte_len(out.front())),
        )?;
    }
    replace(
        lua,
        &string,
        "byte",
        Category::Str,
        |_| 1,
        |out| price::bytes8(out.len() as u64),
    )?;
    replace(
        lua,
        &string,
        "char",
        Category::Str,
        |a| price::bytes8(a.len() as u64),
        |_| 0,
    )?;
    let utf8: Table = g.get("utf8")?;
    for name in ["len", "codepoint", "offset", "codes"] {
        replace(
            lua,
            &utf8,
            name,
            Category::Str,
            |a| price::bytes8(byte_len(a.front())),
            |_| 0,
        )?;
    }
    replace(
        lua,
        &utf8,
        "char",
        Category::Str,
        |a| price::bytes8(a.len() as u64),
        |_| 0,
    )?;
    // `insert` and `remove` shift the tail past the position; `move`
    // and `unpack` copy a range.
    replace(
        lua,
        &table,
        "insert",
        Category::Str,
        |a| match (a.front(), a.len()) {
            (Some(Value::Table(t)), 3) => {
                let len = t.raw_len() as u64;
                price::bytes8(len.saturating_sub(count_arg(a.get(1))))
            }
            _ => 1,
        },
        |_| 0,
    )?;
    replace(
        lua,
        &table,
        "remove",
        Category::Str,
        |a| match (a.front(), a.get(1)) {
            (Some(Value::Table(t)), Some(pos)) => {
                let len = t.raw_len() as u64;
                price::bytes8(len.saturating_sub(count_arg(Some(pos))))
            }
            _ => 1,
        },
        |_| 0,
    )?;
    replace(
        lua,
        &table,
        "move",
        Category::Str,
        |a| {
            let from = count_arg(a.get(1));
            let to = count_arg(a.get(2));
            price::bytes8(to.saturating_sub(from).saturating_add(1))
        },
        |_| 0,
    )?;
    replace(
        lua,
        &table,
        "unpack",
        Category::Str,
        |a| match a.front() {
            Some(Value::Table(t)) => {
                let len = t.raw_len() as u64;
                let from = a.get(1).map_or(1, |v| count_arg(Some(v)));
                let to = a.get(2).map_or(len, |v| count_arg(Some(v)));
                price::bytes8(to.saturating_sub(from).saturating_add(1))
            }
            _ => 1,
        },
        |_| 0,
    )?;
    // A full collection walks the heap; `count` is a read. The tuning
    // options stay out so a cart cannot stop the collector.
    let collectgarbage: Function = g.get("collectgarbage")?;
    g.set(
        "collectgarbage",
        lua.create_function(move |lua, (opt, arg): (Option<String>, Value)| {
            let opt = opt.unwrap_or_else(|| "collect".to_string());
            match opt.as_str() {
                "count" => charge(lua, Category::Api, 1)?,
                "collect" | "step" => charge(
                    lua,
                    Category::Api,
                    price::bytes128(lua.used_memory() as u64),
                )?,
                other => {
                    return Err(Error::runtime(format!(
                        "collectgarbage({other:?}) is not available to carts"
                    )))
                }
            }
            collectgarbage.call::<MultiValue>((opt, arg))
        })?,
    )?;
    replace(
        lua,
        &string,
        "format",
        Category::Str,
        |_| 1,
        |out| price::bytes8(byte_len(out.front())),
    )?;
    replace(
        lua,
        &table,
        "concat",
        Category::Str,
        |_| 1,
        |out| price::bytes8(byte_len(out.front())),
    )?;
    replace(
        lua,
        &table,
        "sort",
        Category::Str,
        |a| match a.front() {
            Some(Value::Table(t)) => price::sort(t.raw_len() as u64),
            _ => 1,
        },
        |_| 0,
    )?;
    for name in ["create", "wrap"] {
        replace(
            lua,
            &coroutine,
            name,
            Category::Api,
            |_| price::coroutine(),
            |_| 0,
        )?;
    }
    // The string metatable indexes the same table, so `s:rep(n)` goes
    // through the wrapper too.

    // Lua runs two things with hooks switched off (ldo.c `luaD_hook`,
    // lgc.c `GCTM`): the message handler of an error raised from inside
    // a hook, and `__gc` finalisers. A loop in either is invisible to
    // the count hook, so both are closed here rather than metered.
    //
    // `xpcall`: the native stays, because it is yieldable and a Rust
    // replacement would not be; only the handler is wrapped. When the
    // meter is dead the wrapper hands the fault back without running
    // cart code, and a live meter means the error did not come from
    // the hook, so the handler runs with hooks on as usual. Error
    // values and return values are untouched.
    let guard = lua.create_function(|lua, handler: Value| {
        charge(lua, Category::Api, 1)?;
        let handler = match handler {
            Value::Function(f) => f,
            other => {
                return Err(Error::runtime(format!(
                    "bad argument #2 to 'xpcall' (function expected, got {})",
                    other.type_name()
                )))
            }
        };
        lua.create_function(move |lua, err: Value| {
            if let Some(fault) = with_meter(lua, |m| m.fault().cloned())? {
                return Err(Error::external(MeterError(fault)));
            }
            handler.call::<Value>(err)
        })
    })?;
    let xpcall: Function = g.get("xpcall")?;
    let shim: Function = lua
        .load(
            "local xpcall, guard = ...
             return function(f, handler, ...)
               return xpcall(f, guard(handler), ...)
             end",
        )
        .set_name("=xpcall")
        .call((xpcall, guard))?;
    g.set("xpcall", shim)?;

    // `setmetatable`: a `__gc` field is refused. Lua only marks an object
    // for finalisation if the field is present at this call, so adding
    // it later is harmless. `__close` runs with hooks on and stays.
    let setmetatable: Function = g.get("setmetatable")?;
    g.set(
        "setmetatable",
        lua.create_function(move |lua, (t, mt): (Value, Value)| {
            charge(lua, Category::Api, 1)?;
            if let Value::Table(m) = &mt {
                if m.raw_get::<Value>("__gc")? != Value::Nil {
                    return Err(Error::runtime(
                        "__gc is not available to carts; use a to-be-closed variable (__close)",
                    ));
                }
            }
            setmetatable.call::<Value>((t, mt))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_error_displays_like_a_lua_error() {
        let e = MeterError(Fault::new(
            "budget_exceeded",
            "main.lua",
            Some(4),
            "x used 2 of 1 cycles",
        ));
        assert_eq!(
            e.to_string(),
            "main.lua:4: budget_exceeded: x used 2 of 1 cycles"
        );
    }

    #[test]
    fn byte_len_counts_strings_only() {
        let lua = Lua::new();
        let s = lua.create_string("abcd").unwrap();
        assert_eq!(byte_len(Some(&Value::String(s))), 4);
        assert_eq!(byte_len(Some(&Value::Integer(5))), 24);
        assert_eq!(byte_len(Some(&Value::Nil)), 0);
        assert_eq!(byte_len(None), 0);
    }
}
