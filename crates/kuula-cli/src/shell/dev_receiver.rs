//! `kuula shell --dev-receiver`: the shell as a deploy receiver. The
//! receiver runs on its own thread inside `kuula-net`; an install it
//! makes is handed here, to the main thread that owns the console, which
//! opens the installed file and starts it through the console's
//! `host_load_cart` (the normal guest teardown and start, under the same
//! runner as any cart the shell starts, with the save store keyed by the
//! installed path). The answer goes back for the result frame. The cart
//! list is built at start, so a cart installed for the first time shows
//! in it from the next shell start.
//!
//! The shell's Lua keeps its own screen (list, pause, error, ...) and
//! only its cart screen is transparent. `host_load_cart` tells it that
//! the host started a cart, and `rom/main.lua` moves to its cart screen
//! on that, so the deployed cart is seen whatever the shell was showing:
//! its boot screen, its list, a pause menu or another cart's error.
//!
//! Progress goes to stderr; ticket and id are printed when the receiver
//! starts. Nothing here is reachable from a cart.

use std::cell::RefCell;
use std::net::SocketAddr;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;

use kuula_core::net::identity::Identity;
use kuula_core::{Console, ConsoleState, SaveStore};
use kuula_host_sdl::ShellService;
use kuula_net::deploy::{DeployReceiver, DeployStore, ReceiverConfig, Restart, RestartRequest};
use kuula_net::NetConfig;

use crate::EXIT_FAULT;

/// How the save store for an installed cart is made from its path.
pub type SaveStoreFor = fn(&Path) -> Box<dyn SaveStore>;

/// A started receiver and the installs it hands over.
pub struct Started {
    receiver: DeployReceiver,
    requests: mpsc::Receiver<RestartRequest>,
}

/// Start a receiver for `carts`, print how to reach it, and return it.
pub fn start(carts: &Path, bind: Option<SocketAddr>, store: DeployStore) -> Result<Started, u8> {
    let (restart, requests) = mpsc::channel();
    let receiver = DeployReceiver::start(ReceiverConfig {
        carts: carts.to_path_buf(),
        store,
        net: NetConfig {
            enabled: true,
            bind,
            ..Default::default()
        },
        restart: Some(restart),
    })
    .map_err(|e| {
        eprintln!("error: {e}");
        EXIT_FAULT
    })?;
    eprintln!("dev receiver: ticket: {}", receiver.ticket());
    eprintln!("dev receiver: id: {}", receiver.id());
    eprintln!("dev receiver: carts: {}", carts.display());
    Ok(Started { receiver, requests })
}

/// Load an installed cart into the console and report how it went.
fn reload(
    console: &mut Console,
    request: RestartRequest,
    identity: &RefCell<Identity>,
    save_store: SaveStoreFor,
) {
    // A shell that faulted has no console to load into until the host
    // rebuilds it.
    if !console.has_shell() {
        request
            .reply
            .send(Restart::NotRun, "the shell is not running");
        return;
    }
    let snap = match super::open_cart(&request.path) {
        Ok(snap) => snap,
        Err(fault) => {
            request.reply.send(Restart::Faulted, fault.code.to_string());
            return;
        }
    };
    *identity.borrow_mut() = Identity::new(&snap);
    console.host_load_cart(Rc::new(snap), save_store(&request.path));
    match console.state() {
        ConsoleState::Running => request.reply.send(Restart::Started, ""),
        ConsoleState::Faulted(fault) => {
            request.reply.send(Restart::Faulted, fault.code.to_string())
        }
    }
    eprintln!("dev receiver: reloaded {}", request.name);
}

