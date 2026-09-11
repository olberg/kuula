//! Lua 5.5 guest for Kuula through mlua.
//!
//! The Lua state holds no pixels. During a step the console's
//! [`DrawState`] is moved into the state's app data, the bindings borrow it
//! from there, and it is moved back out before the step returns. Buffers
//! are handles: a `buf` userdata carries a `BufId`, and when Lua collects
//! one the id lands in a graveyard the guest frees at the next step.
//!
//! Every step is metered (see [`meter`]) and the heap is capped: a soft
//! cap checked after each step with a full collection and a re-check, and
//! mlua's hard limit above it, so a runaway allocation inside one
//! callback is contained by the state.

#[macro_use]
pub mod api;
mod audio;
mod bindings;
mod bufs;
mod codec;
mod fault;
pub mod meter;
mod natives;
mod numeric;
mod require;
mod sys;

use std::cell::RefCell;
use std::rc::Rc;

use kuula_core::meter::{FRAME_BUDGET, INIT_BUDGET};
use kuula_core::{BufId, Console, DrawState, Fault, FrameInput, Guest, FRAME_DT};
use mlua::chunk::ChunkMode;
use mlua::{Function, Lua, LuaOptions, StdLib};

pub use bindings::RANDOM_SEED;
pub use fault::fault_from_lua;

/// Lua heap bytes above which a step ends in `out_of_memory` after a
/// full collection failed to bring it back under.
pub const LUA_HEAP_SOFT_CAP: usize = 16 * 1024 * 1024;

/// mlua's allocation limit: the hard backstop inside a callback.
pub const LUA_HEAP_HARD_LIMIT: usize = 24 * 1024 * 1024;

/// Live Lua handles per buffer, and the ids whose last handle has been
/// collected. A named load returns the cached buffer under a fresh
/// handle, so several handles may share one id; the buffer is only
/// given up once every one of them is gone.
#[derive(Default)]
pub(crate) struct Handles {
    live: std::collections::HashMap<BufId, u32>,
    dead: Vec<BufId>,
}

impl Handles {
    /// A new handle for `id` was created.
    pub(crate) fn acquire(&mut self, id: BufId) {
        *self.live.entry(id).or_insert(0) += 1;
    }

    /// A handle for `id` was collected. The last one buries the buffer.
    pub(crate) fn release(&mut self, id: BufId) {
        let last = match self.live.get_mut(&id) {
            Some(n) if *n > 1 => {
                *n -= 1;
                false
            }
            _ => true,
        };
        if last {
            self.live.remove(&id);
            self.dead.push(id);
        }
    }

    /// Ids that no handle refers to any more, cleared.
    pub(crate) fn drain_dead(&mut self) -> Vec<BufId> {
        std::mem::take(&mut self.dead)
    }
}

pub(crate) type Graveyard = Rc<RefCell<Handles>>;

/// What the bindings can reach during a step.
pub(crate) struct FrameCtx {
    pub state: DrawState,
    pub input: FrameInput,
    pub frame: u64,
}

/// A compiled cart. Created without running any cart code.
pub struct LuaGuest {
    lua: Lua,
    chunk: Option<Function>,
    chunk_name: String,
    started: bool,
    graveyard: Graveyard,
}

impl LuaGuest {
    /// Create a sandboxed state, register the cart API and compile
    /// `source` under `name` without running it. A syntax error is a
    /// `compile_error` fault.
    pub fn new(source: &str, name: &str) -> Result<LuaGuest, Fault> {
        let libs = StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::UTF8 | StdLib::COROUTINE;
        let lua = Lua::new_with(libs, LuaOptions::default())
            .map_err(|e| fault_from_lua(&e, name, Fault::RUNTIME_ERROR))?;
        // The hard limit goes on before any cart source is loaded.
        lua.set_memory_limit(LUA_HEAP_HARD_LIMIT)
            .map_err(|e| fault_from_lua(&e, name, Fault::RUNTIME_ERROR))?;
        let graveyard: Graveyard = Rc::new(RefCell::new(Handles::default()));
        let mut reg = api::reg::Reg::lua(&lua);
        bindings::install(&mut reg, graveyard.clone())
            .map_err(|e| fault_from_lua(&e, name, Fault::RUNTIME_ERROR))?;
        meter::install(&mut reg).map_err(|e| fault_from_lua(&e, name, Fault::RUNTIME_ERROR))?;
        drop(reg);
        // Lua reports a chunk named `@main.lua` as `main.lua:line:`. Text
        // mode refuses precompiled chunks.
        let chunk = lua
            .load(source)
            .set_name(format!("@{name}"))
            .set_mode(ChunkMode::Text)
            .into_function()
            .map_err(|e| fault_from_lua(&e, name, Fault::COMPILE_ERROR))?;
        Ok(LuaGuest {
            lua,
            chunk: Some(chunk),
            chunk_name: name.to_string(),
            started: false,
            graveyard,
        })
    }

    /// The shell guest: the same sandbox plus the `sys` table
    ///. Only the host builds one.
    pub fn new_shell(source: &str, name: &str) -> Result<LuaGuest, Fault> {
        let guest = LuaGuest::new(source, name)?;
        sys::install(&mut api::reg::Reg::lua(&guest.lua))
            .map_err(|e| fault_from_lua(&e, name, Fault::RUNTIME_ERROR))?;
        Ok(guest)
    }

