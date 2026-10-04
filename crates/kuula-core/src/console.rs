use std::rc::Rc;

use crate::audio::VALUES_PER_FRAME;
use crate::draw::DrawState;
use crate::fault::Fault;
use crate::input::{FrameInput, BTN_MENU};
use crate::manifest::{Manifest, ManifestError, ScreenMode, MANIFEST_FILE};
use crate::meter::FrameProfile;
use crate::net::{self, NetEnv, NetState};
use crate::palette::PALETTE_SIZE;
use crate::save::SaveStore;
use crate::shell::{self, CartEntry, Settings, SysRequest, SysState};
use crate::snapshot::Snapshot;
use crate::source::CartSource;

/// Name of the cart's entry file.
pub const MAIN_FILE: &str = "main.lua";

/// A cart runtime. `kuula-core` defines the contract; `kuula-lua`
/// implements it with mlua. A stub implementation is enough to test the
/// console.
pub trait Guest {
    /// Run one frame. `frame` is 1 on the first call. The implementation
    /// decides what a frame means (for Lua: chunk and `_init` on frame 1,
    /// `_update` then `_draw` afterwards). Returning `Err` ends the cart.
    fn step(&mut self, state: &mut DrawState, input: FrameInput, frame: u64) -> Result<(), Fault>;

    /// Dump the named globals as canonical codec text,
    /// for the harness `state` command. A guest without a codec refuses.
    fn state(&mut self, _names: &[String]) -> Result<String, Fault> {
        Err(Fault::new(
            "unsupported",
            "",
            None,
            "this guest cannot dump state",
        ))
    }
}

/// Builds a guest from source text and a chunk name.
pub trait FactoryFn: Fn(&str, &str) -> Result<Box<dyn Guest>, Fault> {}
impl<T: Fn(&str, &str) -> Result<Box<dyn Guest>, Fault>> FactoryFn for T {}

/// A shared [`FactoryFn`], kept by a console with a shell so it can
/// rebuild carts.
pub type GuestFactory = Rc<dyn FactoryFn>;

/// Whether loading a cart decodes its preload sheets and maps into the
/// draw state. A guest that runs in this process needs them; a guest in
/// a worker process decodes the cart itself, so the broker skips the
/// work and keeps untrusted asset parsing behind the sandbox
///.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preload {
    Decode,
    Skip,
}

/// Resolves a cart name from the shell's list to a source and the save
/// store that cart gets (the host assigns the identity).
pub type CartOpener = Rc<dyn Fn(&str) -> Result<(Rc<dyn CartSource>, Box<dyn SaveStore>), Fault>>;

/// Whether the cart is still running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleState {
    Running,
    Faulted(Fault),
}

impl ConsoleState {
    pub fn fault(&self) -> Option<&Fault> {
        match self {
            ConsoleState::Running => None,
            ConsoleState::Faulted(f) => Some(f),
        }
    }
}

/// What a host presents after a step. Borrows the console.
#[derive(Debug, Clone, Copy)]
pub struct FrameOutput<'a> {
    /// The presentation buffer: the cart's screen with the shell's
    /// overlay composed over it when a shell is present, otherwise the
    /// cart's screen itself.
    pub screen: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub palette: &'a [[u8; 3]; PALETTE_SIZE],
    /// Number of steps that have run; 0 before the first.
    pub frame: u64,
    pub log: &'a [String],
    /// What the step cost, by category.
    pub profile: &'a FrameProfile,
    /// This frame's PCM, `audio::VALUES_PER_FRAME` values: 735 sample
    /// frames of 44.1 kHz stereo 16-bit, the left value first. Silence
    /// after a fault and while paused.
    pub audio: &'a [i16],
}

/// The cart's half of the console.
struct Cart {
    guest: Option<Box<dyn Guest>>,
    state: DrawState,
    frame: u64,
    status: ConsoleState,
    manifest: Manifest,
    source: Option<Rc<dyn CartSource>>,
    /// Network events offered by the host and not yet admitted: they
    /// wait here while the cart is paused, and at most `MAX_BATCH` are
    /// admitted per step.
    net_pending: std::collections::VecDeque<net::Event>,
    /// A session was live when this cart (or the one before it) was
    /// torn down; the host owes the peer a `Leave`.
    net_teardown: bool,
}

