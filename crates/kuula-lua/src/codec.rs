//! Lua values to and from the canonical codec, and the
//! `save(slot, table)` and `load(slot)` bindings.
//!
//! Tables are walked with raw access only, so no metamethod runs during
//! a save or a state dump. A `buf` becomes a blob of its storage behind a
//! small header; a blob with that header becomes a fresh `buf` on load.

use crate::api::reg::Reg;
use crate::api::{Group, Price, Scope, Sig};
use crate::bindings::{with_ctx, with_gfx};
use crate::bufs::{self, BufHandle};
use crate::meter::charge;
use crate::Graveyard;
use kuula_core::codec::{self, Budget, CodecError, Key, Table, Value};
use kuula_core::meter::price;
use kuula_core::{BufId, BufKind, Category};
use mlua::{Error, Lua, Result, Value as LuaValue};

/// Flat cost of a `save` or `load` on top of the bytes.
pub const SAVE_FLAT_CYCLES: u64 = 64;

/// Magic at the front of a `buf` blob.
const BLOB_MAGIC: &[u8; 4] = b"KBUF";
const BLOB_HEADER: usize = 13;

/// How the conversion treats what the codec cannot carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// `save`: an error with a codec code.
    Strict,
    /// `state`: a marker string, so a dump never fails on shape.
    Lenient,
}

/// What a conversion needs from a `buf` handle: kind, width, height and
/// raw bytes, or `None` when the buffer is not reachable.
pub(crate) type BufReader<'a> = dyn FnMut(BufId) -> Option<(BufKind, u32, u32, Vec<u8>)> + 'a;

fn kind_byte(kind: BufKind) -> u8 {
    match kind {
        BufKind::U8 => 0,
        BufKind::I16 => 1,
        BufKind::I32 => 2,
        BufKind::F32 => 3,
    }
}

fn kind_of(byte: u8) -> Option<BufKind> {
    Some(match byte {
        0 => BufKind::U8,
        1 => BufKind::I16,
        2 => BufKind::I32,
        3 => BufKind::F32,
        _ => return None,
    })
}

/// The blob bytes of a buffer: header then elements.
pub(crate) fn blob_of(kind: BufKind, w: u32, h: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(BLOB_HEADER + bytes.len());
    out.extend_from_slice(BLOB_MAGIC);
    out.push(kind_byte(kind));
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

/// Split a blob into its buffer shape and elements.
pub(crate) fn blob_shape(blob: &[u8]) -> Option<(BufKind, u32, u32, &[u8])> {
    if blob.len() < BLOB_HEADER || &blob[..4] != BLOB_MAGIC {
        return None;
    }
    let kind = kind_of(blob[4])?;
    let w = u32::from_le_bytes([blob[5], blob[6], blob[7], blob[8]]);
    let h = u32::from_le_bytes([blob[9], blob[10], blob[11], blob[12]]);
    let expected = (w as usize)
        .checked_mul(h as usize)?
        .checked_mul(kind.element_bytes())?;
    let data = &blob[BLOB_HEADER..];
    if data.len() != expected {
        return None;
    }
    Some((kind, w, h, data))
}

fn unsupported(what: &str) -> CodecError {
    CodecError::new(CodecError::UNSUPPORTED, format!("{what} cannot be saved"))
}

struct Walker<'a> {
    mode: Mode,
    bufs: &'a mut BufReader<'a>,
    budget: Budget,
    /// Tables on the path from the root, by address.
    path: Vec<*const std::ffi::c_void>,
}