/// The shell service with the receiver's work added: events are printed,
/// installs are loaded. `inner` is the service the shell already had.
pub fn service(
    mut inner: Option<ShellService>,
    started: Started,
    identity: Rc<RefCell<Identity>>,
    save_store: SaveStoreFor,
) -> ShellService {
    Box::new(move |console, requests| {
        if let Some(inner) = inner.as_mut() {
            inner(console, requests);
        }
        while let Some(event) = started.receiver.try_event() {
            eprintln!("dev receiver: {}", crate::deploy_cmd::describe(&event));
        }
        while let Ok(request) = started.requests.try_recv() {
            reload(console, request, &identity, save_store);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use kuula_core::input::BTN_A;
    use kuula_core::shell::Settings;
    use kuula_core::{FrameInput, MemoryStore, Preload, Snapshot, SnapshotLimits};
    use kuula_lua::LuaGuest;
    use kuula_net::deploy::{push, DeployCode, Package, PushReport};

    /// A shell listing one cart, `first`, which draws colour 2.
    fn shell_console() -> Console {
        let opener: kuula_core::console::CartOpener = Rc::new(|_: &str| {
            let snap = Snapshot::from_entries(
                [("main.lua", b"function _draw() cls(2) end".to_vec())],
                SnapshotLimits::default(),
            )
            .unwrap();
            Ok((
                Rc::new(snap) as Rc<dyn kuula_core::CartSource>,
                Box::new(MemoryStore::new()) as Box<dyn SaveStore>,
            ))
        });
        let entry = kuula_core::shell::CartEntry {
            name: "first".into(),
            title: "FIRST".into(),
            ..Default::default()
        };
        Console::with_shell(
            Box::new(LuaGuest::new_shell(super::super::ROM_MAIN, "rom/main.lua").unwrap()),
            vec![entry],
            Settings::default(),
            opener,
            Rc::new(LuaGuest::factory),
            Preload::Decode,
        )
    }

    fn package(main: &str) -> Package {
        let snap = Snapshot::from_entries(
            [("main.lua", main.as_bytes().to_vec())],
            SnapshotLimits::default(),
        )
        .unwrap();
        Package::from_snapshot("dev", &snap).unwrap()
    }

    fn memory_store(_: &Path) -> Box<dyn SaveStore> {
        Box::new(MemoryStore::new())
    }

    /// Push from a thread while this one plays the console's part: the
    /// shell service runs once per frame, as the windowed host runs it.
    fn push_and_serve(
        console: &mut Console,
        service: &mut ShellService,
        ticket: &str,
        secret: kuula_net::deploy::SecretKey,
        package: Package,
    ) -> PushReport {
        let ticket = ticket.to_string();
        let net = NetConfig {
            enabled: true,
            bind: Some("127.0.0.1:0".parse().unwrap()),
            ..Default::default()
        };
        let sender = std::thread::spawn(move || push(&net, secret, &ticket, &package).unwrap());
        let until = Instant::now() + Duration::from_secs(20);
        while !sender.is_finished() {
            assert!(Instant::now() < until, "the push never finished");
            console.step(FrameInput::NONE);
            service(console, &[]);
            std::thread::sleep(Duration::from_millis(5));
        }
        sender.join().unwrap()
    }

    #[test]
    fn an_install_starts_from_the_list_and_from_the_pause_menu() {
        let root = std::env::temp_dir().join(format!("kuula-dev-idle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (rx_store, tx_store) = (
            DeployStore::new(&root.join("rx")),
            DeployStore::new(&root.join("tx")),
        );
        rx_store.approve(&tx_store.id().unwrap()).unwrap();
        let carts = root.join("carts");
        let started = start(&carts, Some("127.0.0.1:0".parse().unwrap()), rx_store).unwrap();
        let ticket = started.receiver.ticket().to_string();
        let identity = Rc::new(RefCell::new(Identity::new(&Snapshot::empty())));
        let mut service = service(None, started, identity, memory_store);
        // Still on the boot screen, then on the list.
        let mut console = shell_console();
        for bits in [0, 0, BTN_A, 0] {
            console.step(FrameInput::new(bits));
        }
        assert_eq!(console.frame(), 0, "no cart runs yet");
        let secret = || tx_store.secret_key().unwrap();
        let shows = |console: &mut Console, colour: u8| {
            for _ in 0..10 {
                console.step(FrameInput::NONE);
            }
            console.output().screen.iter().all(|&p| p == colour)
        };

        // From the list: the cart is installed, started, and the list
        // steps aside for it.
        let five = "function _draw() cls(5) end\n";
        let report = push_and_serve(&mut console, &mut service, &ticket, secret(), package(five));
        assert_eq!(report.code, DeployCode::Ok, "{report:?}");
        assert_eq!(report.restart, Restart::Started, "{report:?}");
        assert!(carts.join("dev.cart").is_file());
        assert!(shows(&mut console, 5), "the cart shows over the list");

        // From the pause menu: the menu closes and the new version runs.
        console.step(FrameInput::new(kuula_core::input::BTN_MENU));
        console.step(FrameInput::NONE);
        assert!(console.is_paused(), "the menu is open");
        let nine = "function _draw() cls(9) end\n";
        let report = push_and_serve(&mut console, &mut service, &ticket, secret(), package(nine));
        assert_eq!(report.restart, Restart::Started, "{report:?}");
        assert!(!console.is_paused());
        assert!(shows(&mut console, 9), "the new version shows, unpaused");
        drop(service);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_install_reloads_the_running_cart_and_reports_started_or_faulted() {
        let root = std::env::temp_dir().join(format!("kuula-dev-recv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (rx_store, tx_store) = (
            DeployStore::new(&root.join("rx")),
            DeployStore::new(&root.join("tx")),
        );
        rx_store.approve(&tx_store.id().unwrap()).unwrap();
        let carts = root.join("carts");
        let started = start(&carts, Some("127.0.0.1:0".parse().unwrap()), rx_store).unwrap();
        let ticket = started.receiver.ticket().to_string();
        let identity = Rc::new(RefCell::new(Identity::new(&Snapshot::empty())));
        let mut service = service(None, started, identity.clone(), memory_store);
        let mut console = shell_console();
        // Start the first cart from the list, as a person developing it
        // would: the shell is then on its transparent cart screen.
        for bits in [0, 0, BTN_A, 0, BTN_A, 0] {
            console.step(FrameInput::new(bits));
        }
        assert!(console.frame() >= 1, "the first cart runs");
        assert!(
            console.shell_fault().is_none(),
            "{:?}",
            console.shell_fault()
        );
        let secret = || tx_store.secret_key().unwrap();

        // A cart that runs: installed, loaded, started, and the console
        // is now stepping it from its first frame.
        let ok = "function _update(dt) end\nfunction _draw() cls(5) end\n";
        let report = push_and_serve(&mut console, &mut service, &ticket, secret(), package(ok));
        assert_eq!(report.code, DeployCode::Ok, "{report:?}");
        assert_eq!(report.restart, Restart::Started, "{report:?}");
        assert!(carts.join("dev.cart").is_file());
        assert!(console.state().fault().is_none());
        assert!(
            console.frame() < 20,
            "a fresh cart: frame {}",
            console.frame()
        );
        // The shell's list steps aside for the running cart within a few
        // frames and the cart's own screen shows through.
        for _ in 0..10 {
            console.step(FrameInput::NONE);
        }
        assert!(
            console.output().screen.iter().all(|&p| p == 5),
            "the new cart draws"
        );
        assert_ne!(
            *identity.borrow(),
            Identity::new(&Snapshot::empty()),
            "the shell's net identity follows the loaded cart"
        );

        // A cart that cannot start is installed all the same, and the
        // result says it faulted, with the fault's code.
        let report = push_and_serve(
            &mut console,
            &mut service,
            &ticket,
            secret(),
            package("this is not lua"),
        );
        assert_eq!(report.code, DeployCode::Ok, "{report:?}");
        assert_eq!(report.restart, Restart::Faulted, "{report:?}");
        assert!(!report.detail.is_empty(), "the fault code travels back");
        assert!(console.state().fault().is_some());
        for _ in 0..5 {
            console.step(FrameInput::NONE);
        }

        // From the error screen a fix replaces it and the shell steps
        // aside, which only the shell's own A press would do by hand.
        let report = push_and_serve(&mut console, &mut service, &ticket, secret(), package(ok));
        assert_eq!(report.restart, Restart::Started, "{report:?}");
        assert!(console.state().fault().is_none());
        for _ in 0..10 {
            console.step(FrameInput::NONE);
        }
        assert!(
            console.output().screen.iter().all(|&p| p == 5),
            "the error screen is gone and the fix draws"
        );
        drop(service);
        let _ = std::fs::remove_dir_all(&root);
    }
}