/// The shell's half: a second guest drawing into the overlay.
struct Shell {
    guest: Box<dyn Guest>,
    state: DrawState,
    frame: u64,
    status: ConsoleState,
    presentation: Vec<u8>,
    paused: bool,
    menu_was: bool,
    /// Buttons held when the shell handed the screen to the cart. They
    /// stay masked from the cart until released, so the A that chose a
    /// cart in the list is not an A press inside that cart.
    stale: u8,
    opener: CartOpener,
    factory: GuestFactory,
    preload: Preload,
    host_requests: Vec<SysRequest>,
}

/// The console: a cart guest, optionally a shell guest, and the frame
/// counters.
pub struct Console {
    cart: Cart,
    shell: Option<Shell>,
    silence: Vec<i16>,
    /// What a cart that declares the net service starts with.
    net_env: NetEnv,
}

mod net_side;

impl Cart {
    /// Read `cart.toml` and `main.lua` from `source`, decode the preload
    /// set and build a guest. Any error yields a cart that is already
    /// faulted, so the host reports it like a runtime fault.
    fn load(
        source: Rc<dyn CartSource>,
        factory: &dyn FactoryFn,
        preload: Preload,
        env: &NetEnv,
    ) -> Cart {
        let manifest = match read_manifest(source.as_ref()) {
            Ok(m) => m,
            Err(fault) => return Cart::faulted(fault, Manifest::default(), Some(source)),
        };
        let (w, h) = manifest.screen_mode.size();
        let mut state = DrawState::new(w, h, source.clone());
        if manifest.has_service(crate::manifest::Service::Net) {
            state.net = Some(NetState::new(env));
        }
        let (sheets, maps): (&[String], &[String]) = match preload {
            Preload::Decode => (&manifest.preload_sheets, &manifest.preload_maps),
            Preload::Skip => (&[], &[]),
        };
        for name in sheets {
            if let Err(e) = state.load_sheet(name) {
                let fault = Fault::new(
                    e.code(),
                    &crate::assets::sheet_path(name),
                    None,
                    e.to_string(),
                );
                return Cart::faulted(fault, manifest, Some(source));
            }
        }
        for name in maps {
            if let Err(e) = state.load_map(name) {
                let fault = Fault::new(
                    e.code(),
                    &crate::assets::map_path(name),
                    None,
                    e.to_string(),
                );
                return Cart::faulted(fault, manifest, Some(source));
            }
        }
        let bytes = match source.read(MAIN_FILE) {
            Ok(b) => b,
            Err(e) => {
                return Cart::faulted(
                    Fault::new(Fault::CART_READ_ERROR, MAIN_FILE, None, e.to_string()),
                    manifest,
                    Some(source),
                )
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => {
                return Cart::faulted(
                    Fault::new(
                        Fault::CART_READ_ERROR,
                        MAIN_FILE,
                        None,
                        "main.lua is not valid UTF-8",
                    ),
                    manifest,
                    Some(source),
                )
            }
        };
        match factory(&text, MAIN_FILE) {
            Ok(guest) => Cart {
                guest: Some(guest),
                state,
                frame: 0,
                status: ConsoleState::Running,
                manifest,
                source: Some(source),
                net_pending: Default::default(),
                net_teardown: false,
            },
            Err(fault) => Cart::faulted(fault, manifest, Some(source)),
        }
    }

    fn faulted(fault: Fault, manifest: Manifest, source: Option<Rc<dyn CartSource>>) -> Cart {
        let (w, h) = manifest.screen_mode.size();
        Cart {
            guest: None,
            state: DrawState::new(w, h, Rc::new(Snapshot::empty())),
            frame: 0,
            status: ConsoleState::Faulted(fault),
            manifest,
            source,
            net_pending: Default::default(),
            net_teardown: false,
        }
    }

    /// No cart at all: the shell's screen shows alone.
    fn empty() -> Cart {
        let (w, h) = ScreenMode::default().size();
        Cart {
            guest: None,
            state: DrawState::new(w, h, Rc::new(Snapshot::empty())),
            frame: 0,
            status: ConsoleState::Running,
            manifest: Manifest::default(),
            source: None,
            net_pending: Default::default(),
            net_teardown: false,
        }
    }

    fn step(&mut self, input: FrameInput) {
        if let (ConsoleState::Running, Some(guest)) = (&self.status, self.guest.as_mut()) {
            self.frame += 1;
            // The frame's batch: what the host offered, up to the bound,
            // admitted before the cart runs.
            if let Some(net) = self.state.net.as_mut() {
                let n = self.net_pending.len().min(net::MAX_BATCH);
                let batch: Vec<net::Event> = self.net_pending.drain(..n).collect();
                net.begin_frame(batch);
            }
            if let Err(fault) = guest.step(&mut self.state, input, self.frame) {
                self.status = ConsoleState::Faulted(fault);
                // A faulted cart's sound stops with it, and its session
                // ends: the host says bye for it.
                self.state.audio.stop_all();
                if self.state.net.as_ref().is_some_and(|n| n.used_link()) {
                    self.net_teardown = true;
                }
            }
        }
        // Audio renders every step, faulted or not, so the host's ring
        // buffer and the capture stay in lockstep with the frames.
        self.state.audio.render();
    }
}

impl Console {
    /// Read `cart.toml` and `main.lua` from `source`, decode the preload
    /// set and build a guest with `factory`, which receives the source
    /// text and the chunk name. Any error yields a console that is
    /// already faulted, so the host reports it like a runtime fault.
    pub fn new<F>(source: Rc<dyn CartSource>, factory: F) -> Console
    where
        F: FnOnce(&str, &str) -> Result<Box<dyn Guest>, Fault>,
    {
        Console::new_with(source, factory, Preload::Decode)
    }

