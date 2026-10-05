//! The shell side of the console.
//!
//! The shell is a second guest with its own [`DrawState`]: it draws an
//! overlay the console composes over the cart's screen, keyed on colour
//! 0, and talks to the console through [`SysState`], the data behind the
//! shell guest's `sys` table. The cart guest's state has no `SysState`,
//! which is how a cart never reaches host authority.
//!
//! Requests the shell makes (`run`, `quit`, `restart`, settings) are
//! queued here during its step and applied by the console afterwards, so
//! a request never tears down the guest that is making it.

pub mod network;

use std::rc::Rc;

use crate::fault::Fault;
use crate::source::CartSource;

/// A cart the shell can list and run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CartEntry {
    /// Stable name the shell passes back to `sys.run`.
    pub name: String,
    /// Display title from the manifest, or the name.
    pub title: String,
    pub network: bool,
    pub author: String,
    pub license: Option<crate::manifest::License>,
}

/// Host-side settings the shell edits. Applying them is the host's job
/// (the window scale) or the console's (the master volume).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub scale: u32,
    /// Master volume, 0 to 100.
    pub volume: u32,
    /// Whether carts the shell starts may use the network: the
    /// persistent half of the permission gate.
    pub net: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            scale: 2,
            volume: 100,
            net: false,
        }
    }
}

/// What the shell asked for during its step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysRequest {
    Run(String),
    RunNetwork {
        name: String,
        invite: Option<String>,
    },
    Network(network::Action),
    Quit,
    Restart,
    SetScale(u32),
    SetVolume(u32),
    /// Grant or withdraw networking for the running cart and the ones
    /// after it; the host flips its link and persists the setting.
    SetNet(bool),
    Paused(bool),
    /// End the host: what a shell started on one cart asks for where it
    /// would have gone back to its list.
    Exit,
    /// The person's answer to a developer waiting for approval.
    Dev(DevAction),
}

/// What the person holding the device answers to `DevView::pending`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevAction {
    Approve,
    Refuse,
}

/// The development receiver as the shell sees it: `sys.dev()`. The host
/// fills it; a shell without a receiver has it empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DevView {
    /// The shell was started as a development receiver.
    pub active: bool,
    /// The endpoint id of a developer the receiver refused because it is
    /// not approved, until the person answers; empty when nobody waits.
    pub pending: String,
    /// What that developer offered to send, as it claimed: the cart's
    /// name, its size in bytes and the address it came from. Empty and 0
    /// when it did not say.
    pub cart: String,
    pub bytes: u32,
    pub from: String,
    /// The last thing the receiver did, for the list screen.
    pub note: String,
}

/// Where the console gets a cart from when the shell asks to run one.
/// The CLI hands in a closure that builds a snapshot; the core never
/// opens a directory itself.
pub type CartOpener = Rc<dyn Fn(&str) -> Result<Rc<dyn CartSource>, Fault>>;

/// The data behind `sys`. Lives in the shell's `DrawState`.
pub struct SysState {
    pub carts: Vec<CartEntry>,
    pub settings: Settings,
    /// The cart's fault, copied in before each shell step.
    pub fault: Option<Fault>,
    /// Whether a cart is loaded.
    pub running: bool,
    pub paused: bool,
    /// Menu went down this frame.
    pub menu_pressed: bool,
    /// The host loaded a cart itself (`Console::host_load_cart`) and the
    /// shell has not asked since: `sys.host_started()` reads and clears it.
    pub host_started: bool,
    /// The shell was started on one cart (`kuula shell --cart`): it opens
    /// that cart at once and ends the host instead of showing its list.
    pub single: bool,
    pub requests: Vec<SysRequest>,
    pub network: network::View,
    pub dev: DevView,
}

impl SysState {
    pub fn new(carts: Vec<CartEntry>, settings: Settings) -> SysState {
        SysState {
            carts,
            settings,
            fault: None,
            running: false,
            paused: false,
            menu_pressed: false,
            host_started: false,
            single: false,
            requests: Vec::new(),
            network: network::View::default(),
            dev: DevView::default(),
        }
    }

    pub fn request(&mut self, r: SysRequest) {
        // A frame of shell code makes at most a handful of requests;
        // a loop of them is bounded by the shell's own cycle budget,
        // and the vector by this cap.
        if self.requests.len() < 64 {
            self.requests.push(r);
        }
    }
}

impl std::fmt::Debug for SysState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SysState")
            .field("carts", &self.carts.len())
            .field("settings", &self.settings)
            .field("running", &self.running)
            .field("paused", &self.paused)
            .field("requests", &self.requests)
            .finish()
    }
}

/// Compose `overlay` over `cart` into `out`: every overlay pixel that is
/// not colour 0 replaces the cart's. All three are the same size.
pub fn compose(cart: &[u8], overlay: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.extend(
        cart.iter()
            .zip(overlay.iter())
            .map(|(&c, &o)| if o == 0 { c } else { o }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_keys_on_colour_zero() {
        let mut out = Vec::new();
        compose(&[5, 6, 7], &[0, 9, 0], &mut out);
        assert_eq!(out, [5, 9, 7]);
    }

    #[test]
    fn requests_are_capped() {
        let mut s = SysState::new(Vec::new(), Settings::default());
        for _ in 0..100 {
            s.request(SysRequest::Quit);
        }
        assert_eq!(s.requests.len(), 64);
    }
}
