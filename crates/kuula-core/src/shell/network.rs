//! Shell-only network UI data. Never installed in a cart or transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Overlay(bool),
    Browse(String),
    Discovery(bool),
    Relay { url: String, only: bool },
    Edit(String),
    Text(String),
    Copy,
    Paste,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Candidate {
    pub title: String,
    pub ticket: String,
}

#[derive(Debug, Clone, Default)]
pub struct View {
    /// Why the host has no networking at all (a build without it); empty
    /// when it has. The shell shows this instead of the multiplayer menu.
    pub unavailable: String,
    pub overlay: bool,
    pub ended: bool,
    pub status: String,
    pub detail: String,
    pub ticket: String,
    pub path: String,
    pub relay: String,
    pub relay_only: bool,
    pub discovery: bool,
    pub discovery_detail: String,
    pub candidates: Vec<Candidate>,
    pub editing: String,
    pub text: String,
    pub network_profile: String,
}

impl View {
    pub fn reset_session(&mut self) {
        self.ended = false;
        self.status.clear();
        self.detail.clear();
        self.path.clear();
    }
    /// Updates the event-driven fields. `ticket` is not one of them: the
    /// console copies it from the cart's net state every frame.
    pub fn observe(&mut self, event: &crate::net::Event) {
        use crate::net::{Event, Reason};
        match event {
            Event::Hosting { .. } => self.detail.clear(),
            Event::Connected { .. } => {
                self.detail.clear();
                self.ended = false;
            }
            Event::Disconnected { reason } if *reason != Reason::Closed => {
                self.ended = true;
                self.detail =
                    "Peer disconnected. This round has ended; start a fresh session.".into();
            }
            Event::Failed { code, detail } => {
                self.detail = format!("{}: {}", code.as_str(), detail)
            }
            Event::Permission { granted: false } => {
                self.detail = "Networking is off. Sessions and discovery are closed.".into();
                self.path.clear();
                self.candidates.clear();
            }
            _ => {}
        }
    }
}