    /// [`Console::new`] with a say over asset decoding; hosts pass
    /// [`Preload::Skip`] for a guest that lives in a worker.
    pub fn new_with<F>(source: Rc<dyn CartSource>, factory: F, preload: Preload) -> Console
    where
        F: FnOnce(&str, &str) -> Result<Box<dyn Guest>, Fault>,
    {
        let factory = std::cell::RefCell::new(Some(factory));
        let cart = Cart::load(
            source,
            &|text, name| {
                let f = factory.borrow_mut().take().expect("the factory runs once");
                f(text, name)
            },
            preload,
            &NetEnv::default(),
        );
        Console::from_cart(cart)
    }

    /// Wrap an already-built guest with no cart and the default screen
    /// mode.
    pub fn from_guest(guest: Box<dyn Guest>) -> Console {
        let mut cart = Cart::empty();
        cart.guest = Some(guest);
        Console::from_cart(cart)
    }

    /// A console that faulted before it could start.
    pub fn faulted(fault: Fault) -> Console {
        Console::from_cart(Cart::faulted(fault, Manifest::default(), None))
    }

    fn from_cart(cart: Cart) -> Console {
        Console {
            cart,
            shell: None,
            silence: vec![0; VALUES_PER_FRAME],
            net_env: NetEnv::default(),
        }
    }

    /// A console booted into the shell with no cart loaded. `shell` is
    /// the shell guest (built with `sys` installed), `carts` what it may
    /// list, `opener` how a listed name becomes a cart source, and
    /// `factory` how cart guests are built and `preload` whether loading
    /// decodes their assets here.
    pub fn with_shell(
        shell: Box<dyn Guest>,
        carts: Vec<CartEntry>,
        settings: Settings,
        opener: CartOpener,
        factory: GuestFactory,
        preload: Preload,
    ) -> Console {
        let (w, h) = ScreenMode::default().size();
        let mut state = DrawState::new(w, h, Rc::new(Snapshot::empty()));
        state.sys = Some(SysState::new(carts, settings));
        let mut console = Console::from_cart(Cart::empty());
        console.shell = Some(Shell {
            guest: shell,
            state,
            frame: 0,
            status: ConsoleState::Running,
            presentation: Vec::new(),
            paused: false,
            menu_was: false,
            stale: 0,
            opener,
            factory,
            preload,
            host_requests: Vec::new(),
        });
        console
    }

    /// Replace the cart. Used by the shell's `sys.run` and `sys.restart`
    /// and by hosts that skip the shell's list.
    pub fn load_cart(&mut self, source: Rc<dyn CartSource>, store: Box<dyn SaveStore>) {
        let (factory, preload) = match &self.shell {
            Some(s) => (s.factory.clone(), s.preload),
            None => return,
        };
        let mut cart = Cart::load(source, &*factory, preload, &self.net_env);
        cart.state.saves = store;
        self.install_cart(cart);
    }

