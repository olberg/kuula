//! Binding descriptors: the metadata beside every Lua binding that the
//! reference in `docs/api.md` is generated from.
//!
//! A descriptor is a `static` [`Binding`] declared with [`binding!`]
//! next to the closure it describes, and the same descriptor is what
//! the registration helper ([`reg::Reg`]) uses for the Lua name. The
//! install functions run either against a Lua state or in describe
//! mode, which registers nothing and only collects the descriptors, so
//! [`inventory`] needs no Lua and the generator runs offline.
//!
//! Rust types alone do not say what a binding means to a cart:
//! `Option<f64>` does not name its default, and `print` has a drawing
//! and a logging form. Those facts are explicit fields here. Prices are
//! [`Price`] references to the core's price list, never a second set of
//! numbers.

mod price;
pub(crate) mod reg;

use std::collections::{BTreeMap, BTreeSet};

pub use price::Price;

/// Where a binding lives in the Lua state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// A cart global.
    Global,
    /// A method of the `buf` userdata.
    Method,
    /// A `sys` entry: the shell only, never a cart.
    Sys,
    /// A `net` entry: only a cart whose manifest declares the service.
    Net,
    /// A standard-library entry replaced or wrapped by the runtime, in
    /// the named table (`None` for a base-library global).
    Std(Option<&'static str>),
}

/// A section of the generated reference. Order is the order in the
/// document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Drawing,
    Palette,
    Sprites,
    Buffers,
    BufMethods,
    Input,
    Logging,
    Modules,
    Stdlib,
    Numeric,
    Audio,
    Saves,
    Net,
    Shell,
}

impl Group {
    pub const ALL: [Group; 14] = [
        Group::Drawing,
        Group::Palette,
        Group::Sprites,
        Group::Buffers,
        Group::BufMethods,
        Group::Input,
        Group::Logging,
        Group::Modules,
        Group::Stdlib,
        Group::Numeric,
        Group::Audio,
        Group::Saves,
        Group::Net,
        Group::Shell,
    ];

    /// The marker name of the group's section in `docs/api.md`.
    pub fn key(self) -> &'static str {
        match self {
            Group::Drawing => "drawing",
            Group::Palette => "palette",
            Group::Sprites => "sprites",
            Group::Buffers => "buffers",
            Group::BufMethods => "buf-methods",
            Group::Input => "input",
            Group::Logging => "logging",
            Group::Modules => "modules",
            Group::Stdlib => "stdlib",
            Group::Numeric => "numeric",
            Group::Audio => "audio",
            Group::Saves => "saves",
            Group::Net => "net",
            Group::Shell => "shell",
        }
    }
}

/// One way to call a binding. A binding with several forms (`pal`,
/// `clip`, `print`) has one signature per form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sig {
    /// The call as a cart writes it, optional arguments in brackets.
    pub call: &'static str,
    /// What comes back, or `nothing`.
    pub returns: &'static str,
    /// A price for this form only; `None` uses the binding's.
    pub price: Option<Price>,
    /// A section for this form only; `None` uses the binding's.
    pub group: Option<Group>,
}

impl Sig {
    pub const fn new(call: &'static str, returns: &'static str) -> Sig {
        Sig {
            call,
            returns,
            price: None,
            group: None,
        }
    }

    pub const fn priced(self, price: Price) -> Sig {
        Sig {
            price: Some(price),
            ..self
        }
    }

    pub const fn grouped(self, group: Group) -> Sig {
        Sig {
            group: Some(group),
            ..self
        }
    }
}

/// The metadata of one binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// The Lua name: the global, the method or the table entry.
    pub name: &'static str,
    pub scope: Scope,
    pub group: Group,
    /// At least one.
    pub sigs: &'static [Sig],
    pub price: Price,
    /// Argument defaults, `(argument, default)`.
    pub defaults: &'static [(&'static str, &'static str)],
    /// Stable error codes the call can raise.
    pub errors: &'static [&'static str],
    /// Explanatory prose, one paragraph, empty when the signature says
    /// it all.
    pub doc: &'static str,
}

impl Binding {
    /// The base of a [`binding!`] declaration: every optional field
    /// empty, the required ones missing.
    pub const EMPTY: Binding = Binding {
        name: "",
        scope: Scope::Global,
        group: Group::Drawing,
        sigs: &[],
        price: Price::One,
        defaults: &[],
        errors: &[],
        doc: "",
    };

