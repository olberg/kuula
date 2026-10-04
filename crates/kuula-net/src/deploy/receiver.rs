//! The development receiver: an endpoint under `kuula/deploy/1` that
//! installs a cart pushed by an approved developer.
//!
//! The order of a connection is fixed. The TLS-authenticated remote id
//! is checked against the approved list first, and an id that is not on
//! it is closed with code 5 before any stream is accepted or any byte
//! read. Then the one-transfer-at-a-time gate (code 1). Then the offer,
//! the answer, the package into a staging file under `<carts>/.staging/`
//! (never into memory as a whole, never more than offered), the digest,
//! the archive and manifest validation, the atomic rename over
//! `<carts>/<name>.cart`, the restart, the result. A failure before the
//! rename deletes the staged file and leaves the installed cart as it
//! was; the guard that owns the staged file deletes it even when the
//! task is dropped by a shutdown.

use std::fs;
use std::future::Future;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::endpoint::{Connection, Incoming, SendStream};
use iroh::Endpoint;
use iroh_tickets::endpoint::EndpointTicket;
use sha2::{Digest, Sha256};
use tokio::sync::{oneshot, watch};
use tokio::time::{timeout, timeout_at, Instant as TokioInstant};

use super::package::{check_package_file, hex};
use super::store::DeployStore;
use super::wire::{self, close, DeployCode, Offer, Refusal, Restart};
use super::DeployError;
use crate::proto::DEPLOY_ALPN;
use crate::{Net, NetConfig};

/// Connections handled at once, counting those still in their TLS
/// handshake; more are refused at the QUIC level.
const MAX_CONNECTIONS: usize = 16;

/// How long a connection may take to finish its TLS handshake.
const HANDSHAKE_BOUND: Duration = Duration::from_secs(10);

/// How long a shutdown waits for transfers to end on their own.
const SHUTDOWN_WAIT: Duration = Duration::from_millis(1500);

/// Events a receiver holds for its caller; later ones are dropped.
const EVENT_QUEUE: usize = 64;

/// At most one `Unpaired` event this often.
const UNPAIRED_REPORT: Duration = Duration::from_secs(1);

/// The staging directory under a carts directory.
pub const STAGING_DIR: &str = ".staging";

/// What a receiver needs.
pub struct ReceiverConfig {
    /// Where installed carts go, as `<name>.cart`.
    pub carts: PathBuf,
    /// The installation's key and approved list.
    pub store: DeployStore,
    /// `enabled` must be set by the caller that decided to open a socket.
    pub net: NetConfig,
    /// Where an install is handed to a console. `None`: the receiver has
    /// no console and reports `not_run`.
    pub restart: Option<mpsc::Sender<RestartRequest>>,
}

/// Ask the console to load an installed cart; answer through `reply`.
pub struct RestartRequest {
    /// The cart's name as deployed.
    pub name: String,
    /// The installed file, `<carts>/<name>.cart`.
    pub path: PathBuf,
    pub reply: RestartReply,
}

/// The console's answer to a [`RestartRequest`]. Dropping it unanswered
/// is a `timeout` at the sender.
pub struct RestartReply(oneshot::Sender<(Restart, String)>);

impl RestartReply {
    /// Report how the load went; `detail` carries a fault code.
    pub fn send(self, restart: Restart, detail: impl Into<String>) {
        let _ = self.0.send((restart, detail.into()));
    }
}

/// What a receiver reports, one event at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployEvent {
    /// A connection from an id that is not approved was closed. Reported
    /// at most once a second, so a person at the receiver can approve it.
    Unpaired { id: String },
    /// A second paired connection arrived during a transfer.
    Busy { id: String },
    /// An offer passed its checks and was accepted.
    Offered {
        id: String,
        name: String,
        bytes: u32,
    },
    /// A transfer attempt ended, installed or not.
    Finished {
        id: String,
        /// Empty when the offer never named one.
        name: String,
        bytes: u64,
        code: DeployCode,
        restart: Restart,
        detail: String,
    },
}

/// Everything the connection tasks share.
struct Ctx {
    carts: PathBuf,
    staging: PathBuf,
    store: DeployStore,
    restart: Option<mpsc::Sender<RestartRequest>>,
    events: mpsc::SyncSender<DeployEvent>,
    /// A transfer is in progress.
    active: AtomicBool,
    next_part: AtomicU64,
    /// Connection tasks alive.
    live: AtomicUsize,
    unpaired: Limiter,
    stop: watch::Receiver<bool>,
}

impl Ctx {
    fn emit(&self, event: DeployEvent) {
        let _ = self.events.try_send(event);
    }

