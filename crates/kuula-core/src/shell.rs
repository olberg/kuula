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

use std::rc::Rc;

use crate::fault::Fault;
use crate::source::CartSource;

/// A cart the shell can list and run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartEntry {
    /// Stable name the shell passes back to `sys.run`.
    pub name: String,
    /// Display title from the manifest, or the name.
    pub title: String,
}

/// Host-side settings the shell edits. Applying them is the host's job
/// (the window scale) or the console's (the master volume).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub scale: u32,
    /// Master volume, 0 to 100.
    pub volume: u32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            scale: 2,
            volume: 100,
        }
    }
}

/// What the shell asked for during its step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysRequest {
    Run(String),
    Quit,
    Restart,
    SetScale(u32),
    SetVolume(u32),
    Paused(bool),
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
    pub requests: Vec<SysRequest>,
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
            requests: Vec::new(),
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
