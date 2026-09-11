//! The priced and guarded standard-library natives. Native functions
//! whose cost is not in instructions are wrapped with a price in their
//! arguments; `collectgarbage`, `xpcall` and `setmetatable` are
//! replaced for containment. Errors, not yields, are used because an
//! error crosses non-yieldable C boundaries (`table.sort` comparators,
//! `__gc`, `gsub` callbacks) where a hook yield would be skipped.

use crate::api::reg::Reg;
use crate::api::{Binding, Group, Price, Scope, Sig};
use crate::meter::{charge, with_meter, MeterError};
use kuula_core::meter::price;
use kuula_core::Category;
use mlua::{Error, Function, MultiValue, Result, Value};

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

/// Wrap the native `b` describes with a charge of `before(args)` cycles
/// before the call and `after(results)` after it.
fn replace(
    reg: &mut Reg<'_>,
    b: &'static Binding,
    cat: Category,
    before: impl Fn(&MultiValue) -> u64 + 'static,
    after: impl Fn(&MultiValue) -> u64 + 'static,
) -> Result<()> {
    reg.custom(b, |lua, table| {
        let original: Function = table.get(b.name)?;
        let wrapped = lua.create_function(move |lua, args: MultiValue| {
            charge(lua, cat, before(&args))?;
            let out = original.call::<MultiValue>(args)?;
            let extra = after(&out);
            if extra > 0 {
                charge(lua, cat, extra)?;
            }
            Ok(out)
        })?;
        table.set(b.name, wrapped)
    })
}

/// The price of a pattern call from its arguments: `plain_find` for
/// `string.find`, whose fourth argument can turn the pattern off.
fn pattern_price(a: &MultiValue, plain_find: bool) -> u64 {
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
}

/// Replace the natives whose cost is not visible to the instruction
/// hook with priced versions (audit or wrap costly
/// operations), and close the two hook-free paths.
pub(crate) fn install(reg: &mut Reg<'_>) -> Result<()> {
    replace(reg, &FIND, Category::Str, |a| pattern_price(a, true), |_| 0)?;
    replace(
        reg,
        &MATCH,
        Category::Str,
        |a| pattern_price(a, false),
        |_| 0,
    )?;
    replace(
        reg,
        &GMATCH,
        Category::Str,
        |a| pattern_price(a, false),
        |_| 0,
    )?;
    // `gsub` builds a result; the others return the subject's own bytes
    // or captures of it.
    replace(
        reg,
        &GSUB,
        Category::Str,
        |a| pattern_price(a, false),
        |out| price::bytes8(byte_len(out.front())),
    )?;
    replace(
        reg,
        &REP,
        Category::Str,
        |a| {
            let n = count_arg(a.get(1));
            let each = byte_len(a.front()).saturating_add(byte_len(a.get(2)));
            price::bytes8(each.saturating_mul(n))
        },
        |_| 0,
    )?;
    // Linear in their input or output, and cheap per byte.
    for b in [&UPPER, &LOWER, &REVERSE, &SUB] {
        replace(
            reg,
            b,
            Category::Str,
            |_| 1,
            |out| price::bytes8(byte_len(out.front())),
        )?;
    }
    replace(
        reg,
        &BYTE,
        Category::Str,
        |_| 1,
        |out| price::bytes8(out.len() as u64),
    )?;
    replace(
        reg,
        &CHAR,
        Category::Str,
        |a| price::bytes8(a.len() as u64),
        |_| 0,
    )?;
    for b in [&UTF8_LEN, &UTF8_CODEPOINT, &UTF8_OFFSET, &UTF8_CODES] {
        replace(
            reg,
            b,
            Category::Str,
            |a| price::bytes8(byte_len(a.front())),
            |_| 0,
        )?;
    }
    replace(
        reg,
        &UTF8_CHAR,
        Category::Str,
        |a| price::bytes8(a.len() as u64),
        |_| 0,
    )?;
    // `insert` and `remove` shift the tail past the position; `move`
    // and `unpack` copy a range.
    replace(
        reg,
        &INSERT,
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
        reg,
        &REMOVE,
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
        reg,
        &MOVE,
        Category::Str,
        |a| {
            let from = count_arg(a.get(1));
            let to = count_arg(a.get(2));
            price::bytes8(to.saturating_sub(from).saturating_add(1))
        },
        |_| 0,
    )?;
    replace(
        reg,
        &UNPACK,
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
    replace(
        reg,
        &FORMAT,
        Category::Str,
        |_| 1,
        |out| price::bytes8(byte_len(out.front())),
    )?;
    replace(
        reg,
        &CONCAT,
        Category::Str,
        |_| 1,
        |out| price::bytes8(byte_len(out.front())),
    )?;
    replace(
        reg,
        &SORT,
        Category::Str,
        |a| match a.front() {
            Some(Value::Table(t)) => price::sort(t.raw_len() as u64),
            _ => 1,
        },
        |_| 0,
    )?;
    for b in [&CREATE, &WRAP] {
        replace(reg, b, Category::Api, |_| price::coroutine(), |_| 0)?;
    }
    // The string metatable indexes the same table, so `s:rep(n)` goes
    // through the wrapper too.

    // A full collection walks the heap; `count` is a read. The tuning
    // options stay out so a cart cannot stop the collector.
    reg.custom(&COLLECTGARBAGE, |lua, g| {
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
        )
    })?;

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
    reg.custom(&XPCALL, |lua, g| {
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
        g.set("xpcall", shim)
    })?;

    // `setmetatable`: a `__gc` field is refused. Lua only marks an object
    // for finalisation if the field is present at this call, so adding
    // it later is harmless. `__close` runs with hooks on and stays.
    reg.custom(&SETMETATABLE, |lua, g| {
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
        )
    })?;
    Ok(())
}

