//! The deploy sender: connect to a receiver's ticket under
//! `kuula/deploy/1`, offer a package, wait for the answer, stream the
//! bytes, read the result. The receiver is authenticated by the endpoint
//! id in the ticket (the QUIC handshake fails against any other key); the
//! sender is identified to the receiver by the installation's
//! development key.

use iroh::endpoint::{
    ApplicationClose, ConnectError, ConnectingError, Connection, ConnectionError, ReadError,
    ReadExactError, RecvStream, SendStream, WriteError,
};
use iroh::{Endpoint, EndpointAddr, SecretKey};
use tokio::time::timeout;

use super::package::{hex, Package};
use super::wire::{self, close, DeployCode, Refusal, Restart};
use super::DeployError;
use crate::proto::{self, DEPLOY_ALPN};
use crate::{session, Code, Net, NetConfig, NetError};

/// What a receiver reported in its result frame: for a package it
/// received in full, or for one it gave up on part-way. A refusal
/// reported there is a report with a code other than `deploy_ok`, not an
/// error: the bytes, the digest and the receiver's detail are all here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushReport {
    pub name: String,
    /// Bytes of the package.
    pub bytes: u64,
    /// SHA-256 of the package, lower-case hex.
    pub digest: String,
    pub code: DeployCode,
    pub restart: Restart,
    /// The receiver's detail: the core's error code and path for
    /// `deploy_invalid`, the fault code for a `faulted` restart.
    pub detail: String,
}

/// How far a push got, derived from the code: `ok`, `failed` or
/// `skipped` for each stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stages {
    pub transfer: &'static str,
    pub validation: &'static str,
    pub install: &'static str,
}

impl PushReport {
    /// The cart is installed at the receiver.
    pub fn installed(&self) -> bool {
        self.code == DeployCode::Ok
    }

    pub fn stages(&self) -> Stages {
        let (transfer, validation, install) = match self.code {
            DeployCode::Ok => ("ok", "ok", "ok"),
            DeployCode::Invalid => ("ok", "failed", "skipped"),
            DeployCode::Install => ("ok", "ok", "failed"),
            _ => ("failed", "skipped", "skipped"),
        };
        Stages {
            transfer,
            validation,
            install,
        }
    }
}

/// Push `package` to the receiver `ticket` names, as the installation
/// holding `secret`. Blocks for at most the protocol's deadlines; a set
/// cancel flag in `config` ends it with `net_cancelled`.
pub fn push(
    config: &NetConfig,
    secret: SecretKey,
    ticket: &str,
    package: &Package,
) -> Result<PushReport, DeployError> {
    let net = Net::build(config, DEPLOY_ALPN, Some(secret))?;
    let addr = net.inner.relay.ticket_addr(ticket)?;
    let endpoint = net.inner.endpoint.clone();
    let result = net.inner.block_on(run(endpoint, addr, package))?;
    net.shutdown();
    result
}

async fn run(
    endpoint: Endpoint,
    addr: EndpointAddr,
    package: &Package,
) -> Result<PushReport, DeployError> {
    let connected = timeout(proto::CONNECT_DEADLINE, endpoint.connect(addr, DEPLOY_ALPN)).await;
    let conn = match connected {
        Ok(Ok(conn)) => conn,
        Ok(Err(e)) => return Err(mismatch_detail(connect_error(e))),
        Err(_) => {
            return Err(NetError::new(
                Code::Connect,
                format!(
                    "no answer from the receiver within {} s",
                    proto::CONNECT_DEADLINE.as_secs()
                ),
            )
            .into())
        }
    };
    let result = exchange(&conn, package).await;
    let code = if result.is_ok() {
        close::BYE
    } else {
        close::SHUTDOWN
    };
    conn.close(code.into(), close::reason(code));
    result.map_err(mismatch_detail)
}

/// The play ALPN's mismatch text names the wrong protocol here.
fn mismatch_detail(e: DeployError) -> DeployError {
    match e {
        DeployError::Net(mut n) if n.code == Code::ProtocolMismatch => {
            n.detail = format!(
                "the peer does not speak {}",
                String::from_utf8_lossy(DEPLOY_ALPN)
            );
            DeployError::Net(n)
        }
        other => other,
    }
}