impl Walker<'_> {
    fn marker(&self, name: &str, err: CodecError) -> std::result::Result<Value, CodecError> {
        match self.mode {
            Mode::Strict => Err(err),
            Mode::Lenient => Ok(Value::str(&format!("<{name}>"))),
        }
    }

    fn value(&mut self, v: &LuaValue) -> std::result::Result<Value, CodecError> {
        match v {
            LuaValue::Nil => Ok(Value::Nil),
            LuaValue::Boolean(b) => {
                self.budget.value(0)?;
                Ok(Value::Bool(*b))
            }
            LuaValue::Integer(i) => {
                self.budget.value(0)?;
                Ok(Value::Int(*i))
            }
            LuaValue::Number(f) => {
                codec::check_float(*f)?;
                self.budget.value(0)?;
                Ok(Value::Float(*f))
            }
            LuaValue::String(s) => {
                let bytes = s.as_bytes();
                self.budget.value(bytes.len())?;
                Ok(Value::Str(bytes.to_vec()))
            }
            LuaValue::Table(t) => self.table(t),
            LuaValue::UserData(ud) => match ud.borrow::<BufHandle>() {
                Ok(handle) => match (self.bufs)(handle.id) {
                    Some((kind, w, h, bytes)) => {
                        self.budget.value(BLOB_HEADER + bytes.len())?;
                        Ok(Value::Blob(blob_of(kind, w, h, &bytes)))
                    }
                    None => self.marker("buf", unsupported("a released or unreachable buf")),
                },
                Err(_) => self.marker("userdata", unsupported("userdata")),
            },
            LuaValue::Function(_) => self.marker("function", unsupported("a function")),
            LuaValue::Thread(_) => self.marker("thread", unsupported("a coroutine")),
            _ => self.marker("userdata", unsupported("this kind of value")),
        }
    }

    fn table(&mut self, t: &mlua::Table) -> std::result::Result<Value, CodecError> {
        let addr = t.to_pointer();
        if self.path.contains(&addr) {
            return self.marker(
                "cycle",
                CodecError::new(CodecError::CYCLE, "table contains itself"),
            );
        }
        self.budget.value(0)?;
        if let Err(e) = self.budget.enter() {
            // `enter` leaves the depth untouched on failure.
            return self.marker("depth", e);
        }
        self.path.push(addr);
        let mut out = Table::new();
        for pair in t.pairs::<LuaValue, LuaValue>() {
            let (k, v) = pair.map_err(|e| {
                CodecError::new(CodecError::UNSUPPORTED, format!("table walk failed: {e}"))
            })?;
            self.budget.entry()?;
            let key = match self.key(&k) {
                Ok(key) => key,
                Err(e) if self.mode == Mode::Lenient => {
                    // A dump keeps going: the entry is dropped, not the dump.
                    let _ = e;
                    continue;
                }
                Err(e) => return Err(e),
            };
            let value = self.value(&v)?;
            if let Value::Nil = value {
                continue;
            }
            out.insert(key, value)?;
        }
        self.path.pop();
        self.budget.leave();
        Ok(Value::Table(out))
    }

    fn key(&mut self, k: &LuaValue) -> std::result::Result<Key, CodecError> {
        match k {
            LuaValue::Boolean(b) => Ok(Key::Bool(*b)),
            LuaValue::Integer(i) => Ok(Key::Int(*i)),
            LuaValue::Number(f) => Key::float(*f),
            LuaValue::String(s) => Ok(Key::Str(s.as_bytes().to_vec())),
            other => Err(CodecError::new(
                CodecError::KEY,
                format!("{} keys cannot be saved", other.type_name()),
            )),
        }
    }
}

/// Convert a Lua value. `bufs` reaches a buffer's bytes for the blob
/// form; returning `None` makes the handle unsupported.
///
/// The walk is bounded by the codec budget: `MAX_ENTRIES` entries and
/// `MAX_DECODED` bytes of values, counted whether or not the walk
/// succeeds, so a caller can price the work of a failed conversion. The
/// second half of the result is the budget the walk consumed.
pub(crate) fn to_value<'a>(
    v: &LuaValue,
    mode: Mode,
    bufs: &'a mut BufReader<'a>,
) -> (std::result::Result<Value, CodecError>, Budget) {
    to_value_within(v, mode, bufs, Budget::default())
}