/// A priced standard function: the call as Lua defines it, with the
/// price on top of Lua's own semantics.
macro_rules! std {
    ($id:ident, $table:literal, $name:literal, $call:literal, $price:expr) => {
        binding!($id {
            name: $name,
            scope: Scope::Std(Some($table)),
            group: Group::Stdlib,
            sigs: &[Sig::new($call, "as Lua")],
            price: $price,
        });
    };
    ($id:ident, $table:literal, $name:literal, $call:literal, $price:expr, $doc:literal) => {
        binding!($id {
            name: $name,
            scope: Scope::Std(Some($table)),
            group: Group::Stdlib,
            sigs: &[Sig::new($call, "as Lua")],
            price: $price,
            doc: $doc,
        });
    };
}

std!(
    FIND,
    "string",
    "find",
    "string.find(s, pattern, [init, plain])",
    Price::Pattern,
    "With `plain` true the price is that of an unanchored pattern with no quantifiers."
);
std!(
    MATCH,
    "string",
    "match",
    "string.match(s, pattern, [init])",
    Price::Pattern
);
std!(
    GMATCH,
    "string",
    "gmatch",
    "string.gmatch(s, pattern, [init])",
    Price::Pattern
);
std!(
    GSUB,
    "string",
    "gsub",
    "string.gsub(s, pattern, repl, [n])",
    Price::Gsub
);
std!(
    REP,
    "string",
    "rep",
    "string.rep(s, n, [sep])",
    Price::Bytes8("result bytes")
);
std!(
    UPPER,
    "string",
    "upper",
    "string.upper(s)",
    Price::Bytes8("result bytes")
);
std!(
    LOWER,
    "string",
    "lower",
    "string.lower(s)",
    Price::Bytes8("result bytes")
);
std!(
    REVERSE,
    "string",
    "reverse",
    "string.reverse(s)",
    Price::Bytes8("result bytes")
);
std!(
    SUB,
    "string",
    "sub",
    "string.sub(s, i, [j])",
    Price::Bytes8("result bytes")
);
std!(
    BYTE,
    "string",
    "byte",
    "string.byte(s, [i, j])",
    Price::Bytes8("values returned")
);
std!(
    CHAR,
    "string",
    "char",
    "string.char(...)",
    Price::Bytes8("arguments")
);
std!(
    FORMAT,
    "string",
    "format",
    "string.format(fmt, ...)",
    Price::Bytes8("result bytes")
);
std!(
    UTF8_LEN,
    "utf8",
    "len",
    "utf8.len(s, [i, j, lax])",
    Price::Bytes8("subject bytes")
);
std!(
    UTF8_CODEPOINT,
    "utf8",
    "codepoint",
    "utf8.codepoint(s, [i, j, lax])",
    Price::Bytes8("subject bytes")
);
std!(
    UTF8_OFFSET,
    "utf8",
    "offset",
    "utf8.offset(s, n, [i])",
    Price::Bytes8("subject bytes")
);
std!(
    UTF8_CODES,
    "utf8",
    "codes",
    "utf8.codes(s, [lax])",
    Price::Bytes8("subject bytes")
);
std!(
    UTF8_CHAR,
    "utf8",
    "char",
    "utf8.char(...)",
    Price::Bytes8("arguments")
);
std!(
    INSERT,
    "table",
    "insert",
    "table.insert(t, [pos,] v)",
    Price::Bytes8("elements shifted")
);
std!(
    REMOVE,
    "table",
    "remove",
    "table.remove(t, [pos])",
    Price::Bytes8("elements shifted")
);
std!(
    MOVE,
    "table",
    "move",
    "table.move(a, f, e, t, [b])",
    Price::Bytes8("elements copied")
);
std!(
    UNPACK,
    "table",
    "unpack",
    "table.unpack(t, [i, j])",
    Price::Bytes8("elements copied")
);
std!(
    CONCAT,
    "table",
    "concat",
    "table.concat(t, [sep, i, j])",
    Price::Bytes8("result bytes")
);
std!(SORT, "table", "sort", "table.sort(t, [comp])", Price::Sort);
std!(
    CREATE,
    "coroutine",
    "create",
    "coroutine.create(f)",
    Price::Coroutine
);
std!(
    WRAP,
    "coroutine",
    "wrap",
    "coroutine.wrap(f)",
    Price::Coroutine
);

