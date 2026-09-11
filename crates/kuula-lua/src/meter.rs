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

use crate::api::reg::Reg;
use kuula_core::meter::{HOOK_CHARGE, HOOK_INTERVAL};
use kuula_core::{BudgetExceeded, Category, Fault, Meter};
use mlua::debug::Debug;
use mlua::{Error, HookTriggers, Lua, Result, VmState};

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

/// Put a fresh meter in the state, arm the hook and wrap the priced
/// natives (or, in describe mode, collect their descriptors).
pub(crate) fn install(reg: &mut Reg<'_>) -> Result<()> {
    reg.setup(|lua| {
        lua.set_app_data(Meter::new());
        arm(lua, HOOK_INTERVAL)
    })?;
    crate::natives::install(reg)
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
}