async fn exchange(conn: &Connection, package: &Package) -> Result<PushReport, DeployError> {
    let len = package.bytes().len();
    let offer = wire::encode_offer(
        package.name(),
        u32::try_from(len).unwrap_or(u32::MAX),
        package.digest(),
    )?;
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| peer_closed(&e))?;
    write(&mut send, &offer, "the offer").await?;

    // The answer: one byte.
    let mut answer = [0u8; 1];
    match timeout(wire::ANSWER_DEADLINE, recv.read_exact(&mut answer)).await {
        Err(_) => {
            return Err(NetError::new(
                Code::Timeout,
                format!("no answer within {} s", wire::ANSWER_DEADLINE.as_secs()),
            )
            .into())
        }
        Ok(r) => r.map_err(read_error)?,
    }
    if answer[0] != 0 {
        return Err(match DeployCode::from_status(answer[0]) {
            Some(code) => Refusal::new(code, "the receiver refused the offer").into(),
            None => {
                NetError::new(Code::Frame, format!("unknown answer status {}", answer[0])).into()
            }
        });
    }

    // The package, then the end of the stream. A receiver that fails
    // part-way stops the stream and sends its result at once, so a
    // stopped write goes on to read that result.
    let mut sent = 0usize;
    for chunk in package.bytes().chunks(wire::CHUNK) {
        match timeout(wire::STALL_DEADLINE, send.write_all(chunk)).await {
            Ok(Err(WriteError::Stopped(_))) => break,
            Err(_) => {
                return Err(NetError::new(
                    Code::Timeout,
                    format!(
                        "the receiver took no data for {} s after {sent} of {len} bytes",
                        wire::STALL_DEADLINE.as_secs()
                    ),
                )
                .into())
            }
            Ok(r) => r.map_err(write_error)?,
        }
        sent += chunk.len();
    }
    let _ = send.finish();

    // The result.
    let waited = timeout(wire::RESULT_DEADLINE, read_result(&mut recv)).await;
    let (code, restart, detail) = match waited {
        Err(_) => {
            return Err(NetError::new(
                Code::Timeout,
                format!(
                    "no result within {} s of the last byte",
                    wire::RESULT_DEADLINE.as_secs()
                ),
            )
            .into())
        }
        Ok(r) => r?,
    };
    Ok(PushReport {
        name: package.name().to_string(),
        bytes: len as u64,
        digest: hex(package.digest()),
        code,
        restart,
        detail,
    })
}

async fn write(send: &mut SendStream, bytes: &[u8], what: &str) -> Result<(), DeployError> {
    match timeout(wire::STALL_DEADLINE, send.write_all(bytes)).await {
        Err(_) => Err(NetError::new(
            Code::Timeout,
            format!(
                "the receiver did not take {what} within {} s",
                wire::STALL_DEADLINE.as_secs()
            ),
        )
        .into()),
        Ok(r) => r.map_err(write_error),
    }
}

async fn read_result(recv: &mut RecvStream) -> Result<(DeployCode, Restart, String), DeployError> {
    let frame = |why: String| DeployError::from(NetError::new(Code::Frame, why));
    let mut header = [0u8; wire::RESULT_HEADER];
    recv.read_exact(&mut header).await.map_err(read_error)?;
    let (code, restart, n) = wire::result_header(&header).map_err(frame)?;
    let mut body = vec![0u8; n];
    recv.read_exact(&mut body).await.map_err(read_error)?;
    let detail = wire::result_detail(&body).map_err(frame)?;
    Ok((code, restart, detail))
}

// ----- error mapping ---------------------------------------------------------

/// What the receiver's close code means to the sender.
fn peer_closed(c: &ConnectionError) -> DeployError {
    if let ConnectionError::ApplicationClosed(ApplicationClose { error_code, .. }) = c {
        let code = u64::from(*error_code) as u32;
        return match code {
            close::BUSY => Refusal::new(
                DeployCode::Busy,
                "the receiver is busy with another transfer",
            )
            .into(),
            close::UNPAIRED => Refusal::new(
                DeployCode::Unpaired,
                "this installation's development id is not approved at the receiver",
            )
            .into(),
            close::SHUTDOWN => {
                Refusal::new(DeployCode::Cancelled, "the receiver is shutting down").into()
            }
            close::TIMEOUT => NetError::new(Code::Timeout, "the receiver timed out").into(),
            close::PROTOCOL => {
                NetError::new(Code::Frame, "the receiver reported a protocol violation").into()
            }
            close::BYE => NetError::new(Code::Connect, "the receiver closed the connection").into(),
            other => NetError::new(Code::Connect, format!("closed with code {other}")).into(),
        };
    }
    session::map_connection_error(c.clone()).into()
}

fn read_error(e: ReadExactError) -> DeployError {
    match e {
        ReadExactError::ReadError(ReadError::ConnectionLost(c)) => peer_closed(&c),
        ReadExactError::FinishedEarly(_) => {
            NetError::new(Code::Connect, "the receiver ended the stream early").into()
        }
        other => NetError::new(Code::Connect, other.to_string()).into(),
    }
}

fn write_error(e: WriteError) -> DeployError {
    match e {
        WriteError::ConnectionLost(c) => peer_closed(&c),
        other => NetError::new(Code::Connect, other.to_string()).into(),
    }
}

fn connect_error(e: ConnectError) -> DeployError {
    match e {
        ConnectError::Connecting { source, .. } => match source {
            ConnectingError::ConnectionError { source, .. } => peer_closed(&source),
            other => session::map_connecting_error(other).into(),
        },
        ConnectError::Connection { source, .. } => peer_closed(&source),
        other => session::map_connect_error(other).into(),
    }
}