    /// Replace the cart from the host's side, with no request from the
    /// shell: a development deploy. It is `load_cart` with what a shell's
    /// own `sys.run` does around it (no invite, a fresh network view), and
    /// the shell is told once through `sys.host_started()`, so that it
    /// shows the cart whatever screen it was on.
    pub fn host_load_cart(&mut self, source: Rc<dyn CartSource>, store: Box<dyn SaveStore>) {
        if self.shell.is_none() {
            return;
        }
        self.net_env.invite = None;
        if let Some(view) = self.network_view_mut() {
            view.reset_session();
        }
        self.load_cart(source, store);
        if let Some(sys) = self.shell.as_mut().and_then(|s| s.state.sys.as_mut()) {
            sys.host_started = true;
        }
    }

    fn install_cart(&mut self, mut cart: Cart) {
        if let Some(shell) = &mut self.shell {
            cart.state.audio.set_master(shell.settings().volume as u8);
            let (w, h) = (cart.state.width(), cart.state.height());
            if shell.state.width() != w || shell.state.height() != h {
                // The overlay follows the cart's screen mode. The shell
                // keeps its Lua state; only its buffers are rebuilt.
                let sys = shell.state.sys.take();
                shell.state = DrawState::new(w, h, Rc::new(Snapshot::empty()));
                shell.state.sys = sys;
            }
            shell.paused = false;
        }
        // A session of the outgoing cart ends with it; the host owes its
        // peer a `Leave`, which `take_net_commands` delivers. An ended
        // session counts too: the link still holds the transport built
        // for the outgoing cart, and the next cart must not greet its
        // peer through it with the wrong identity.
        cart.net_teardown =
            self.cart.net_teardown || self.cart.state.net.as_ref().is_some_and(|n| n.used_link());
        self.cart = cart;
    }

    /// Run one frame. After a fault this is a no-op that keeps returning
    /// the last screen with an empty log, so a host that echoes the log
    /// every step prints the faulting frame's lines once.
    pub fn step(&mut self, input: FrameInput) -> FrameOutput<'_> {
        self.step_with(input, Vec::new())
    }

    /// [`Console::step`] with the network events the host offers for
    /// this frame; see `net_side.rs`.
    pub fn step_with(&mut self, input: FrameInput, events: Vec<net::Event>) -> FrameOutput<'_> {
        self.offer_net_events(events);
        self.cart.state.log.clear();
        let Some(shell) = self.shell.as_mut() else {
            self.cart.step(input.for_cart());
            return self.output();
        };

        // The Menu key is the shell's; the cart never sees it.
        let menu = input.buttons & BTN_MENU != 0;
        let menu_pressed = menu && !shell.menu_was;
        shell.menu_was = menu;
        let network_overlay = shell.state.sys.as_ref().is_some_and(|s| s.network.overlay);
        let overlay_active = network_overlay
            || shell.paused
            || self.cart.guest.is_none()
            || self.cart.status.fault().is_some();

        shell.stale &= input.buttons;
        let cart_input = FrameInput::new(input.buttons & !shell.stale).for_cart();
        if !shell.paused {
            self.cart.step(if network_overlay {
                FrameInput::NONE
            } else {
                cart_input
            });
        }

