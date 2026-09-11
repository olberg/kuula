//! The descriptors against the state they describe. The inventory is
//! what the install code declared; the surface is what a built Lua
//! state holds. They must agree exactly, after an explicit baseline of
//! untouched standard-library names, so a binding registered past the
//! helper or a descriptor with no binding fails here.

use std::collections::BTreeSet;

use super::{console, run};
use crate::api::{
    inventory, problems, surface, Binding, Group, Kind, Price, Scope, Sig, STD_GLOBALS, STD_TABLES,
};

fn names<'a>(it: impl Iterator<Item = &'a &'static Binding>) -> BTreeSet<String> {
    it.map(|b| b.name.to_string()).collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn every_descriptor_validates_and_names_are_unique_per_scope() {
    let inv = inventory();
    assert!(!inv.is_empty());
    assert_eq!(problems(&inv), Vec::<String>::new());
}

#[test]
fn cart_globals_are_the_baseline_plus_the_described_ones() {
    let inv = inventory();
    let cart = surface(Kind::Cart);
    let described = names(
        inv.iter()
            .filter(|b| matches!(b.scope, Scope::Global | Scope::Std(None))),
    );
    let baseline = set(STD_GLOBALS);
    assert!(
        described.is_disjoint(&baseline),
        "described and baseline overlap: {:?}",
        described.intersection(&baseline).collect::<Vec<_>>()
    );
    let expected: BTreeSet<String> = described.union(&baseline).cloned().collect();
    assert_eq!(
        cart.globals,
        expected,
        "undescribed globals: {:?}; described but absent: {:?}",
        cart.globals.difference(&expected).collect::<Vec<_>>(),
        expected.difference(&cart.globals).collect::<Vec<_>>()
    );
    assert!(cart.sys.is_empty(), "a cart has no sys table");
    assert!(!cart.globals.contains("sys"));
    assert!(cart.net.is_empty(), "a plain cart has no net table");
    assert!(!cart.globals.contains("net"));
}

#[test]
fn standard_tables_are_the_baseline_plus_the_described_ones() {
    let inv = inventory();
    let cart = surface(Kind::Cart);
    for (table, untouched) in STD_TABLES {
        let described = names(inv.iter().filter(|b| b.scope == Scope::Std(Some(table))));
        let baseline = set(untouched);
        assert!(
            described.is_disjoint(&baseline),
            "{table}: described and baseline overlap: {:?}",
            described.intersection(&baseline).collect::<Vec<_>>()
        );
        let expected: BTreeSet<String> = described.union(&baseline).cloned().collect();
        let actual = &cart.tables[*table];
        assert_eq!(
            actual,
            &expected,
            "{table}: undescribed: {:?}; described but absent: {:?}",
            actual.difference(&expected).collect::<Vec<_>>(),
            expected.difference(actual).collect::<Vec<_>>()
        );
    }
    // Every described standard table is one the surface reads.
    for b in &inv {
        if let Scope::Std(Some(table)) = b.scope {
            assert!(
                STD_TABLES.iter().any(|(t, _)| *t == table),
                "{} lives in a table the surface does not read",
                b.qualified()
            );
        }
    }
}

#[test]
fn buf_methods_are_exactly_the_described_ones() {
    let inv = inventory();
    let cart = surface(Kind::Cart);
    let described = names(inv.iter().filter(|b| b.scope == Scope::Method));
    assert_eq!(cart.methods, described);
    assert!(cart.methods.contains("__tostring"));
}

#[test]
fn sys_is_shell_only_and_exactly_described() {
    let inv = inventory();
    let shell = surface(Kind::Shell);
    let described = names(inv.iter().filter(|b| b.scope == Scope::Sys));
    assert_eq!(shell.sys, described);
    // The shell's globals are the cart's plus `sys`; nothing described
    // as shell-only leaks into a cart global.
    let cart = surface(Kind::Cart);
    let mut expected = cart.globals.clone();
    expected.insert("sys".to_string());
    assert_eq!(shell.globals, expected);
    assert!(shell.net.is_empty(), "the shell has no net table");
    for b in &inv {
        assert_eq!(
            b.scope == Scope::Sys,
            b.group == Group::Shell,
            "{} mixes shell scope and group",
            b.qualified()
        );
    }
}

#[test]
fn net_is_for_declaring_carts_only_and_exactly_described() {
    let inv = inventory();
    let net = surface(Kind::Net);
    let described = names(inv.iter().filter(|b| b.scope == Scope::Net));
    assert!(!described.is_empty());
    assert_eq!(net.net, described);
    let cart = surface(Kind::Cart);
    let mut expected = cart.globals.clone();
    expected.insert("net".to_string());
    assert_eq!(net.globals, expected);
    assert!(net.sys.is_empty());
    for b in &inv {
        assert_eq!(
            b.scope == Scope::Net,
            b.group == Group::Net,
            "{} mixes net scope and group",
            b.qualified()
        );
    }
}