/// [`to_value`] continuing from `budget`, for a walk that is part of a
/// larger document.
pub(crate) fn to_value_within<'a>(
    v: &LuaValue,
    mode: Mode,
    bufs: &'a mut BufReader<'a>,
    budget: Budget,
) -> (std::result::Result<Value, CodecError>, Budget) {
    let mut w = Walker {
        mode,
        bufs,
        budget,
        path: Vec::new(),
    };
    let out = w.value(v);
    (out, w.budget)
}

/// Build a `buf` handle from a blob's shape and bytes.
pub(crate) type BufMaker<'a> = dyn FnMut(&Lua, BufKind, u32, u32, &[u8]) -> Result<LuaValue> + 'a;

/// Convert a codec value into Lua. Tables are filled with raw sets.
pub(crate) fn to_lua(lua: &Lua, v: &Value, make_buf: &mut BufMaker) -> Result<LuaValue> {
    Ok(match v {
        Value::Nil => LuaValue::Nil,
        Value::Bool(b) => LuaValue::Boolean(*b),
        Value::Int(i) => LuaValue::Integer(*i),
        Value::Float(f) => LuaValue::Number(*f),
        Value::Str(s) => LuaValue::String(lua.create_string(s)?),
        Value::Blob(b) => match blob_shape(b) {
            Some((kind, w, h, data)) => make_buf(lua, kind, w, h, data)?,
            None => return Err(Error::external(unsupported("a blob that is not a buf"))),
        },
        Value::Table(t) => {
            let out = lua.create_table_with_capacity(0, t.len())?;
            for (k, v) in t.iter() {
                let key = match k {
                    Key::Bool(b) => LuaValue::Boolean(*b),
                    Key::Int(i) => LuaValue::Integer(*i),
                    Key::Float(f) => LuaValue::Number(*f),
                    Key::Str(s) => LuaValue::String(lua.create_string(s)?),
                };
                out.raw_set(key, to_lua(lua, v, make_buf)?)?;
            }
            LuaValue::Table(out)
        }
    })
}

/// A buffer's shape and bytes through the frame context; `None` outside a
/// frame or when the handle is dead.
fn read_buf(lua: &Lua, id: BufId) -> Option<(BufKind, u32, u32, Vec<u8>)> {
    with_ctx(lua, |ctx| {
        let buf = ctx.state.res.get(id).ok()?;
        Some((buf.kind(), buf.width(), buf.height(), buf.raw_bytes()))
    })
    .ok()
    .flatten()
}

fn slot_arg(slot: f64) -> Result<u8> {
    let n = kuula_core::buf::to_int(slot);
    u8::try_from(n)
        .ok()
        .filter(|s| *s < kuula_core::save::SLOT_COUNT)
        .ok_or_else(|| {
            Error::external(kuula_core::save::SaveError::new(
                kuula_core::save::SaveError::SLOT,
                format!("slot {n} is not 0 to {}", kuula_core::save::SLOT_COUNT - 1),
            ))
        })
}

