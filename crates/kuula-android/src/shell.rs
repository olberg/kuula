//! The console the app runs: the shell guest from `rom/main.lua`, with the
//! listed carts, an opener that reads a cart and its saves, and no network.
//! Built the way the desktop CLI's `shell.rs` builds it without its `net`
//! feature, and in this process: carts run in-process, with no worker.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use kuula_core::console::{CartOpener, GuestFactory};
use kuula_core::net::{Link, NetEnv};
use kuula_core::shell::{CartEntry, Settings};
use kuula_core::{Console, Fault, Preload};
use kuula_lua::LuaGuest;

use crate::carts::{self, Listed};
use crate::saves;

/// The shell's source, checked in under `rom/`.
pub const ROM_MAIN: &str = include_str!("../../../rom/main.lua");

/// What the network screen says in place of a connection.
pub const NO_NETWORK: &str = "this build has no networking";

/// A cart opened for a console: its files and its saves.
pub type Opened = (
    Rc<dyn kuula_core::CartSource>,
    Box<dyn kuula_core::SaveStore>,
);

/// The shell, ready to make consoles, and the link they step through.
pub struct Shell {
    /// Offline: a cart that asks to host or join is told so.
    pub link: Link,
    entries: Vec<CartEntry>,
    opener: CartOpener,
    factory: GuestFactory,
    settings: Settings,
    permitted: Rc<Cell<bool>>,
}

impl Shell {
    /// A shell listing `listed`, whose carts keep their saves under
    /// `saves_root`, starting from `settings`.
    pub fn new(listed: Vec<Listed>, saves_root: PathBuf, settings: Settings) -> Shell {
        let entries: Vec<CartEntry> = listed.iter().map(|l| l.entry.clone()).collect();
        let listed = Rc::new(listed);
        let opener: CartOpener = Rc::new(move |name: &str| {
            let cart = listed
                .iter()
                .find(|l| l.entry.name == name)
                .ok_or_else(|| Fault::new(Fault::CART_READ_ERROR, name, None, "no such cart"))?;
            let snap = carts::open_cart(&cart.path)?;
            // Saves are keyed by the cart's path, assigned here and never
            // by the manifest.
            let store = saves::store(&saves_root, &cart.path);
            Ok((Rc::new(snap) as Rc<dyn kuula_core::CartSource>, store))
        });
        let mut link = Link::offline();
        link.set_permitted(settings.net);
        Shell {
            link,
            entries,
            opener,
            factory: Rc::new(LuaGuest::factory),
            settings,
            permitted: Rc::new(Cell::new(settings.net)),
        }
    }

    /// A fresh console: at the start, and again when a fault is answered
    /// with a restart. A shell that does not load gives a faulted console,
    /// which the host shows as an error screen.
    pub fn make(&self) -> Console {
        match LuaGuest::new_shell(ROM_MAIN, "rom/main.lua") {
            Ok(guest) => {
                let net = self.permitted.get();
                let mut console = Console::with_shell(
                    Box::new(guest),
                    self.entries.clone(),
                    Settings {
                        net,
                        ..self.settings
                    },
                    self.opener.clone(),
                    self.factory.clone(),
                    Preload::Decode,
                );
                console.set_net_env(NetEnv {
                    permitted: net,
                    invite: None,
                });
                console.set_single_cart(false);
                console
            }
            Err(fault) => Console::faulted(fault),
        }
    }

    /// The listed cart `name` and its saves, as the shell's own `sys.run`
    /// opens it: for a cart the host starts itself.
    pub fn open(&self, name: &str) -> Result<Opened, Fault> {
        (self.opener)(name)
    }

    /// The settings screen changed something: a console made from now on,
    /// and the link, start from it.
    pub fn settings_changed(&mut self, settings: Settings) {
        self.settings = settings;
        self.permitted.set(settings.net);
        self.link.set_permitted(settings.net);
    }
}

/// Tell the network screen why it has nothing to offer. Called after a
/// step: the view is empty until the shell has drawn it once.
pub fn say_no_network(console: &mut Console) {
    if let Some(view) = console.network_view_mut() {
        if view.unavailable.is_empty() {
            view.unavailable = NO_NETWORK.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuula_core::input::BTN_MENU;
    use kuula_core::FrameInput;

    fn examples() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples")
    }

    #[test]
    fn the_shell_lists_the_carts_and_steps_without_faulting() {
        let none = crate::testdir::new("shell-none");
        let listed = carts::list(&none, &examples());
        assert!(
            listed.iter().any(|l| l.entry.name == "hello"),
            "{:?}",
            listed.iter().map(|l| &l.entry.name).collect::<Vec<_>>()
        );
        let count = listed.len();
        let shell = Shell::new(listed, none.join("saves"), Settings::default());
        let mut console = shell.make();
        assert!(console.has_shell());
        for frame in 0..30 {
            // Menu now and then, as a person would.
            let buttons = if frame % 10 == 5 { BTN_MENU } else { 0 };
            console.step(FrameInput::new(buttons));
        }
        assert!(
            console.shell_fault().is_none(),
            "{:?}",
            console.shell_fault()
        );
        assert!(console.state().fault().is_none());
        assert!(count > 1);
    }

    #[test]
    fn the_network_screen_says_the_build_has_no_networking() {
        let none = crate::testdir::new("shell-net");
        let shell = Shell::new(Vec::new(), none.join("saves"), Settings::default());
        let mut console = shell.make();
        console.step(FrameInput::new(0));
        say_no_network(&mut console);
        let view = console
            .network_view()
            .expect("the shell has a network view");
        assert_eq!(view.unavailable, NO_NETWORK);
    }

    #[test]
    fn a_settings_change_reaches_the_next_console_and_the_link() {
        let none = crate::testdir::new("shell-settings");
        let mut shell = Shell::new(Vec::new(), none.join("saves"), Settings::default());
        let changed = Settings {
            volume: 30,
            ..Settings::default()
        };
        shell.settings_changed(changed);
        assert_eq!(shell.make().settings().volume, 30);
    }
}