    /// Install the `net` table. The console's first step does this for
    /// a cart whose draw state carries a `NetState`; nothing else may.
    pub(crate) fn install_net(&self) -> mlua::Result<()> {
        bindings::net::install(&mut api::reg::Reg::lua(&self.lua))
    }

    /// Boxed factory with the signature [`Console::new`] wants.
    pub fn factory(source: &str, name: &str) -> Result<Box<dyn Guest>, Fault> {
        LuaGuest::new(source, name).map(|g| Box::new(g) as Box<dyn Guest>)
    }

    /// Build a console from a cart source using this guest.
    pub fn console(source: Rc<dyn kuula_core::CartSource>) -> Console {
        Console::new(source, LuaGuest::factory)
    }

    /// Bytes the Lua state currently holds.
    pub fn used_memory(&self) -> usize {
        self.lua.used_memory()
    }

    pub fn gc_collect(&self) {
        let _ = self.lua.gc_collect();
    }

    /// The meter's stored fault, if the cart has been stopped.
    pub fn meter_fault(&self) -> Option<Fault> {
        meter::with_meter(&self.lua, |m| m.fault().cloned())
            .ok()
            .flatten()
    }

    fn global_function(&self, name: &str) -> mlua::Result<Option<Function>> {
        self.lua.globals().get::<Option<Function>>(name)
    }

    fn set_callback(&self, name: &'static str) -> mlua::Result<()> {
        meter::with_meter(&self.lua, |m| m.set_callback(name))
    }

    fn run_frame(&mut self) -> mlua::Result<()> {
        if !self.started {
            self.started = true;
            if let Some(chunk) = self.chunk.take() {
                self.set_callback("main chunk")?;
                chunk.call::<()>(())?;
            }
            if let Some(init) = self.global_function("_init")? {
                self.set_callback("_init")?;
                init.call::<()>(())?;
            }
        } else {
            if let Some(update) = self.global_function("_update")? {
                self.set_callback("_update")?;
                update.call::<()>(FRAME_DT)?;
            }
            if let Some(draw) = self.global_function("_draw")? {
                self.set_callback("_draw")?;
                draw.call::<()>(())?;
            }
        }
        Ok(())
    }

    /// The soft heap cap: collect, re-check, never re-run (architecture
    /// 7.3).
    fn check_heap(&self) -> Result<(), Fault> {
        if self.lua.used_memory() <= LUA_HEAP_SOFT_CAP {
            return Ok(());
        }
        self.gc_collect();
        let used = self.lua.used_memory();
        if used <= LUA_HEAP_SOFT_CAP {
            return Ok(());
        }
        Err(Fault::new(
            Fault::OUT_OF_MEMORY,
            &self.chunk_name,
            None,
            format!(
                "Lua heap holds {used} bytes after a full collection, cap is {LUA_HEAP_SOFT_CAP}"
            ),
        ))
    }
}

impl Guest for LuaGuest {
    /// The named globals through the lenient codec conversion: raw gets,
    /// no metamethods, markers for what cannot be carried. Runs outside
    /// a step, so `buf` bytes are not reachable.
    fn state(&mut self, names: &[String]) -> Result<String, Fault> {
        codec::dump_globals(&self.lua, names)
            .map_err(|e| Fault::new(e.code, &self.chunk_name, None, e.message))
    }

    fn step(&mut self, state: &mut DrawState, input: FrameInput, frame: u64) -> Result<(), Fault> {
        // Buffers Lua let go of since the last step.
        let dead = self.graveyard.borrow_mut().drain_dead();
        state.reap(dead);
        let budget = if self.started {
            FRAME_BUDGET
        } else {
            INIT_BUDGET
        };
        // The gated table goes in before any cart code runs, and only
        // when the console gave this cart the service.
        if !self.started && state.net.is_some() {
            self.install_net()
                .map_err(|e| fault_from_lua(&e, &self.chunk_name, Fault::RUNTIME_ERROR))?;
        }
        let ctx = FrameCtx {
            state: std::mem::replace(state, DrawState::placeholder()),
            input,
            frame,
        };
        let began = meter::with_meter(&self.lua, |m| m.begin_frame(budget));
        self.lua.set_app_data(ctx);
        let result = began.and_then(|()| self.run_frame());
        let ctx = self
            .lua
            .remove_app_data::<FrameCtx>()
            .expect("frame context survives the step");
        *state = ctx.state;

        let mut outcome =
            result.map_err(|e| fault_from_lua(&e, &self.chunk_name, Fault::RUNTIME_ERROR));
        // The meter's own record wins over whatever error object reached
        // us: a cart can replace the error value but not the meter.
        if let Some(fault) = self.meter_fault() {
            outcome = Err(fault);
        }
        if outcome.is_ok() {
            outcome = self.check_heap();
            if let Err(f) = &outcome {
                let _ = meter::with_meter(&self.lua, |m| m.kill(f.clone()));
            }
        }
        let used = self.lua.used_memory() as u64;
        state.profile = meter::with_meter(&self.lua, |m| m.profile(used)).unwrap_or_default();
        outcome
    }
}

#[cfg(test)]
mod tests;
