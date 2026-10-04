//! Host-owned content compatibility. These messages never enter cart state.
//! A digest identifies content; it does not authenticate its publisher.

use sha2::{Digest, Sha256};

use super::{Command, Event, FailCode, Transport};
use crate::Snapshot;

pub const PROFILE: &str = concat!("kuula/", env!("CARGO_PKG_VERSION"), "/lua55-f64-i64/net1");
const MAGIC: &[u8] = b"KUULA-CART-1\0";
/// Polls with available inbox room allowed to complete compatibility.
pub const HANDSHAKE_POLLS: u32 = 600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub digest: [u8; 32],
    pub profile: String,
}

impl Identity {
    pub fn new(snapshot: &Snapshot) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"kuula-cart-sha256-v1\0");
        for (name, bytes) in snapshot.entries() {
            hash.update((name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        Self {
            digest: hash.finalize().into(),
            profile: PROFILE.into(),
        }
    }

    pub fn hex(&self) -> String {
        self.digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn hello(&self) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&self.digest);
        bytes.extend_from_slice(self.profile.as_bytes());
        bytes
    }

    fn check(&self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() < MAGIC.len() + 32 || !data.starts_with(MAGIC) {
            return Err("invalid cart compatibility greeting");
        }
        if data[MAGIC.len()..MAGIC.len() + 32] != self.digest {
            return Err("cart content digest differs; both players need the same cart");
        }
        if data[MAGIC.len() + 32..] != *self.profile.as_bytes() {
            return Err("runtime/network profile differs; both players need compatible runtimes");
        }
        Ok(())
    }
}

/// One bounded greeting in each direction before Connected is admitted.
pub struct VerifiedTransport {
    inner: Box<dyn Transport>,
    identity: Identity,
    waiting: Option<(u32, u32)>,
    ready: bool,
    refused: bool,
}

impl VerifiedTransport {
    pub fn new(inner: Box<dyn Transport>, identity: Identity) -> Self {
        Self {
            inner,
            identity,
            waiting: None,
            ready: false,
            refused: false,
        }
    }

    fn refuse(&mut self, out: &mut Vec<Event>, detail: &str) {
        self.inner.push(Command::Leave);
        self.waiting = None;
        self.ready = false;
        self.refused = true;
        out.push(Event::failed(FailCode::Handshake, detail));
    }
}

impl Transport for VerifiedTransport {
    fn push(&mut self, cmd: Command) {
        match &cmd {
            Command::Host | Command::Join { .. } => {
                self.refused = false;
            }
            Command::Send { .. } if !self.ready => return,
            Command::Leave => {
                self.waiting = None;
                self.ready = false;
            }
            _ => {}
        }
        self.inner.push(cmd);
    }

    fn poll(&mut self, out: &mut Vec<Event>, max: usize) {
        if max == 0 {
            return;
        }
        let before = out.len();
        let mut events = Vec::new();
        self.inner.poll(&mut events, max);
        for event in events {
            if self.refused {
                continue;
            }
            match event {
                Event::Connected { peer } => {
                    self.ready = false;
                    self.waiting = Some((peer, 0));
                    self.inner.push(Command::Send {
                        data: self.identity.hello(),
                    });
                }
                Event::Message { data, .. } if self.waiting.is_some() => {
                    match self.identity.check(&data) {
                        Ok(()) => {
                            let (peer, _) = self.waiting.take().unwrap();
                            self.ready = true;
                            out.push(Event::Connected { peer });
                        }
                        Err(detail) => self.refuse(out, detail),
                    }
                }
                Event::Message { .. } if !self.ready => {
                    self.refuse(out, "gameplay arrived before compatibility")
                }
                Event::Disconnected { .. } => {
                    self.waiting = None;
                    self.ready = false;
                    out.push(event);
                }
                Event::Failed {
                    code: FailCode::Handshake,
                    detail,
                } => {
                    // The raw listener may rearm after a wire handshake error.
                    // Cart-visible handshake failures are terminal, so close it.
                    self.refuse(out, &detail);
                }
                other => out.push(other),
            }
        }
        if let Some((_, polls)) = &mut self.waiting {
            *polls += 1;
            if *polls >= HANDSHAKE_POLLS && out.len() - before < max {
                self.refuse(out, "cart compatibility greeting timed out");
            }
        }
    }
}

#[cfg(test)]
mod tests;