binding!(COLLECTGARBAGE {
    name: "collectgarbage",
    scope: Scope::Std(None),
    group: Group::Stdlib,
    sigs: &[
        Sig::new("collectgarbage(\"count\")", "as Lua"),
        Sig::new("collectgarbage([\"collect\"])", "as Lua").priced(Price::Gc),
        Sig::new("collectgarbage(\"step\", [n])", "as Lua").priced(Price::Gc),
    ],
    price: Price::One,
    doc: "Any other option is a Lua error, so a cart cannot stop or tune \
          the collector.",
});

binding!(XPCALL {
    name: "xpcall",
    scope: Scope::Std(None),
    group: Group::Stdlib,
    sigs: &[Sig::new("xpcall(f, handler, ...)", "as Lua")],
    price: Price::One,
    doc: "The handler is wrapped: once the cart is past its budget the \
          wrapper hands the fault back without running the handler.",
});

binding!(SETMETATABLE {
    name: "setmetatable",
    scope: Scope::Std(None),
    group: Group::Stdlib,
    sigs: &[Sig::new("setmetatable(t, mt)", "as Lua")],
    price: Price::One,
    doc: "A metatable with `__gc` is refused (use `__close`).",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_len_counts_strings_only() {
        let lua = mlua::Lua::new();
        let s = lua.create_string("abcd").unwrap();
        assert_eq!(byte_len(Some(&Value::String(s))), 4);
        assert_eq!(byte_len(Some(&Value::Integer(5))), 24);
        assert_eq!(byte_len(Some(&Value::Nil)), 0);
        assert_eq!(byte_len(None), 0);
    }
}