        // The shell sees the buttons only while its overlay is up, so a
        // running cart and the shell never both react to A.
        shell.state.log.clear();
        if let Some(sys) = shell.state.sys.as_mut() {
            sys.fault = self.cart.status.fault().cloned();
            sys.running = self.cart.guest.is_some() || self.cart.source.is_some();
            sys.paused = shell.paused;
            sys.menu_pressed = menu_pressed;
            sys.network.status = self
                .cart
                .state
                .net
                .as_ref()
                .map(|n| n.status.as_str())
                .unwrap_or("off")
                .into();
            // The one owner of the shown ticket; nothing is shown while
            // networking is off, whatever the cart's state still holds.
            sys.network.ticket = self
                .cart
                .state
                .net
                .as_ref()
                .filter(|_| self.net_env.permitted)
                .and_then(|n| n.ticket.clone())
                .unwrap_or_default();
        }
        let shell_input = if overlay_active {
            input.for_cart()
        } else {
            FrameInput::NONE
        };
        if shell.status.fault().is_none() {
            shell.frame += 1;
            if let Err(fault) = shell.guest.step(&mut shell.state, shell_input, shell.frame) {
                shell.status = ConsoleState::Faulted(fault);
            }
        }
        let requests = shell
            .state
            .sys
            .as_mut()
            .map(|s| std::mem::take(&mut s.requests))
            .unwrap_or_default();
        for r in requests {
            self.apply(r);
        }
        let shell = self.shell.as_mut().expect("shell survives its step");
        let overlay_now = shell.state.sys.as_ref().is_some_and(|s| s.network.overlay)
            || shell.paused
            || self.cart.guest.is_none()
            || self.cart.status.fault().is_some();
        if overlay_active && !overlay_now {
            shell.stale = input.buttons;
        }
        if shell.status.fault().is_none() {
            shell::compose(
                self.cart.state.screen_pixels(),
                shell.state.screen_pixels(),
                &mut shell.presentation,
            );
        }
        self.output()
    }

    fn apply(&mut self, r: SysRequest) {
        let shell = self.shell.as_mut().expect("requests come from a shell");
        match r {
            SysRequest::RunNetwork { name, invite } => {
                self.net_env.invite = invite;
                self.run_shell_cart(&name);
            }
            SysRequest::Network(shell::network::Action::Overlay(on)) => {
                if let Some(sys) = shell.state.sys.as_mut() {
                    sys.network.overlay = on;
                }
            }
            SysRequest::Network(action) => shell.host_requests.push(SysRequest::Network(action)),
            SysRequest::Run(name) => {
                self.net_env.invite = None;
                self.run_shell_cart(&name);
            }
            SysRequest::Restart => {
                if let Some(sys) = shell.state.sys.as_mut() {
                    sys.network.reset_session();
                }
                if let Some(source) = self.cart.source.clone() {
                    let store = std::mem::replace(
                        &mut self.cart.state.saves,
                        Box::new(crate::save::MemoryStore::new()),
                    );
                    self.load_cart(source, store);
                }
            }
            SysRequest::Quit => {
                self.net_env.invite = None;
                if let Some(sys) = shell.state.sys.as_mut() {
                    sys.network.reset_session();
                }
                shell.paused = false;
                self.install_cart(Cart::empty());
            }
            SysRequest::Paused(on) => shell.paused = on,
            SysRequest::SetScale(n) => {
                if let Some(sys) = shell.state.sys.as_mut() {
                    sys.settings.scale = n;
                }
                shell.host_requests.push(SysRequest::SetScale(n));
            }
            SysRequest::SetVolume(v) => {
                if let Some(sys) = shell.state.sys.as_mut() {
                    sys.settings.volume = v;
                }
                self.cart.state.audio.set_master(v as u8);
                // The host persists it.
                shell.host_requests.push(SysRequest::SetVolume(v));
            }
            SysRequest::SetNet(on) => {
                if let Some(sys) = shell.state.sys.as_mut() {
                    sys.settings.net = on;
                }
                // Carts loaded from now on start with this permission;
                // the running one is told through its link's event.
                self.net_env.permitted = on;
                shell.host_requests.push(SysRequest::SetNet(on));
            }
        }
    }

    /// Requests the host carries out or persists (the window scale,
    /// the volume, the network permission), drained.
    pub fn take_host_requests(&mut self) -> Vec<SysRequest> {
        match &mut self.shell {
            Some(s) => std::mem::take(&mut s.host_requests),
            None => Vec::new(),
        }
    }

    /// The host's answer to a scale request it could not honour in full:
    /// the scale the window really has, so the settings screen shows it
    /// and the host persists it. A host without a shell has nothing to
    /// tell.
    pub fn set_effective_scale(&mut self, scale: u32) {
        if let Some(sys) = self.shell.as_mut().and_then(|s| s.state.sys.as_mut()) {
            sys.settings.scale = scale;
        }
    }

    pub fn has_shell(&self) -> bool {
        self.shell.is_some()
    }

    /// The shell guest's own fault, if it died. The host falls back to
    /// the Rust error screen then.
    pub fn shell_fault(&self) -> Option<&Fault> {
        self.shell.as_ref().and_then(|s| s.status.fault())
    }

    pub fn is_paused(&self) -> bool {
        self.shell.as_ref().is_some_and(|s| s.paused)
    }

    /// The shell's settings, or the defaults without a shell.
    pub fn settings(&self) -> Settings {
        self.shell
            .as_ref()
            .map(|s| s.settings())
            .unwrap_or_default()
    }

    pub fn state(&self) -> &ConsoleState {
        &self.cart.status
    }

    /// Canonical dump of the guest's named globals; see [`Guest::state`].
    pub fn state_dump(&mut self, names: &[String]) -> Result<String, Fault> {
        match self.cart.guest.as_mut() {
            Some(g) => g.state(names),
            None => Err(Fault::new("unsupported", "", None, "no guest")),
        }
    }

    pub fn frame(&self) -> u64 {
        self.cart.frame
    }

    pub fn manifest(&self) -> &Manifest {
        &self.cart.manifest
    }

    pub fn screen_mode(&self) -> ScreenMode {
        self.cart.manifest.screen_mode
    }

    /// The draw state, for hosts and tests that inspect resources.
    pub fn draw_state(&self) -> &DrawState {
        &self.cart.state
    }

    /// Install the store `save` and `load` use. The
    /// default is an in-memory store that dies with the console.
    pub fn set_save_store(&mut self, store: Box<dyn SaveStore>) {
        self.cart.state.saves = store;
    }

    /// Disk failures the save store kept from the cart, drained.
    pub fn take_save_failures(&mut self) -> Vec<crate::save::SaveError> {
        self.cart.state.saves.take_failures()
    }

    /// The current screen, palette and log without stepping.
    pub fn output(&self) -> FrameOutput<'_> {
        let cart = &self.cart;
        // A paused cart is not rendering, so its last buffer must not be
        // republished, whether or not the shell is still alive.
        let audio = match &self.shell {
            Some(shell) if shell.paused => self.silence.as_slice(),
            _ => cart.state.audio.output(),
        };
        let screen = match &self.shell {
            Some(shell) if shell.status.fault().is_none() => shell.presentation.as_slice(),
            _ => cart.state.screen_pixels(),
        };
        FrameOutput {
            screen,
            width: cart.state.width(),
            height: cart.state.height(),
            palette: cart.state.palette.entries(),
            frame: cart.frame,
            log: &cart.state.log,
            profile: &cart.state.profile,
            audio,
        }
    }
}