#[test]
fn validate_refuses_missing_metadata_and_internal_text() {
    const OK_SIG: &[Sig] = &[Sig::new("x()", "nothing")];
    const EMPTY_SIG: &[Sig] = &[Sig::new("x()", "")];
    let ok = Binding {
        name: "x",
        sigs: OK_SIG,
        ..Binding::EMPTY
    };
    assert_eq!(ok.validate(), Ok(()));
    assert!(Binding::EMPTY.validate().unwrap_err().contains("no name"));
    let no_sig = Binding {
        name: "x",
        ..Binding::EMPTY
    };
    assert!(no_sig.validate().unwrap_err().contains("no signature"));
    let empty_sig = Binding {
        sigs: EMPTY_SIG,
        ..ok
    };
    assert!(empty_sig
        .validate()
        .unwrap_err()
        .contains("empty signature"));
    // Spelled apart so the public export keeps these lines.
    let internal = Binding {
        doc: concat!("public text [inter", "nal] a note"),
        ..ok
    };
    assert!(internal
        .validate()
        .unwrap_err()
        .contains(crate::api::INTERNAL_MARKER));
    let multiline = Binding {
        doc: "two\nlines",
        ..ok
    };
    assert!(multiline.validate().unwrap_err().contains("line break"));
}

#[test]
fn problems_reports_duplicates_within_a_scope_only() {
    static A: Binding = Binding {
        name: "dup",
        sigs: &[Sig::new("dup()", "nothing")],
        ..Binding::EMPTY
    };
    static B: Binding = Binding {
        scope: Scope::Method,
        ..A
    };
    assert_eq!(problems(&[&A, &B]), Vec::<String>::new());
    let found = problems(&[&A, &A]);
    assert_eq!(found, vec!["dup is declared twice".to_string()]);
}

#[test]
fn inventory_order_is_stable_and_prices_render() {
    let a = inventory();
    let b = inventory();
    assert_eq!(a, b);
    for binding in &a {
        let _ = binding.price.render();
        for sig in binding.sigs {
            if let Some(p) = sig.price {
                let _ = p.render();
            }
        }
        if binding.price == Price::Value {
            assert_eq!(binding.sigs.len(), 1, "{}", binding.qualified());
        }
    }
}

/// The documented defaults and overloads, through the meter and the
/// screen, for the forms no other test exercises.
#[test]
fn documented_defaults_and_overloads_hold() {
    let src = "\
function _init()
  pset(0, 0)
  line(1, 0, 3, 0)
  rect(0, 2, 2, 4)
  rectfill(4, 2, 6, 4)
  circ(20, 20, 2)
  circfill(30, 20, 2)
  print('A', 40, 0)
  camera(5, 5)
  pset(5, 10, 3)
  camera()
  pset(1, 10, 4)
  fillp(0xffff)
  rectfill(50, 0, 51, 1, 0x0208)
  fillp()
  rectfill(52, 0, 53, 1, 0x0208)
  clip(60, 0, 2, 2)
  rectfill(0, 0, 100, 10, 5)
  clip()
  pset(70, 0, 6)
  print(select('#', pcall(clip, 1, 2)))
  print(select(2, pcall(pal, 5, 0xff0000)))
  print(pcall(pal, 20, 1, 2))
  palt(9)
  pal_map(1, 2)
  pal_reset()
  print(stat('width') .. 'x' .. stat('height'))
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let px = |x: i32, y: i32| super::pixel(&c, x, y);
    assert_eq!(px(0, 0), 7, "pset colour defaults to 7");
    assert_eq!(px(2, 0), 7, "line default colour");
    assert_eq!(px(0, 2), 7, "rect default colour");
    assert_eq!(px(1, 3), 0, "rect is an outline");
    assert_eq!(px(5, 3), 7, "rectfill default colour");
    assert_eq!(px(22, 20), 7, "circ default colour");
    assert_eq!(px(30, 20), 7, "circfill default colour");
    assert_eq!(px(0, 5), 3, "camera(5, 5) shifted the pixel");
    assert_eq!(px(1, 10), 4, "camera() reset the offset");
    assert_eq!(px(50, 0), 2, "fillp selects the secondary colour");
    assert_eq!(px(52, 0), 8, "fillp() clears the pattern: primary colour");
    assert_eq!(px(61, 1), 5, "inside the clip");
    assert_eq!(px(63, 0), 0, "outside the clip");
    assert_eq!(px(70, 0), 6, "clip() reset");
    let log = c.output().log.to_vec();
    assert_eq!(log[0], "2", "clip with a wrong arity is a Lua error");
    assert!(log[1].contains("palette_index_locked"), "{}", log[1]);
    assert!(
        log[2].starts_with("false") && log[2].contains("pal takes"),
        "{}",
        log[2]
    );
    assert_eq!(log[3], "640x480");
}

/// `music` has three forms and `sfx`'s channel defaults to a free one.
#[test]
fn music_overloads_and_sfx_default_channel() {
    let src = "\
function _init()
  print(pcall(music))
  print(pcall(music, nil, 3))
  print(select(2, pcall(sfx, 'missing')))
end
";
    let mut c = console(src);
    run(&mut c, 1);
    assert_eq!(c.state().fault(), None, "{:?}", c.state());
    let log = c.output().log.to_vec();
    assert_eq!(log[0], "true");
    assert_eq!(log[1], "true");
    assert!(log[2].contains("asset_not_found"), "{}", log[2]);
}
