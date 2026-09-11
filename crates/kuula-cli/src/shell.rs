//! `kuula shell`: boot into the shell guest from `rom/main.lua`, list
//! the carts under a directory and let the shell run them. The shell is
//! embedded so the binary is self-contained. The shell stays in this
//! process; the carts it starts run in a worker unless `--in-process`.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use kuula_core::console::CartOpener;
use kuula_core::net::NetEnv;
use kuula_core::shell::{CartEntry, Settings};
use kuula_core::{Console, Fault, Manifest, Snapshot, SnapshotLimits};
use kuula_host_sdl::HostOptions;
use kuula_lua::LuaGuest;

use crate::remote::RemoteGuest;
use crate::{settings, CartRunner, EXIT_FAULT, EXIT_OK};

/// The shell's source, checked in under `rom/`.
pub const ROM_MAIN: &str = include_str!("../../../rom/main.lua");

/// Most carts the list shows.
const MAX_CARTS: usize = 256;

/// A listed cart and where it lives.
struct Listed {
    entry: CartEntry,
    path: PathBuf,
}

/// Directories with a `main.lua` and `.zip`/`.cart` files directly under
/// `dir`, sorted by name. Titles come from `cart.toml` when it parses.
fn list_carts(dir: &Path) -> Vec<Listed> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if out.len() >= MAX_CARTS {
            break;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let is_dir_cart = path.is_dir() && path.join(kuula_core::console::MAIN_FILE).is_file();
        if !is_dir_cart && !crate::is_archive(&path) {
            continue;
        }
        let title = if path.is_dir() {
            std::fs::read_to_string(path.join(kuula_core::manifest::MANIFEST_FILE))
                .ok()
                .and_then(|t| Manifest::parse(&t).ok())
                .map(|m| m.title)
                .filter(|t| !t.is_empty())
        } else {
            None
        };
        out.push(Listed {
            entry: CartEntry {
                name: name.to_string(),
                title: title.unwrap_or_else(|| name.to_string()),
            },
            path,
        });
    }
    out
}

fn open_cart(path: &Path) -> Result<Snapshot, Fault> {
    let limits = SnapshotLimits::default();
    let snap = if crate::is_archive(path) {
        Snapshot::from_zip(path, limits)
    } else {
        Snapshot::from_dir(path, limits)
    };
    snap.map_err(|e| Fault::new(Fault::CART_READ_ERROR, &e.path, None, e.message))
}

pub fn run(carts: Option<&Path>, scale: Option<u32>, runner: CartRunner) -> u8 {
    let carts_dir: PathBuf = match carts {
        Some(p) => p.to_path_buf(),
        None if Path::new("carts").is_dir() => PathBuf::from("carts"),
        None => PathBuf::from("examples"),
    };
    let listed: Rc<Vec<Listed>> = Rc::new(list_carts(&carts_dir));
    let entries: Vec<CartEntry> = listed.iter().map(|l| l.entry.clone()).collect();

    let opener: CartOpener = {
        let listed = listed.clone();
        Rc::new(move |name: &str| {
            let cart = listed
                .iter()
                .find(|l| l.entry.name == name)
                .ok_or_else(|| Fault::new(Fault::CART_READ_ERROR, name, None, "no such cart"))?;
            let snap = open_cart(&cart.path)?;
            // Saves are keyed by the cart's path, assigned here and never
            // by the manifest.
            let store = crate::desktop_save_store(&cart.path);
            Ok((Rc::new(snap) as Rc<dyn kuula_core::CartSource>, store))
        })
    };
    let factory: kuula_core::console::GuestFactory = match runner.remote(None) {
        Some(config) => Rc::new(RemoteGuest::factory(config)),
        None => Rc::new(LuaGuest::factory),
    };
    let preload = runner.preload();
    // The persistent settings; `--scale` on the command line wins for
    // this run.
    let stored = settings::load(Settings::default());
    let initial = Settings {
        scale: scale.unwrap_or(stored.scale),
        ..stored
    };
    let scale = initial.scale;
    // Carts the shell starts get the persistent permission and no
    // invite; the link is permitted the same way and flipped by the
    // settings screen.
    let mut link = crate::iroh_link();
    link.set_permitted(initial.net);
    // The settings screen flips the link; a console rebuilt after a
    // shell fault must start from what the link holds now, not from
    // the boot-time value, or the two disagree on permission.
    let permitted = Rc::new(Cell::new(initial.net));
    let permitted_now = permitted.clone();

    let mut make = || match LuaGuest::new_shell(ROM_MAIN, "rom/main.lua") {
        Ok(shell) => {
            let net = permitted_now.get();
            let mut c = Console::with_shell(
                Box::new(shell),
                entries.clone(),
                Settings { net, ..initial },
                opener.clone(),
                factory.clone(),
                preload,
            );
            c.set_net_env(NetEnv {
                permitted: net,
                invite: None,
            });
            c
        }
        Err(fault) => Console::faulted(fault),
    };
    let opts = HostOptions {
        link: Some(link),
        on_settings: Some(Box::new(move |s| {
            permitted.set(s.net);
            settings::save(&s)
        })),
        ..HostOptions::new(scale, "Kuula")
    };
    match kuula_host_sdl::run(&mut make, opts) {
        Err(e) => {
            eprintln!("host error: {e}");
            EXIT_FAULT
        }
        Ok(_) => EXIT_OK,
    }
}
