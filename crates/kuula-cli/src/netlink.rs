//! The CLI's one seam to `kuula-net`. With the `net` feature (the
//! default) a networked run steps through an Iroh transport; without it
//! the binary carries no tokio, QUIC or TLS at all, every link is
//! offline and the network options say so. The Miyoo Mini Plus build is
//! the reason.

#[cfg(feature = "net")]
pub use kuula_net::config::RelayConfig;

#[cfg(feature = "net")]
mod on {
    use kuula_core::net::{identity::Identity, identity::VerifiedTransport, Link, Transport};
    use kuula_net::{IrohTransport, NetConfig};
    use std::sync::{Arc, Mutex};

    use super::RelayConfig;

    /// A permitted Iroh link. `relay` and `diagnostics` are asked per
    /// connection, so the shell can change the relay between sessions and
    /// hand each session its own diagnostics buffer.
    pub fn net_link(
        mut relay: impl FnMut() -> RelayConfig + 'static,
        mut identity: impl FnMut() -> Identity + 'static,
        mut diagnostics: impl FnMut() -> Arc<Mutex<String>> + 'static,
    ) -> Link {
        let mut link = Link::new(Box::new(move || {
            Box::new(VerifiedTransport::new(
                Box::new(IrohTransport::configured(
                    NetConfig {
                        enabled: true,
                        relay: relay(),
                        ..Default::default()
                    },
                    diagnostics(),
                )),
                identity(),
            )) as Box<dyn Transport>
        }));
        link.set_permitted(true);
        link
    }

    /// Why a networked run cannot start, if it cannot: never, here.
    pub fn unavailable() -> Option<&'static str> {
        None
    }
}

#[cfg(not(feature = "net"))]
mod off {
    use kuula_core::net::{identity::Identity, Link};
    use std::sync::{Arc, Mutex};

    const MESSAGE: &str = "this build has no networking (built without the `net` feature)";

    /// The relay options the command line still parses, so a script that
    /// passes them gets one clear error rather than an unknown flag.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct RelayConfig {
        pub url: Option<String>,
        pub only: bool,
    }

    impl RelayConfig {
        pub fn validate(&self) -> Result<(), String> {
            match self.url {
                Some(_) => Err(MESSAGE.into()),
                None => Ok(()),
            }
        }
    }

    /// A link with no transport: a permitted cart that hosts or joins is
    /// told `net_disabled`, and nothing is ever constructed.
    pub fn net_link(
        _relay: impl FnMut() -> RelayConfig + 'static,
        _identity: impl FnMut() -> Identity + 'static,
        _diagnostics: impl FnMut() -> Arc<Mutex<String>> + 'static,
    ) -> Link {
        let mut link = Link::offline();
        link.set_permitted(true);
        link
    }

    pub fn unavailable() -> Option<&'static str> {
        Some(MESSAGE)
    }
}

#[cfg(not(feature = "net"))]
pub use off::{net_link, unavailable, RelayConfig};
#[cfg(feature = "net")]
pub use on::{net_link, unavailable};

#[cfg(all(test, not(feature = "net")))]
mod tests {
    use super::*;
    use kuula_core::net::{Command, Event, FailCode};

    #[test]
    fn a_permitted_cart_that_hosts_without_networking_is_told_it_is_disabled() {
        let mut link = net_link(
            Default::default,
            || kuula_core::net::identity::Identity::new(&kuula_core::Snapshot::empty()),
            Default::default,
        );
        link.push(vec![Command::Host]);
        let mut events = Vec::new();
        link.poll(&mut events, 8);
        assert!(!link.is_live());
        assert!(
            events.iter().any(|e| matches!(
                e,
                Event::Failed {
                    code: FailCode::Disabled,
                    ..
                }
            )),
            "{events:?}"
        );
    }
}
