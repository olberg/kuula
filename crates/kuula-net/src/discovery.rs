//! Opt-in LAN session advertisements, independent of Iroh address lookup.
//! Untrusted hints only: compatibility is rechecked by the play handshake.
use kuula_core::net::identity::Identity;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

pub const PORT: u16 = 47755;
pub const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 75, 85);
pub const MAX_PACKET: usize = 1400;
pub const MAX_CANDIDATES: usize = 32;
pub const EXPIRES: Duration = Duration::from_secs(5);
const PERIOD: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Advertisement {
    pub protocol: String,
    pub digest: String,
    pub profile: String,
    pub title: String,
    pub ticket: String,
}
impl Advertisement {
    pub fn new(identity: &Identity, title: &str, ticket: &str) -> Self {
        Self {
            protocol: "kuula-lan-1".into(),
            digest: identity.hex(),
            profile: identity.profile.clone(),
            title: title
                .chars()
                .filter(|c| c.is_ascii() && !c.is_control())
                .take(64)
                .collect(),
            ticket: ticket.into(),
        }
    }
    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_PACKET {
            return None;
        }
        let a: Self = serde_json::from_slice(bytes).ok()?;
        if a.protocol != "kuula-lan-1"
            || a.digest.len() != 64
            || !a.digest.bytes().all(|b| b.is_ascii_hexdigit())
            || a.profile.len() > 128
            || a.title.len() > 64
            || !a.title.is_ascii()
            || a.title.bytes().any(|b| b.is_ascii_control())
            || a.ticket.len() > kuula_core::net::MAX_TICKET
        {
            return None;
        }
        let _: iroh_tickets::endpoint::EndpointTicket = a.ticket.parse().ok()?;
        Some(a)
    }
}

#[derive(Default)]
pub struct Catalog {
    entries: Vec<(Advertisement, Instant)>,
}
impl Catalog {
    /// Incompatible advertisements never take a slot, so a flood of foreign
    /// or hostile packets cannot crowd out a matching host.
    pub fn ingest(&mut self, packet: &[u8], now: Instant, identity: &Identity) {
        self.expire(now);
        let Some(a) = Advertisement::decode(packet) else {
            return;
        };
        if a.digest != identity.hex() || a.profile != identity.profile {
            return;
        }
        if let Some(entry) = self.entries.iter_mut().find(|(e, _)| e.ticket == a.ticket) {
            *entry = (a, now);
        } else if self.entries.len() < MAX_CANDIDATES {
            self.entries.push((a, now));
        }
    }
    pub fn expire(&mut self, now: Instant) {
        self.entries
            .retain(|(_, t)| now.saturating_duration_since(*t) < EXPIRES);
    }
    pub fn candidates(&self, own_ticket: &str) -> Vec<Advertisement> {
        self.entries
            .iter()
            .filter(|(a, _)| a.ticket != own_ticket)
            .map(|(a, _)| a.clone())
            .collect()
    }
}

pub struct Discovery {
    socket: UdpSocket,
    catalog: Catalog,
    last_send: Option<Instant>,
}
impl Discovery {
    pub fn open() -> std::io::Result<Self> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_reuse_address(true)?;
        socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT).into())?;
        socket.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
        socket.set_multicast_ttl_v4(1)?;
        socket.set_multicast_loop_v4(true)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket: socket.into(),
            catalog: Catalog::default(),
            last_send: None,
        })
    }
    pub fn poll(
        &mut self,
        advertisement: Option<&Advertisement>,
        identity: &Identity,
        own_ticket: &str,
    ) -> std::io::Result<Vec<Advertisement>> {
        let now = Instant::now();
        self.catalog.expire(now);
        if let Some(a) = advertisement {
            if self
                .last_send
                .is_none_or(|t| now.duration_since(t) >= PERIOD)
            {
                let bytes = serde_json::to_vec(a).map_err(std::io::Error::other)?;
                if bytes.len() > MAX_PACKET {
                    return Err(std::io::Error::other(
                        "LAN advertisement exceeds packet limit",
                    ));
                }
                self.socket
                    .send_to(&bytes, SocketAddrV4::new(GROUP, PORT))?;
                self.last_send = Some(now);
            }
        } else {
            self.last_send = None;
        }
        let mut buf = [0; MAX_PACKET + 1];
        for _ in 0..32 {
            match self.socket.recv_from(&mut buf) {
                Ok((n, _)) => self.catalog.ingest(&buf[..n], now, identity),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                // Windows reports an oversized datagram as WSAEMSGSIZE.
                Err(e) if e.raw_os_error() == Some(10040) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(self.catalog.candidates(own_ticket))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn advertisement(n: u16) -> (Identity, Advertisement) {
        let identity = Identity::new(&kuula_core::Snapshot::empty());
        let addr = iroh::EndpointAddr::from(iroh::SecretKey::from_bytes(&[3; 32]).public())
            .with_ip_addr(([127, 0, 0, 1], n).into());
        let ticket = iroh_tickets::endpoint::EndpointTicket::new(addr).to_string();
        let a = Advertisement::new(&identity, "Marble Duel", &ticket);
        (identity, a)
    }
    #[test]
    fn bounded_catalog_filters_compatibility_and_expires() {
        let (id, first) = advertisement(1);
        let mut c = Catalog::default();
        let now = Instant::now();
        let mut other = id.clone();
        other.profile.push('x');
        // Foreign advertisements are dropped before they can take a slot.
        for n in 1..=100 {
            let mut foreign = advertisement(n).1;
            foreign.profile.push('x');
            c.ingest(&serde_json::to_vec(&foreign).unwrap(), now, &id);
        }
        assert!(c.candidates("").is_empty());
        for n in 1..=100 {
            c.ingest(&serde_json::to_vec(&advertisement(n).1).unwrap(), now, &id);
        }
        assert_eq!(c.candidates("").len(), MAX_CANDIDATES);
        assert_eq!(c.candidates(&first.ticket).len(), MAX_CANDIDATES - 1);
        let mut c2 = Catalog::default();
        c2.ingest(&serde_json::to_vec(&first).unwrap(), now, &other);
        assert!(c2.candidates("").is_empty());
        c.expire(now + EXPIRES);
        assert!(c.candidates("").is_empty());
    }
    #[test]
    fn malformed_and_oversized_packets_are_ignored() {
        let (_, mut a) = advertisement(1);
        assert!(Advertisement::decode(&vec![b' '; MAX_PACKET + 1]).is_none());
        a.title = "bad\nname".into();
        assert!(Advertisement::decode(&serde_json::to_vec(&a).unwrap()).is_none());
        a.title = "ok".into();
        a.ticket = "not a ticket".into();
        assert!(Advertisement::decode(&serde_json::to_vec(&a).unwrap()).is_none());
    }
}
