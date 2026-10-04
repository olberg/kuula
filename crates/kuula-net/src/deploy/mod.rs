//! The deploy channel: push a development cart from one desktop to
//! another over its own Iroh endpoint and ALPN (`kuula/deploy/1`).
//!
//! Nothing here is cart API. A play endpoint never registers the deploy
//! ALPN and a deploy endpoint never registers the play one; the deploy
//! endpoint carries the installation's persistent development key
//! ([`DeployStore`]) while play identity stays ephemeral. The receiver
//! installs only for ids on its approved list, one transfer at a time,
//! through a bounded stream into a staging file.
//!
//! - [`wire`]: the offer, the answer and the result; pure, limit-checked.
//! - [`store`]: the development key and the approved list on disk.
//! - [`package`]: a validated package and its digest.
//! - [`receiver`] and [`sender`]: the two ends.

pub mod package;
pub mod receiver;
pub mod sender;
pub mod store;
pub mod wire;

#[cfg(test)]
mod tests;

use std::fmt;

pub use iroh::SecretKey;
pub use package::Package;
pub use receiver::{
    DeployEvent, DeployReceiver, ReceiverConfig, RestartReply, RestartRequest, STAGING_DIR,
};
pub use sender::{push, PushReport, Stages};
pub use store::{DeployStore, StoreError, MAX_APPROVED};
pub use wire::{DeployCode, Refusal, Restart, MAX_PACKAGE};

use crate::NetError;

/// A deploy call that produced no result frame to report: a transport
/// failure with its `net_*` code, or a refusal with a `deploy_*` code
/// (from the local checks, the receiver's answer byte or its close code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployError {
    Net(NetError),
    Refused(Refusal),
}

impl DeployError {
    /// The stable code, `net_*` or `deploy_*`.
    pub fn code(&self) -> &'static str {
        match self {
            DeployError::Net(e) => e.code.as_str(),
            DeployError::Refused(r) => r.code.as_str(),
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            DeployError::Net(e) => &e.detail,
            DeployError::Refused(r) => &r.detail,
        }
    }
}

impl fmt::Display for DeployError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.code(), self.detail())
    }
}

impl std::error::Error for DeployError {}

impl From<NetError> for DeployError {
    fn from(e: NetError) -> DeployError {
        DeployError::Net(e)
    }
}

impl From<Refusal> for DeployError {
    fn from(r: Refusal) -> DeployError {
        DeployError::Refused(r)
    }
}

/// A failure of the key or approved-list files is an I/O failure of the
/// installation: `deploy_install`.
impl From<StoreError> for DeployError {
    fn from(e: StoreError) -> DeployError {
        DeployError::Refused(Refusal::new(DeployCode::Install, e.to_string()))
    }
}
