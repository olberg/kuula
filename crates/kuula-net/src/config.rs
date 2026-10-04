//! Explicit relay policy. No public relay or address lookup defaults.
use crate::{Code, NetError};
use iroh::{EndpointAddr, RelayMode, RelayUrl, TransportAddr};
use iroh_tickets::endpoint::EndpointTicket;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayConfig {
    pub url: Option<String>,
    pub only: bool,
}
impl RelayConfig {
    pub fn validate(&self) -> Result<(), NetError> {
        self.parsed()?;
        if self.only && self.url.is_none() {
            return Err(NetError::new(
                Code::Ticket,
                "relay-only needs a configured relay URL",
            ));
        }
        Ok(())
    }
    fn parsed(&self) -> Result<Option<RelayUrl>, NetError> {
        let Some(s) = &self.url else { return Ok(None) };
        if s.len() > 512
            || s.trim() != s
            || !(s.starts_with("https://") || s.starts_with("http://"))
        {
            return Err(NetError::new(
                Code::Ticket,
                "relay URL must be http(s), at most 512 bytes",
            ));
        }
        let url: RelayUrl = s
            .parse()
            .map_err(|_| NetError::new(Code::Ticket, "invalid relay URL"))?;
        let u = url::Url::parse(s).map_err(|_| NetError::new(Code::Ticket, "invalid relay URL"))?;
        if !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
            || u.path() != "/"
        {
            return Err(NetError::new(
                Code::Ticket,
                "relay URL must be an origin without credentials, path, query or fragment",
            ));
        }
        Ok(Some(url))
    }
    pub(crate) fn mode(&self) -> Result<RelayMode, NetError> {
        Ok(match self.parsed()? {
            Some(url) => RelayMode::custom([url]),
            None => RelayMode::Disabled,
        })
    }
    pub fn ticket_addr(&self, text: &str) -> Result<EndpointAddr, NetError> {
        self.validate()?;
        if text.len() > kuula_core::net::MAX_TICKET {
            return Err(NetError::new(Code::Ticket, "ticket exceeds 1024 bytes"));
        }
        let ticket: EndpointTicket = text
            .trim()
            .parse()
            .map_err(|_| NetError::new(Code::Ticket, "cannot parse connection ticket"))?;
        let source = ticket.endpoint_addr();
        let allowed = self.parsed()?;
        // Never dial a relay merely because an untrusted ticket names it.
        let addrs = source
            .addrs
            .iter()
            .filter(|addr| match addr {
                TransportAddr::Ip(_) => !self.only,
                TransportAddr::Relay(url) => allowed.as_ref() == Some(url),
                _ => false,
            })
            .cloned();
        let addr = EndpointAddr::from(source.id).with_addrs(addrs);
        if addr.addrs.is_empty() {
            return Err(NetError::new(
                Code::Ticket,
                "ticket has no address allowed by your relay configuration",
            ));
        }
        Ok(addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ticket_relays_cannot_override_host_policy() {
        let secret = iroh::SecretKey::from_bytes(&[7; 32]);
        let good: RelayUrl = "http://127.0.0.1:3340".parse().unwrap();
        let bad: RelayUrl = "https://unconfigured.example".parse().unwrap();
        let addr = EndpointAddr::from(secret.public())
            .with_relay_url(bad)
            .with_ip_addr(([127, 0, 0, 1], 1000).into());
        let text = EndpointTicket::new(addr).to_string();
        let direct = RelayConfig::default().ticket_addr(&text).unwrap();
        assert_eq!(direct.ip_addrs().count(), 1);
        assert_eq!(direct.relay_urls().count(), 0);
        let only = RelayConfig {
            url: Some(good.to_string()),
            only: true,
        };
        assert!(only.ticket_addr(&text).is_err());
        let text = EndpointTicket::new(EndpointAddr::from(secret.public()).with_relay_url(good))
            .to_string();
        assert!(RelayConfig::default().ticket_addr(&text).is_err());
        let addr = only.ticket_addr(&text).unwrap();
        assert_eq!(addr.relay_urls().count(), 1);
        assert_eq!(addr.ip_addrs().count(), 0);
    }
    #[test]
    fn configuration_is_explicit_and_strict() {
        assert!(RelayConfig::default().validate().is_ok());
        assert!(RelayConfig {
            only: true,
            url: None
        }
        .validate()
        .is_err());
        for url in [
            "",
            "ftp://host/",
            "https://u:p@host",
            "https://host/path",
            "https://host/?x=1",
            "https://host/#x",
        ] {
            assert!(
                RelayConfig {
                    url: Some(url.into()),
                    only: false
                }
                .validate()
                .is_err(),
                "{url}"
            );
        }
        assert!(RelayConfig {
            url: Some("http://127.0.0.1:3340".into()),
            only: true
        }
        .validate()
        .is_ok());
    }
}