    fn report_unpaired(&self, id: &str) {
        if self.unpaired.allow(Instant::now()) {
            self.emit(DeployEvent::Unpaired { id: id.to_string() });
        }
    }
}

/// Lets something happen at most once per interval.
struct Limiter {
    every: Duration,
    last: Mutex<Option<Instant>>,
}

impl Limiter {
    fn new(every: Duration) -> Limiter {
        Limiter {
            every,
            last: Mutex::new(None),
        }
    }

    fn allow(&self, now: Instant) -> bool {
        let mut last = self.last.lock().unwrap();
        if last.is_some_and(|t| now.duration_since(t) < self.every) {
            return false;
        }
        *last = Some(now);
        true
    }
}

/// A running receiver. Dropping it, or [`DeployReceiver::shutdown`],
/// cancels a transfer in flight, closes the endpoint and empties the
/// staging directory.
pub struct DeployReceiver {
    net: Option<Net>,
    ctx: Arc<Ctx>,
    stop: watch::Sender<bool>,
    events: mpsc::Receiver<DeployEvent>,
    ticket: String,
    id: String,
    addresses: Vec<SocketAddr>,
}

impl std::fmt::Debug for DeployReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DeployReceiver({})", self.id)
    }
}

impl DeployReceiver {
    /// Empty the staging directory, bind the endpoint with the
    /// development key and start accepting.
    pub fn start(config: ReceiverConfig) -> Result<DeployReceiver, DeployError> {
        let staging = config.carts.join(STAGING_DIR);
        fs::create_dir_all(&staging).map_err(|e| {
            Refusal::new(
                DeployCode::Install,
                format!("cannot create {}: {e}", staging.display()),
            )
        })?;
        clear_staging(&staging);
        let secret = config.store.secret_key()?;
        let net = Net::build(&config.net, DEPLOY_ALPN, Some(secret))?;
        let inner = &net.inner;
        let addr = inner.block_on(crate::direct_addr(&inner.endpoint, &inner.relay))??;
        let addresses: Vec<SocketAddr> = addr.ip_addrs().copied().collect();
        let ticket = EndpointTicket::new(addr).to_string();

        let (stop_tx, stop_rx) = watch::channel(false);
        let (event_tx, events) = mpsc::sync_channel(EVENT_QUEUE);
        let ctx = Arc::new(Ctx {
            carts: config.carts,
            staging,
            store: config.store,
            restart: config.restart,
            events: event_tx,
            active: AtomicBool::new(false),
            next_part: AtomicU64::new(0),
            live: AtomicUsize::new(0),
            unpaired: Limiter::new(UNPAIRED_REPORT),
            stop: stop_rx,
        });
        inner
            .handle
            .spawn(accept_loop(inner.endpoint.clone(), ctx.clone()));
        let id = net.id();
        Ok(DeployReceiver {
            net: Some(net),
            ctx,
            stop: stop_tx,
            events,
            ticket,
            id,
            addresses,
        })
    }

    /// The ticket to hand to a sender.
    pub fn ticket(&self) -> &str {
        &self.ticket
    }

    /// The receiver's endpoint id: the installation's development id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The direct addresses the ticket carries.
    pub fn addresses(&self) -> &[SocketAddr] {
        &self.addresses
    }

    /// The next event, or `None` when `wait` passed first.
    pub fn next_event(&self, wait: Duration) -> Option<DeployEvent> {
        self.events.recv_timeout(wait).ok()
    }

    /// The next event if one is queued.
    pub fn try_event(&self) -> Option<DeployEvent> {
        self.events.try_recv().ok()
    }

    /// Cancel, close and clean up; the same as dropping.
    pub fn shutdown(self) {}
}

impl Drop for DeployReceiver {
    fn drop(&mut self) {
        // Tell a transfer to cancel, give it a moment to answer and
        // delete its staged file, then stop the runtime; whatever a
        // task could not clean up goes with the directory sweep.
        let _ = self.stop.send(true);
        let began = Instant::now();
        while self.ctx.live.load(Ordering::SeqCst) > 0 && began.elapsed() < SHUTDOWN_WAIT {
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(self.net.take());
        clear_staging(&self.ctx.staging);
    }
}

/// Remove everything under a staging directory.
pub(crate) fn clear_staging(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = fs::remove_dir_all(&path);
        } else {
            let _ = fs::remove_file(&path);
        }
    }
}

// ----- connections -----------------------------------------------------------

struct LiveGuard(Arc<Ctx>);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::SeqCst);
    }
}