    /// The name as a cart writes it: `string.rep`, `b:get`, `sys.run`.
    pub fn qualified(&self) -> String {
        match self.scope {
            Scope::Global | Scope::Std(None) => self.name.to_string(),
            Scope::Method => format!("b:{}", self.name),
            Scope::Sys => format!("sys.{}", self.name),
            Scope::Net => format!("net.{}", self.name),
            Scope::Std(Some(table)) => format!("{table}.{}", self.name),
        }
    }

    /// What is wrong with the descriptor, if anything: the checks the
    /// generator and the tests apply to every entry.
    pub fn validate(&self) -> Result<(), String> {
        let name = self.qualified();
        if self.name.is_empty() {
            return Err("a binding has no name".into());
        }
        if self.sigs.is_empty() {
            return Err(format!("{name} has no signature"));
        }
        for sig in self.sigs {
            if sig.call.is_empty() || sig.returns.is_empty() {
                return Err(format!("{name} has an empty signature"));
            }
        }
        let texts = self
            .sigs
            .iter()
            .flat_map(|s| [s.call, s.returns])
            .chain(self.errors.iter().copied())
            .chain(self.defaults.iter().flat_map(|(a, d)| [*a, *d]))
            .chain([self.doc]);
        for text in texts {
            if text.contains(INTERNAL_MARKER) {
                return Err(format!("{name} carries {INTERNAL_MARKER} text"));
            }
            if text.contains('\n') {
                return Err(format!("{name} has a line break in its metadata"));
            }
        }
        Ok(())
    }
}

/// The marker the public export drops lines on. Descriptor text may
/// not carry it: the reference is public, and a dropped line inside a
/// generated section would fail the freshness check of the public copy.
/// Spelled in two pieces so this declaration is not itself a line the
/// export drops; a test walks the tree for any other such line.
pub const INTERNAL_MARKER: &str = concat!("[inter", "nal]");

/// Declare a descriptor `static`. Only the fields given are set; the
/// rest come from [`Binding::EMPTY`], and [`Binding::validate`] refuses
/// a descriptor whose required fields are still empty.
///
/// ```ignore
/// binding!(PSET {
///     name: "pset",
///     group: Group::Drawing,
///     sigs: &[Sig::new("pset(x, y, [c])", "nothing")],
///     price: Price::Pixels,
///     defaults: &[("c", "7")],
/// });
/// reg.function(&PSET, |lua, (x, y, c)| ...)?;
/// ```
macro_rules! binding {
    ($id:ident { $($field:ident : $value:expr),* $(,)? }) => {
        // A descriptor that sets every field still spells the base.
        #[allow(clippy::needless_update)]
        pub(crate) static $id: $crate::api::Binding = $crate::api::Binding {
            $($field: $value,)*
            ..$crate::api::Binding::EMPTY
        };
    };
}

/// Every descriptor, in registration order, with no Lua state built.
/// The order is the order of the install code, so it is stable.
pub fn inventory() -> Vec<&'static Binding> {
    let mut reg = reg::Reg::describe();
    let graveyard: crate::Graveyard = Default::default();
    crate::bindings::install(&mut reg, graveyard).expect("describe mode cannot fail");
    crate::meter::install(&mut reg).expect("describe mode cannot fail");
    crate::sys::install(&mut reg).expect("describe mode cannot fail");
    crate::bindings::net::install(&mut reg).expect("describe mode cannot fail");
    let mut methods = reg::Describe::default();
    crate::bufs::methods(&mut methods);
    let mut found = reg.found;
    found.extend(methods.found);
    found
}

/// Descriptors of `inventory` that fail [`Binding::validate`] or share
/// a name within a scope.
pub fn problems(inventory: &[&Binding]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for b in inventory {
        if let Err(e) = b.validate() {
            out.push(e);
        }
        if !seen.insert((b.scope, b.name)) {
            out.push(format!("{} is declared twice", b.qualified()));
        }
    }
    out
}