impl Shell {
    fn settings(&self) -> Settings {
        self.state
            .sys
            .as_ref()
            .map(|s| s.settings)
            .unwrap_or_default()
    }
}

fn read_manifest(source: &dyn CartSource) -> Result<Manifest, Fault> {
    if !source.exists(MANIFEST_FILE) {
        return Ok(Manifest::default());
    }
    let bytes = source
        .read(MANIFEST_FILE)
        .map_err(|e| Fault::new(Fault::CART_READ_ERROR, MANIFEST_FILE, None, e.to_string()))?;
    let text = String::from_utf8(bytes).map_err(|_| {
        Fault::new(
            ManifestError::CODE,
            MANIFEST_FILE,
            None,
            "cart.toml is not valid UTF-8",
        )
    })?;
    Manifest::parse(&text)
        .map_err(|e| Fault::new(ManifestError::CODE, MANIFEST_FILE, e.line, e.message))
}

#[cfg(test)]
mod tests;

impl Console {
    fn run_shell_cart(&mut self, name: &str) {
        if let Some(view) = self.network_view_mut() {
            view.reset_session();
        }
        let opener = self.shell.as_ref().expect("shell request").opener.clone();
        match opener(name) {
            Ok((source, store)) => self.load_cart(source, store),
            Err(fault) => self.install_cart(Cart::faulted(fault, Manifest::default(), None)),
        }
    }

    pub fn network_view_mut(&mut self) -> Option<&mut shell::network::View> {
        self.shell
            .as_mut()?
            .state
            .sys
            .as_mut()
            .map(|s| &mut s.network)
    }

    pub fn network_view(&self) -> Option<&shell::network::View> {
        self.shell.as_ref()?.state.sys.as_ref().map(|s| &s.network)
    }
}
