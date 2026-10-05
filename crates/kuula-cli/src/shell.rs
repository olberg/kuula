//! `kuula shell`: boot into the shell guest from `rom/main.lua`, list
//! the carts under a directory and let the shell run them. The shell is
//! embedded so the binary is self-contained. The shell stays in this
//! process; the carts it starts run in a worker unless `--in-process`.

#[cfg(feature = "net")]
mod dev_receiver;
#[cfg(feature = "net")]
mod network;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use kuula_core::console::CartOpener;
use kuula_core::net::NetEnv;
use kuula_core::shell::{CartEntry, Settings};
use kuula_core::{Console, Fault, Snapshot};
use kuula_host_common::carts::{list_dir as list_carts, listed, open_cart, Listed};
use kuula_host_sdl::HostOptions;
use kuula_lua::LuaGuest;

use crate::remote::RemoteGuest;
use crate::{settings, CartRunner, EXIT_FAULT, EXIT_OK, EXIT_USAGE};

/// The shell's source, checked in under `rom/`.
pub const ROM_MAIN: &str = include_str!("../../../rom/main.lua");

/// `--dev-receiver`: the shell also receives carts pushed by approved
/// developers.
pub struct DevReceiver {
    /// Bind this address only (default: every interface). Without the
    /// `net` feature the flag is refused before a shell starts.
    #[cfg_attr(not(feature = "net"), allow(dead_code))]
    pub bind: Option<std::net::SocketAddr>,
}

/// What the shell lists.
pub enum Carts<'a> {
    /// The carts of a directory (default: carts/, or examples/ when
    /// carts/ is missing).
    Listed(Option<&'a Path>),
    /// `--cart`: this cart alone, opened at once; quitting it ends the
    /// shell.
    One(&'a Path),
}

/// The link the shell steps carts through and the service behind its
/// network screens: Iroh, relay settings and LAN discovery with the
/// `net` feature; an offline link and no service without it.
#[cfg(feature = "net")]
fn net_services(
    listed: &Rc<Vec<Listed>>,
    identity: &Rc<RefCell<kuula_core::net::identity::Identity>>,
) -> (kuula_core::net::Link, Option<kuula_host_sdl::ShellService>) {
    let config = Rc::new(RefCell::new(network::Config::load()));
    let diagnostics = Rc::new(RefCell::new(std::sync::Arc::new(std::sync::Mutex::new(
        String::new(),
    ))));
    let service = network::service(
        listed.clone(),
        config.clone(),
        diagnostics.clone(),
        identity.clone(),
    );
    let identity = identity.clone();
    let link = crate::netlink::net_link(
        move || config.borrow().relay.clone(),
        move || identity.borrow().clone(),
        move || {
            let diag = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            *diagnostics.borrow_mut() = diag.clone();
            diag
        },
    );
    (link, Some(service))
}

#[cfg(not(feature = "net"))]
fn net_services(
    _: &Rc<Vec<Listed>>,
    _: &Rc<RefCell<kuula_core::net::identity::Identity>>,
) -> (kuula_core::net::Link, Option<kuula_host_sdl::ShellService>) {
    let link = crate::netlink::net_link(
        Default::default,
        || kuula_core::net::identity::Identity::new(&Snapshot::empty()),
        Default::default,
    );
    // The multiplayer screen shows why it has nothing to offer.
    let service: kuula_host_sdl::ShellService = Box::new(|console, _| {
        if let (Some(view), Some(why)) = (console.network_view_mut(), crate::netlink::unavailable())
        {
            if view.unavailable.is_empty() {
                view.unavailable = why.into();
            }
        }
    });
    (link, Some(service))
}

pub fn run(
    carts: Carts<'_>,
    scale: Option<u32>,
    runner: CartRunner,
    dev: Option<DevReceiver>,
) -> u8 {
    let single = matches!(carts, Carts::One(_));
    // The directory a development receiver installs into; with one cart
    // there is no receiver, and it names nothing.
    let (carts_dir, listed) = match carts {
        Carts::One(path) => match listed(path.to_path_buf()) {
            Some(one) => (PathBuf::new(), vec![one]),
            None => {
                eprintln!(
                    "error: {} is not a cart (a directory with main.lua, or a .zip or .cart file)",
                    path.display()
                );
                return EXIT_USAGE;
            }
        },
        Carts::Listed(dir) => {
            let dir: PathBuf = match dir {
                Some(p) => p.to_path_buf(),
                None if Path::new("carts").is_dir() => PathBuf::from("carts"),
                None => PathBuf::from("examples"),
            };
            let listed = list_carts(&dir);
            (dir, listed)
        }
    };
    let listed: Rc<Vec<Listed>> = Rc::new(listed);
    let entries: Vec<CartEntry> = listed.iter().map(|l| l.entry.clone()).collect();

    let identity = Rc::new(RefCell::new(kuula_core::net::identity::Identity::new(
        &Snapshot::empty(),
    )));
    let opener: CartOpener = {
        let identity = identity.clone();
        let listed = listed.clone();
        Rc::new(move |name: &str| {
            let cart = listed
                .iter()
                .find(|l| l.entry.name == name)
                .ok_or_else(|| Fault::new(Fault::CART_READ_ERROR, name, None, "no such cart"))?;
            let snap = open_cart(&cart.path)?;
            *identity.borrow_mut() = kuula_core::net::identity::Identity::new(&snap);
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
    let (mut link, service) = net_services(&listed, &identity);
    link.set_permitted(initial.net);
    // The development receiver is separate from the network permission:
    // it exists only because the person started the shell with the flag.
    #[cfg(feature = "net")]
    let service = match dev {
        Some(dev) => {
            let store = match crate::deploy_cmd::store() {
                Ok(store) => store,
                Err(why) => {
                    eprintln!("error: deploy_install {why}");
                    return EXIT_FAULT;
                }
            };
            match dev_receiver::start(&carts_dir, dev.bind, store) {
                Ok(started) => Some(dev_receiver::service(
                    service,
                    started,
                    identity.clone(),
                    crate::desktop_save_store,
                )),
                Err(code) => return code,
            }
        }
        None => service,
    };
    #[cfg(not(feature = "net"))]
    let _ = (dev, &carts_dir);
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
            c.set_single_cart(single);
            c
        }
        Err(fault) => Console::faulted(fault),
    };
    let opts = HostOptions {
        link: Some(link),
        shell_service: service,
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