/// Base-library globals the sandbox leaves as Lua ships them. Every
/// other global of a cart state must have a descriptor.
pub const STD_GLOBALS: &[&str] = &[
    "_G",
    "_VERSION",
    "assert",
    "coroutine",
    "error",
    "getmetatable",
    "ipairs",
    "math",
    "next",
    "pairs",
    "pcall",
    "rawequal",
    "rawget",
    "rawlen",
    "rawset",
    "select",
    "string",
    "table",
    "tonumber",
    "tostring",
    "type",
    "utf8",
    "warn",
];

/// Standard tables the runtime touches, and the entries in each that it
/// leaves as Lua ships them. Every other entry must have a descriptor.
pub const STD_TABLES: &[(&str, &[&str])] = &[
    (
        "math",
        &[
            "abs",
            "ceil",
            "deg",
            "floor",
            "fmod",
            "frexp",
            "huge",
            "ldexp",
            "max",
            "maxinteger",
            "min",
            "mininteger",
            "modf",
            "pi",
            "rad",
            "random",
            "sqrt",
            "tointeger",
            "type",
            "ult",
        ],
    ),
    ("string", &["len", "pack", "packsize", "unpack"]),
    // `table.create` (Lua 5.5) preallocates; the allocation is bounded
    // by the heap limit, not the meter.
    ("table", &["create", "pack"]),
    ("utf8", &["charpattern"]),
    (
        "coroutine",
        &[
            "close",
            "isyieldable",
            "resume",
            "running",
            "status",
            "yield",
        ],
    ),
];

/// The names a Lua state actually holds, read back from a built guest:
/// the ground truth the descriptors are checked against.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Surface {
    /// Every global.
    pub globals: BTreeSet<String>,
    /// Entries of the standard tables in [`STD_TABLES`].
    pub tables: BTreeMap<String, BTreeSet<String>>,
    /// Methods of the `buf` userdata, with `__tostring` if set.
    pub methods: BTreeSet<String>,
    /// Entries of `sys`, when the state is a shell.
    pub sys: BTreeSet<String>,
    /// Entries of `net`, when the state is a networked cart.
    pub net: BTreeSet<String>,
}

/// Which state a surface is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A plain cart.
    Cart,
    /// The shell: a cart plus `sys`.
    Shell,
    /// A cart that declares the `net` service: a cart plus `net`.
    Net,
}

/// Read the surface of a fresh state of `kind`.
pub fn surface(kind: Kind) -> Surface {
    let guest = match kind {
        Kind::Shell => crate::LuaGuest::new_shell("", "main.lua"),
        Kind::Cart | Kind::Net => crate::LuaGuest::new("", "main.lua"),
    }
    .expect("an empty cart compiles");
    if kind == Kind::Net {
        guest.install_net().expect("net installs");
    }
    let lua = &guest.lua;
    let g = lua.globals();
    let mut out = Surface::default();
    for pair in g.pairs::<String, mlua::Value>() {
        let (k, _) = pair.expect("globals are strings");
        out.globals.insert(k);
    }
    for (table, _) in STD_TABLES {
        let t: mlua::Table = g.get(*table).expect("standard table");
        let mut names = BTreeSet::new();
        for pair in t.pairs::<String, mlua::Value>() {
            names.insert(pair.expect("string keys").0);
        }
        out.tables.insert(table.to_string(), names);
    }
    let screen: mlua::AnyUserData = g.get("screen").expect("screen handle");
    let mt = screen.metatable().expect("buf metatable");
    for pair in mt.pairs::<mlua::Value>() {
        let (k, v) = pair.expect("metatable entries");
        if k == "__index" {
            if let mlua::Value::Table(index) = v {
                for pair in index.pairs::<String, mlua::Value>() {
                    out.methods.insert(pair.expect("method names").0);
                }
            }
        } else if k.starts_with("__") && k != "__name" {
            // `__name` is mlua's type name for error messages, not a
            // method a cart can reach.
            out.methods.insert(k);
        }
    }
    if let Ok(sys) = g.get::<mlua::Table>("sys") {
        for pair in sys.pairs::<String, mlua::Value>() {
            out.sys.insert(pair.expect("sys names").0);
        }
    }
    if let Ok(net) = g.get::<mlua::Table>("net") {
        for pair in net.pairs::<String, mlua::Value>() {
            out.net.insert(pair.expect("net names").0);
        }
    }
    out
}