pub(crate) fn install(reg: &mut Reg<'_>, graveyard: Graveyard) -> Result<()> {
    reg.function(&SAVE, |lua, (slot, value): (f64, LuaValue)| {
        let slot = slot_arg(slot)?;
        if !matches!(value, LuaValue::Table(_)) {
            return Err(Error::runtime(format!(
                "save takes a table, got {}",
                value.type_name()
            )));
        }
        charge(lua, Category::Api, SAVE_FLAT_CYCLES)?;
        let mut bufs = |id: BufId| read_buf(lua, id);
        let (v, walked) = to_value(&value, Mode::Strict, &mut bufs);
        // The walk is paid for whether or not it produced a value, so
        // a save that fails on size is not cheaper than one that fits.
        charge(lua, Category::Api, price::bytes8(walked.decoded as u64))?;
        let v = v.map_err(Error::external)?;
        let text = codec::encode(&v).map_err(Error::external)?;
        charge(lua, Category::Api, price::bytes8(text.len() as u64))?;
        with_ctx(lua, |ctx| ctx.state.saves.write(slot, text.as_bytes()))?.map_err(Error::external)
    })?;

    let gy = graveyard.clone();
    reg.function(&LOAD, move |lua, slot: f64| {
        let slot = slot_arg(slot)?;
        charge(lua, Category::Api, SAVE_FLAT_CYCLES)?;
        let bytes = with_ctx(lua, |ctx| ctx.state.saves.read(slot))?.map_err(Error::external)?;
        let Some(bytes) = bytes else {
            return Ok(LuaValue::Nil);
        };
        charge(lua, Category::Api, price::bytes8(bytes.len() as u64))?;
        let v = codec::decode_bytes(&bytes).map_err(Error::external)?;
        let gy = gy.clone();
        let mut make_buf = move |lua: &Lua, kind: BufKind, w: u32, h: u32, data: &[u8]| {
            charge(lua, Category::Buf, 1)?;
            let id = with_gfx(lua, |ctx| ctx.state.buf_alloc(kind, w, h))?;
            let ok = with_gfx(lua, |ctx| {
                Ok(ctx.state.res.get_mut(id)?.set_raw_bytes(data))
            })?;
            debug_assert!(ok, "blob shape was checked");
            charge(lua, Category::Buf, price::bytes128(data.len() as u64))?;
            bufs::handle(lua, id, &gy)
        };
        to_lua(lua, &v, &mut make_buf)
    })?;

    Ok(())
}

const SAVE_ERRORS: &[&str] = &[
    "save_slot",
    "save_size",
    "codec_unsupported",
    "codec_cycle",
    "codec_depth",
    "codec_size",
    "codec_key",
    "codec_number",
];

binding!(SAVE {
    name: "save",
    scope: Scope::Global,
    group: Group::Saves,
    sigs: &[Sig::new("save(slot, table)", "nothing; slot 0 to 7")],
    price: Price::Save,
    errors: SAVE_ERRORS,
    doc: "A value that is not a table is a Lua error. The walk is paid for \
          whether or not the value fits, so a save that fails on size is \
          not cheaper than one that succeeds.",
});

binding!(LOAD {
    name: "load",
    scope: Scope::Global,
    group: Group::Saves,
    sigs: &[Sig::new(
        "load(slot)",
        "the table, or `nil` when the slot is empty",
    )],
    price: Price::Load,
    errors: &[
        "save_slot",
        "codec_syntax",
        "codec_depth",
        "codec_size",
        "codec_key",
        "codec_number",
        "graphics_budget_exceeded",
    ],
    doc: "Every `buf` in the table comes back as a fresh buffer, priced \
          like `buf`. A slot whose text does not decode is a codec error.",
});

/// The named globals as one canonical document, for `Guest::state`.
pub(crate) fn dump_globals(lua: &Lua, names: &[String]) -> std::result::Result<String, CodecError> {
    let globals = lua.globals();
    let mut bufs = |_id: BufId| None;
    let mut table = Table::new();
    // The document is one table of the named globals, so the walk starts
    // one level down and the budget runs across all of them, the way
    // the encoder will count them.
    let mut budget = Budget::default();
    budget.enter()?;
    for name in names {
        budget.entry()?;
        let v: LuaValue = globals.raw_get(name.as_str()).map_err(|e| {
            CodecError::new(CodecError::UNSUPPORTED, format!("cannot read {name}: {e}"))
        })?;
        let (value, walked) = to_value_within(&v, Mode::Lenient, &mut bufs, budget);
        budget = walked;
        let value = value?;
        if let Value::Nil = value {
            continue;
        }
        table.insert(Key::Str(name.as_bytes().to_vec()), value)?;
    }
    codec::encode(&Value::Table(table))
}