struct ActiveGuard<'a>(&'a AtomicBool);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

async fn accept_loop(endpoint: Endpoint, ctx: Arc<Ctx>) {
    while let Some(incoming) = endpoint.accept().await {
        if ctx.live.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            incoming.refuse();
            continue;
        }
        ctx.live.fetch_add(1, Ordering::SeqCst);
        let guard = LiveGuard(ctx.clone());
        tokio::spawn(async move {
            handle(incoming, &guard.0).await;
        });
    }
}

/// Run `fut` unless the receiver is told to stop first.
async fn or_stop<T>(stop: &mut watch::Receiver<bool>, fut: impl Future<Output = T>) -> Option<T> {
    tokio::select! {
        biased;
        _ = stop.changed() => None,
        v = fut => Some(v),
    }
}

async fn handle(incoming: Incoming, ctx: &Ctx) {
    let Ok(accepting) = incoming.accept() else {
        return;
    };
    let mut stop = ctx.stop.clone();
    let conn = match or_stop(&mut stop, timeout(HANDSHAKE_BOUND, accepting)).await {
        Some(Ok(Ok(conn))) => conn,
        _ => return,
    };
    // The offer's deadline runs from the connection, not from the
    // handshake that made it, which has a bound of its own.
    let connected = TokioInstant::now();
    let id = conn.remote_id().to_string();
    // Pairing first: nothing of this connection is read or accepted.
    if !ctx.store.is_approved(&id) {
        ctx.report_unpaired(&id);
        conn.close(close::UNPAIRED.into(), close::reason(close::UNPAIRED));
        return;
    }
    if ctx.active.swap(true, Ordering::SeqCst) {
        ctx.emit(DeployEvent::Busy { id });
        conn.close(close::BUSY.into(), close::reason(close::BUSY));
        return;
    }
    let _active = ActiveGuard(&ctx.active);
    let done = transfer(&conn, ctx, &id, connected + wire::OFFER_DEADLINE).await;
    ctx.emit(DeployEvent::Finished {
        id,
        name: done.name,
        bytes: done.bytes,
        code: done.code,
        restart: done.restart,
        detail: done.detail,
    });
}

/// How one transfer attempt ended.
struct Done {
    name: String,
    bytes: u64,
    code: DeployCode,
    restart: Restart,
    detail: String,
}

impl Done {
    fn fail(name: &str, bytes: u64, code: DeployCode, detail: impl Into<String>) -> Done {
        Done {
            name: name.to_string(),
            bytes,
            code,
            restart: Restart::NotRun,
            detail: detail.into(),
        }
    }
}

/// A staged file that deletes itself unless it was installed.
struct Staged {
    path: PathBuf,
    file: Option<fs::File>,
    installed: bool,
}

impl Drop for Staged {
    fn drop(&mut self) {
        // The handle goes first: Windows will not delete an open file.
        self.file = None;
        if !self.installed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Send the result, then wait for the sender to close.
async fn finish_with(conn: &Connection, send: &mut SendStream, done: &Done, linger: Duration) {
    let frame = wire::encode_result(done.code, done.restart, &done.detail);
    let _ = timeout(wire::STALL_DEADLINE, send.write_all(&frame)).await;
    let _ = send.finish();
    let _ = timeout(linger, conn.closed()).await;
    conn.close(close::BYE.into(), close::reason(close::BYE));
}

/// Refuse an offer with its status byte; no result frame follows.
async fn refuse(conn: &Connection, send: &mut SendStream, name: &str, r: Refusal) -> Done {
    let status = r.code.status().unwrap_or(1);
    let _ = timeout(wire::STALL_DEADLINE, send.write_all(&[status])).await;
    let _ = send.finish();
    let _ = timeout(wire::LINGER, conn.closed()).await;
    conn.close(close::BYE.into(), close::reason(close::BYE));
    Done::fail(name, 0, r.code, r.detail)
}

async fn transfer(conn: &Connection, ctx: &Ctx, id: &str, deadline: TokioInstant) -> Done {
    let mut stop = ctx.stop.clone();

    // The offer, within its deadline of the connection.
    let opened = or_stop(&mut stop, timeout_at(deadline, conn.accept_bi())).await;
    let (mut send, mut recv) = match opened {
        None => return cancelled_before_offer(conn),
        Some(Err(_)) => return no_offer(conn),
        Some(Ok(Err(e))) => {
            return Done::fail("", 0, DeployCode::Offer, format!("no offer: {e}"));
        }
        Some(Ok(Ok(streams))) => streams,
    };
    let offer = match or_stop(&mut stop, timeout_at(deadline, read_offer(&mut recv))).await {
        None => return cancelled_before_offer(conn),
        Some(Err(_)) => return no_offer(conn),
        Some(Ok(Err(OfferError::Lost(why)))) => {
            return Done::fail("", 0, DeployCode::Offer, why);
        }
        Some(Ok(Err(OfferError::Refused(r)))) => return refuse(conn, &mut send, "", r).await,
        Some(Ok(Ok(offer))) => offer,
    };
    let name = offer.name.clone();

    // A staging file, then the answer.
    let part = ctx.staging.join(format!(
        "incoming-{}.part",
        ctx.next_part.fetch_add(1, Ordering::SeqCst)
    ));
    let created = fs::create_dir_all(&ctx.staging).and_then(|()| {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
    });
    let mut staged = match created {
        Ok(file) => Staged {
            path: part,
            file: Some(file),
            installed: false,
        },
        Err(e) => {
            let r = Refusal::new(
                DeployCode::Install,
                format!("cannot create a staging file: {e}"),
            );
            return refuse(conn, &mut send, &name, r).await;
        }
    };
    if timeout(wire::STALL_DEADLINE, send.write_all(&[0]))
        .await
        .is_err()
    {
        return Done::fail(
            &name,
            0,
            DeployCode::Interrupted,
            "the sender took no answer",
        );
    }
    ctx.emit(DeployEvent::Offered {
        id: id.to_string(),
        name: name.clone(),
        bytes: offer.len,
    });

    // The package, hashed as it arrives.
    let mut hasher = Sha256::new();
    let mut got = 0u64;
    let received = receive_data(
        &mut recv,
        &mut staged,
        offer.len,
        &mut hasher,
        &mut got,
        &mut stop,
    )
    .await;
    if let Err(r) = received {
        drop(staged);
        // A sender still writing would wait on flow control for a reader
        // that has gone, and never read the result: stop it first.
        let _ = recv.stop(0u32.into());
        let done = Done::fail(&name, got, r.code, r.detail);
        if r.code == DeployCode::Cancelled {
            finish_with(conn, &mut send, &done, Duration::from_millis(500)).await;
        } else {
            finish_with(conn, &mut send, &done, wire::LINGER).await;
        }
        return done;
    }

    // Verify, validate, install, restart.
    let done = verify_and_install(ctx, &mut staged, &offer, hasher).await;
    drop(staged);
    let done = Done {
        name,
        bytes: got,
        ..done
    };
    finish_with(conn, &mut send, &done, wire::LINGER).await;
    done
}

fn no_offer(conn: &Connection) -> Done {
    conn.close(close::TIMEOUT.into(), close::reason(close::TIMEOUT));
    Done::fail(
        "",
        0,
        DeployCode::Offer,
        format!("no offer within {} s", wire::OFFER_DEADLINE.as_secs()),
    )
}

fn cancelled_before_offer(conn: &Connection) -> Done {
    conn.close(close::SHUTDOWN.into(), close::reason(close::SHUTDOWN));
    Done::fail(
        "",
        0,
        DeployCode::Cancelled,
        "the receiver is shutting down",
    )
}

enum OfferError {
    Refused(Refusal),
    Lost(String),
}

impl From<Refusal> for OfferError {
    fn from(r: Refusal) -> OfferError {
        OfferError::Refused(r)
    }
}

fn lost(e: iroh::endpoint::ReadExactError) -> OfferError {
    OfferError::Lost(format!("the offer ended early: {e}"))
}

/// Read and check an offer; each part is checked before the next is
/// read, so nothing is allocated for an offer that cannot be valid.
async fn read_offer(recv: &mut iroh::endpoint::RecvStream) -> Result<Offer, OfferError> {
    let mut prefix = [0u8; wire::OFFER_PREFIX];
    recv.read_exact(&mut prefix).await.map_err(lost)?;
    let n = wire::offer_prefix(&prefix)?;
    let mut name = [0u8; wire::MAX_NAME];
    recv.read_exact(&mut name[..n]).await.map_err(lost)?;
    let name = wire::offer_name(&name[..n])?;
    let mut tail = [0u8; wire::OFFER_TAIL];
    recv.read_exact(&mut tail).await.map_err(lost)?;
    Ok(wire::offer_tail(name, &tail)?)
}

/// Read exactly `expect` bytes into the staged file, hashing them, then
/// require the stream to end.
async fn receive_data(
    recv: &mut iroh::endpoint::RecvStream,
    staged: &mut Staged,
    expect: u32,
    hasher: &mut Sha256,
    got: &mut u64,
    stop: &mut watch::Receiver<bool>,
) -> Result<(), Refusal> {
    let interrupted = |why: String| Refusal::new(DeployCode::Interrupted, why);
    let expect = u64::from(expect);
    let mut buf = vec![0u8; wire::CHUNK];
    while *got < expect {
        let want = (expect - *got).min(wire::CHUNK as u64) as usize;
        let read = match or_stop(
            stop,
            timeout(wire::STALL_DEADLINE, recv.read(&mut buf[..want])),
        )
        .await
        {
            None => {
                return Err(Refusal::new(
                    DeployCode::Cancelled,
                    "the receiver is shutting down",
                ))
            }
            Some(r) => r,
        };
        let n = match read {
            Err(_) => {
                return Err(interrupted(format!(
                    "no data for {} s after {got} of {expect} bytes",
                    wire::STALL_DEADLINE.as_secs()
                )))
            }
            Ok(Err(e)) => return Err(interrupted(format!("after {got} of {expect} bytes: {e}"))),
            Ok(Ok(None)) => {
                return Err(interrupted(format!(
                    "the sender stopped after {got} of {expect} bytes"
                )))
            }
            Ok(Ok(Some(n))) => n,
        };
        hasher.update(&buf[..n]);
        let file = staged.file.as_mut().expect("the staged file is open");
        file.write_all(&buf[..n]).map_err(|e| {
            Refusal::new(
                DeployCode::Install,
                format!("cannot write the staged file: {e}"),
            )
        })?;
        *got += n as u64;
    }
    // Nothing may follow the package: the sender finishes its stream.
    match timeout(wire::STALL_DEADLINE, recv.read(&mut buf[..1])).await {
        Ok(Ok(None)) => Ok(()),
        Ok(Ok(Some(_))) => Err(Refusal::new(
            DeployCode::Offer,
            "data after the offered package",
        )),
        Ok(Err(e)) => Err(interrupted(format!("at the end of the package: {e}"))),
        Err(_) => Err(interrupted("the sender did not finish its stream".into())),
    }
}

/// After the last byte: the digest, the archive and manifest checks, the
/// atomic replace and the restart. The result has no name or byte count
/// yet; the caller adds them.
async fn verify_and_install(ctx: &Ctx, staged: &mut Staged, offer: &Offer, hasher: Sha256) -> Done {
    let fail = |code: DeployCode, detail: String| Done::fail("", 0, code, detail);
    let got: [u8; 32] = hasher.finalize().into();
    if got != offer.digest {
        return fail(
            DeployCode::Digest,
            format!("offered {}, received {}", hex(&offer.digest), hex(&got)),
        );
    }
    if let Some(file) = staged.file.take() {
        if let Err(e) = file.sync_all() {
            return fail(
                DeployCode::Install,
                format!("cannot flush the staged file: {e}"),
            );
        }
    }
    if let Err(detail) = check_package_file(&staged.path) {
        return fail(DeployCode::Invalid, detail);
    }
    let dest = ctx.carts.join(format!("{}.cart", offer.name));
    if let Err(e) = fs::rename(&staged.path, &dest) {
        return fail(
            DeployCode::Install,
            format!("cannot replace {}: {e}", dest.display()),
        );
    }
    staged.installed = true;
    let (restart, detail) = restart(ctx, &offer.name, dest).await;
    Done {
        name: String::new(),
        bytes: 0,
        code: DeployCode::Ok,
        restart,
        detail,
    }
}

/// Hand the install to the console and wait for its answer.
async fn restart(ctx: &Ctx, name: &str, path: PathBuf) -> (Restart, String) {
    let Some(tx) = &ctx.restart else {
        return (Restart::NotRun, String::new());
    };
    let (reply, answer) = oneshot::channel();
    let request = RestartRequest {
        name: name.to_string(),
        path,
        reply: RestartReply(reply),
    };
    if tx.send(request).is_err() {
        return (Restart::NotRun, "the console has gone".into());
    }
    match timeout(wire::RESTART_DEADLINE, answer).await {
        Ok(Ok(outcome)) => outcome,
        _ => (Restart::Timeout, String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unpaired_report_is_limited_to_one_a_second() {
        let limiter = Limiter::new(UNPAIRED_REPORT);
        let t0 = Instant::now();
        assert!(limiter.allow(t0));
        assert!(!limiter.allow(t0));
        assert!(!limiter.allow(t0 + Duration::from_millis(999)));
        assert!(limiter.allow(t0 + Duration::from_millis(1000)));
        assert!(!limiter.allow(t0 + Duration::from_millis(1500)));
    }
}
