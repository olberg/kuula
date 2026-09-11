//! Host-side networking for Kuula over Iroh: the runtime, the endpoint
//! and wire protocol version 1 (`docs/net.md`).
//!
//! Nothing here is reachable from a cart. `kuula-cli` builds a [`Net`]
//! only inside its `net` diagnostic; `kuula run`, the shell, the MCP
//! server and the worker never construct one, and the worker is denied
//! the network by the OS besides.
//!
//! The async lives inside this crate. A [`Net`] owns one current-thread
//! tokio runtime driven by one dedicated thread, so a session keeps
//! answering pings and notices a lost peer while the caller is idle;
//! every public call is blocking with a bound, and dropping the `Net`
//! stops the thread.

pub mod proto;
mod session;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_transport;
pub mod transport;

use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use iroh::endpoint::{presets, IdleTimeout, QuicTransportConfig};
use iroh::{Endpoint, RelayMode, Watcher};
use iroh_tickets::endpoint::EndpointTicket;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::oneshot;

pub use session::{Event, Session};
pub use transport::IrohTransport;

/// The runtime version string carried in the handshake.
pub const RUNTIME_VERSION: &str = concat!("kuula ", env!("CARGO_PKG_VERSION"));

/// How long shutdown waits for the endpoint to close, and then for the
/// runtime to stop.
pub const SHUTDOWN_BOUND: Duration = Duration::from_secs(2);

/// Stable error codes; host-side only, never a cart fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    Disabled,
    NoAddress,
    Ticket,
    Connect,
    ProtocolMismatch,
    Handshake,
    Frame,
    Timeout,
    Busy,
    Cancelled,
    /// The outgoing queue had no room for a non-blocking send.
    QueueFull,
}

impl Code {
    pub const ALL: [Code; 11] = [
        Code::Disabled,
        Code::NoAddress,
        Code::Ticket,
        Code::Connect,
        Code::ProtocolMismatch,
        Code::Handshake,
        Code::Frame,
        Code::Timeout,
        Code::Busy,
        Code::Cancelled,
        Code::QueueFull,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Code::Disabled => "net_disabled",
            Code::NoAddress => "net_no_address",
            Code::Ticket => "net_ticket",
            Code::Connect => "net_connect",
            Code::ProtocolMismatch => "net_protocol_mismatch",
            Code::Handshake => "net_handshake",
            Code::Frame => "net_frame",
            Code::Timeout => "net_timeout",
            Code::Busy => "net_busy",
            Code::Cancelled => "net_cancelled",
            Code::QueueFull => "net_queue_full",
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A failure: the code and a human-readable detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetError {
    pub code: Code,
    pub detail: String,
}

impl NetError {
    pub fn new(code: Code, detail: impl Into<String>) -> NetError {
        NetError {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.code, self.detail)
    }
}

impl std::error::Error for NetError {}

/// What a `Net` needs. There is no default-on path: `enabled` must be
/// set by the caller that decided to open a socket.
#[derive(Debug, Clone, Default)]
pub struct NetConfig {
    pub enabled: bool,
    /// Bind one explicit address instead of every interface; tests use
    /// `127.0.0.1:0`.
    pub bind: Option<SocketAddr>,
    /// A flag the caller sets to cancel: every blocking call except a
    /// session's `close` returns `net_cancelled` within
    /// [`CANCEL_POLL`] of it being set, so a Ctrl+C handler that sets
    /// it unblocks a connect, a handshake wait, a ping or a full queue.
    pub cancel: Option<Arc<AtomicBool>>,
}

/// How often a blocking call checks the cancel flag.
pub const CANCEL_POLL: Duration = Duration::from_millis(50);

/// How long a shutdown waits for the endpoint to close once the cancel
/// flag is set. Iroh's close waits about 3 s for a failed connection
/// attempt to drain, which a cancelled caller must not pay.
pub const CANCELLED_CLOSE_BOUND: Duration = Duration::from_millis(500);

/// The slot a waiting listener leaves for the accept loop: the one
/// joiner it admits is handed over through it.
type Waiting = Arc<Mutex<Option<oneshot::Sender<Result<session::Parts, NetError>>>>>;

/// The runtime, its driver thread and the endpoint. Handed to the
/// listener and sessions made from it, so they keep it alive.
struct Inner {
    handle: Handle,
    endpoint: Endpoint,
    rt: Mutex<Option<Arc<Runtime>>>,
    driver: Mutex<Option<JoinHandle<()>>>,
    stop: Mutex<Option<oneshot::Sender<()>>>,
    /// The listener waiting for a session, if any; a joiner that finds
    /// none is refused as busy.
    waiting: Waiting,
    accepting: AtomicBool,
    closed: AtomicBool,
    cancel: Option<Arc<AtomicBool>>,
    /// Sessions alive on this endpoint; a listener holds one at a time,
    /// so `listen` and `rearm` refuse while it is above zero.
    sessions: std::sync::atomic::AtomicU32,
}

impl Inner {
    /// Run a future to completion from the caller's thread while the
    /// driver thread turns the I/O and timer drivers; `net_cancelled`
    /// once the caller's cancel flag is set.
    fn block_on<F: std::future::Future>(&self, fut: F) -> Result<F::Output, NetError> {
        self.run(fut, true)
    }

    /// The same under a deadline; the timer is created inside the
    /// runtime context, which `tokio::time::timeout` needs.
    fn timed<F: std::future::Future>(
        &self,
        wait: Duration,
        fut: F,
    ) -> Result<Result<F::Output, tokio::time::error::Elapsed>, NetError> {
        self.run(async move { tokio::time::timeout(wait, fut).await }, true)
    }

    /// A deadline the cancel flag does not cut short: the `Bye` and
    /// the wait for the task after it, so a cancelled session still
    /// leaves cleanly.
    fn timed_final<F: std::future::Future>(
        &self,
        wait: Duration,
        fut: F,
    ) -> Result<Result<F::Output, tokio::time::error::Elapsed>, NetError> {
        self.run(async move { tokio::time::timeout(wait, fut).await }, false)
    }

    fn run<F: std::future::Future>(
        &self,
        fut: F,
        cancellable: bool,
    ) -> Result<F::Output, NetError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(NetError::new(
                Code::Connect,
                "the network has been shut down",
            ));
        }
        let flag = match (&self.cancel, cancellable) {
            (Some(flag), true) => flag.clone(),
            _ => return Ok(self.handle.block_on(fut)),
        };
        if flag.load(Ordering::SeqCst) {
            return Err(cancelled());
        }
        self.handle.block_on(async move {
            tokio::pin!(fut);
            let mut tick = tokio::time::interval(CANCEL_POLL);
            tick.tick().await;
            loop {
                tokio::select! {
                    out = &mut fut => return Ok(out),
                    _ = tick.tick() => {
                        if flag.load(Ordering::SeqCst) {
                            return Err(cancelled());
                        }
                    }
                }
            }
        })
    }

    fn shutdown(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(stop) = self.stop.lock().unwrap().take() {
            let _ = stop.send(());
        }
        if let Some(driver) = self.driver.lock().unwrap().take() {
            let _ = driver.join();
        }
        if let Some(rt) = self.rt.lock().unwrap().take() {
            // The driver thread held the only other clone and has been
            // joined, so this always unwraps.
            if let Ok(rt) = Arc::try_unwrap(rt) {
                rt.shutdown_timeout(SHUTDOWN_BOUND);
            }
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn cancelled() -> NetError {
    NetError::new(Code::Cancelled, "cancelled by the caller")
}

/// One endpoint with its own runtime. Dropping it, or [`Net::shutdown`],
/// closes the endpoint and stops the runtime within a bound.
pub struct Net {
    inner: Arc<Inner>,
}

impl fmt::Debug for Net {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Net({})", self.inner.endpoint.id())
    }
}

impl Net {
    /// Bind an endpoint speaking [`proto::ALPN`]: relays disabled, no
    /// address lookup, a fresh key.
    pub fn new(config: &NetConfig) -> Result<Net, NetError> {
        Net::with_alpn(config, proto::ALPN)
    }

    /// The same under another ALPN; tests use it to force a mismatch.
    pub(crate) fn with_alpn(config: &NetConfig, alpn: &[u8]) -> Result<Net, NetError> {
        if !config.enabled {
            return Err(NetError::new(Code::Disabled, "networking is not enabled"));
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .thread_name("kuula-net")
            .build()
            .map_err(|e| NetError::new(Code::Connect, format!("cannot start runtime: {e}")))?;
        let rt = Arc::new(rt);
        let endpoint = rt.block_on(bind(config, alpn))?;

        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let driver_rt = rt.clone();
        let driver_ep = endpoint.clone();
        let driver_cancel = config.cancel.clone();
        let driver = std::thread::Builder::new()
            .name("kuula-net".into())
            .spawn(move || {
                driver_rt.block_on(async move {
                    let _ = stop_rx.await;
                    // A cancelled caller is leaving: a connection attempt
                    // it abandoned has nobody on the other end to drain
                    // for, so the close waits only briefly.
                    let cancelled = driver_cancel.is_some_and(|f| f.load(Ordering::SeqCst));
                    let bound = if cancelled {
                        CANCELLED_CLOSE_BOUND
                    } else {
                        SHUTDOWN_BOUND
                    };
                    let _ = tokio::time::timeout(bound, driver_ep.close()).await;
                });
            })
            .map_err(|e| NetError::new(Code::Connect, format!("cannot start thread: {e}")))?;

        Ok(Net {
            inner: Arc::new(Inner {
                handle: rt.handle().clone(),
                endpoint,
                rt: Mutex::new(Some(rt)),
                driver: Mutex::new(Some(driver)),
                stop: Mutex::new(Some(stop_tx)),
                waiting: Arc::new(Mutex::new(None)),
                accepting: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                cancel: config.cancel.clone(),
                sessions: std::sync::atomic::AtomicU32::new(0),
            }),
        })
    }

    /// This endpoint's id.
    pub fn id(&self) -> String {
        self.inner.endpoint.id().to_string()
    }

    /// The addresses the endpoint bound.
    pub fn bound(&self) -> Vec<SocketAddr> {
        self.inner.endpoint.bound_sockets()
    }

    /// Wait for a direct address and hand out a ticket. One joiner is
    /// handed to the listener; others are refused as busy.
    pub fn listen(&self) -> Result<Listener, NetError> {
        let inner = &self.inner;
        {
            let mut waiting = inner.waiting.lock().unwrap();
            if waiting.as_ref().is_some_and(|w| !w.is_closed()) {
                return Err(NetError::new(Code::Busy, "a listener is already waiting"));
            }
            if inner.sessions.load(Ordering::SeqCst) > 0 {
                return Err(NetError::new(Code::Busy, "a session is open"));
            }
            *waiting = None;
        }
        let addr = inner.block_on(direct_addr(&inner.endpoint))??;
        let addresses: Vec<SocketAddr> = addr.ip_addrs().copied().collect();
        let ticket = EndpointTicket::new(addr).to_string();
        if !inner.accepting.swap(true, Ordering::SeqCst) {
            inner
                .handle
                .spawn(accept_loop(inner.endpoint.clone(), inner.waiting.clone()));
        }
        let (tx, rx) = oneshot::channel();
        *inner.waiting.lock().unwrap() = Some(tx);
        Ok(Listener {
            inner: inner.clone(),
            ticket,
            addresses,
            rx,
        })
    }

    /// Connect to a ticket and complete the handshake.
    pub fn join(&self, ticket: &str) -> Result<Session, NetError> {
        let (addr, endpoint) = self.join_target(ticket)?;
        let inner = &self.inner;
        let parts = inner.block_on(connect(endpoint, addr))??;
        Ok(Session::new(inner.clone(), parts))
    }

    /// Start a join without waiting: the connect and the handshake run
    /// on the runtime and [`Joining::poll`] reports the outcome. Dropping
    /// the `Joining` abandons the attempt.
    pub fn join_start(&self, ticket: &str) -> Result<Joining, NetError> {
        let (addr, endpoint) = self.join_target(ticket)?;
        let (tx, rx) = oneshot::channel();
        let task = self.inner.handle.spawn(async move {
            let _ = tx.send(connect(endpoint, addr).await);
        });
        Ok(Joining {
            inner: self.inner.clone(),
            rx,
            task: Some(task),
        })
    }

    /// Parse a ticket into what a connect needs.
    fn join_target(&self, ticket: &str) -> Result<(iroh::EndpointAddr, Endpoint), NetError> {
        let ticket: EndpointTicket = ticket
            .trim()
            .parse()
            .map_err(|e| NetError::new(Code::Ticket, format!("cannot parse ticket: {e}")))?;
        let addr = ticket.endpoint_addr().clone();
        if addr.ip_addrs().next().is_none() {
            return Err(NetError::new(
                Code::Ticket,
                "the ticket carries no direct address",
            ));
        }
        Ok((addr, self.inner.endpoint.clone()))
    }

    /// Close the endpoint and stop the runtime, each within
    /// [`SHUTDOWN_BOUND`]. `Drop` does the same.
    pub fn shutdown(self) {
        self.inner.shutdown();
    }
}

impl Drop for Net {
    fn drop(&mut self) {
        self.inner.shutdown();
    }
}

/// Connect under the deadline and complete the joiner's handshake.
async fn connect(endpoint: Endpoint, addr: iroh::EndpointAddr) -> Result<session::Parts, NetError> {
    let conn =
        match tokio::time::timeout(proto::CONNECT_DEADLINE, endpoint.connect(addr, proto::ALPN))
            .await
        {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => return Err(session::map_connect_error(e)),
            Err(_) => {
                return Err(NetError::new(
                    Code::Connect,
                    format!(
                        "no answer from the peer within {} s",
                        proto::CONNECT_DEADLINE.as_secs()
                    ),
                ))
            }
        };
    session::establish(conn, session::Side::Joiner).await
}

/// A join in progress; see [`Net::join_start`].
pub struct Joining {
    inner: Arc<Inner>,
    rx: oneshot::Receiver<Result<session::Parts, NetError>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl fmt::Debug for Joining {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Joining")
    }
}

impl Joining {
    /// The outcome if the attempt has finished, without waiting.
    pub fn poll(&mut self) -> Option<Result<Session, NetError>> {
        match self.rx.try_recv() {
            Ok(Ok(parts)) => Some(Ok(Session::new(self.inner.clone(), parts))),
            Ok(Err(e)) => Some(Err(e)),
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => {
                Some(Err(NetError::new(Code::Connect, "the join was abandoned")))
            }
        }
    }
}

impl Drop for Joining {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// The endpoint, configured as `docs/net.md` says: no relay, no lookup.
async fn bind(config: &NetConfig, alpn: &[u8]) -> Result<Endpoint, NetError> {
    let idle = IdleTimeout::try_from(proto::IDLE_TIMEOUT)
        .map_err(|e| NetError::new(Code::Connect, format!("idle timeout: {e}")))?;
    let transport = QuicTransportConfig::builder()
        .max_idle_timeout(Some(idle))
        .keep_alive_interval(proto::KEEP_ALIVE)
        .max_concurrent_bidi_streams(1u32.into())
        .max_concurrent_uni_streams(0u32.into())
        .build();
    let mut builder = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .alpns(vec![alpn.to_vec()])
        .transport_config(transport);
    if let Some(addr) = config.bind {
        builder = builder
            .clear_ip_transports()
            .bind_addr(addr)
            .map_err(|e| NetError::new(Code::Connect, format!("bad bind address: {e}")))?;
    }
    builder
        .bind()
        .await
        .map_err(|e| NetError::new(Code::Connect, format!("cannot bind endpoint: {e}")))
}

/// The endpoint's address once it has at least one direct address, or
/// `net_no_address` after [`proto::ADDRESS_DEADLINE`].
async fn direct_addr(endpoint: &Endpoint) -> Result<iroh::EndpointAddr, NetError> {
    let mut watch = endpoint.watch_addr();
    let deadline = tokio::time::Instant::now() + proto::ADDRESS_DEADLINE;
    loop {
        let addr = watch.get();
        if addr.ip_addrs().next().is_some() {
            return Ok(addr);
        }
        match tokio::time::timeout_at(deadline, watch.updated()).await {
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => {
                return Err(NetError::new(
                    Code::NoAddress,
                    "the endpoint stopped before reporting an address",
                ))
            }
            Err(_) => {
                return Err(NetError::new(
                    Code::NoAddress,
                    format!(
                        "no direct address within {} s",
                        proto::ADDRESS_DEADLINE.as_secs()
                    ),
                ))
            }
        }
    }
}

/// Accept connections for the life of the endpoint: the first while a
/// listener waits becomes its session, every other is closed as busy.
/// A slot whose `Listener` has been dropped counts as no listener.
async fn accept_loop(endpoint: Endpoint, waiting: Waiting) {
    while let Some(incoming) = endpoint.accept().await {
        let listener = waiting.lock().unwrap().take().filter(|tx| !tx.is_closed());
        tokio::spawn(async move {
            let accepting = match incoming.accept() {
                Ok(a) => a,
                Err(e) => {
                    if let Some(tx) = listener {
                        let _ = tx.send(Err(session::map_connection_error(e)));
                    }
                    return;
                }
            };
            let conn = match accepting.await {
                Ok(c) => c,
                Err(e) => {
                    if let Some(tx) = listener {
                        let _ = tx.send(Err(session::map_connecting_error(e)));
                    }
                    return;
                }
            };
            match listener {
                Some(tx) => {
                    let parts = session::establish(conn, session::Side::Listener).await;
                    let _ = tx.send(parts);
                }
                None => conn.close(
                    proto::close::BUSY.into(),
                    proto::close::reason(proto::close::BUSY),
                ),
            }
        });
    }
}

/// A ticket and the wait for the one session it admits.
pub struct Listener {
    inner: Arc<Inner>,
    ticket: String,
    addresses: Vec<SocketAddr>,
    rx: oneshot::Receiver<Result<session::Parts, NetError>>,
}

impl fmt::Debug for Listener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Listener({})", self.ticket)
    }
}

impl Listener {
    /// The ticket to hand to the joiner.
    pub fn ticket(&self) -> &str {
        &self.ticket
    }

    /// The direct addresses the ticket carries.
    pub fn addresses(&self) -> Vec<SocketAddr> {
        self.addresses.clone()
    }

    /// Wait up to `timeout` for a joiner to connect and complete the
    /// handshake; `Ok(None)` when none came, so the caller can poll.
    pub fn accept(&mut self, timeout: Duration) -> Result<Option<Session>, NetError> {
        let inner = self.inner.clone();
        let waited = inner.timed(timeout, &mut self.rx)?;
        match waited {
            Err(_) => Ok(None),
            Ok(Err(_)) => Err(NetError::new(Code::Connect, "the listener was dropped")),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Ok(Ok(parts))) => Ok(Some(Session::new(inner, parts))),
        }
    }

    /// The session if a joiner has completed the handshake, without
    /// waiting. `Ok(None)` while none has.
    pub fn try_accept(&mut self) -> Result<Option<Session>, NetError> {
        match self.rx.try_recv() {
            Ok(Ok(parts)) => Ok(Some(Session::new(self.inner.clone(), parts))),
            Ok(Err(e)) => Err(e),
            Err(oneshot::error::TryRecvError::Empty) => Ok(None),
            Err(oneshot::error::TryRecvError::Closed) => Err(NetError::new(
                Code::Connect,
                "the listener's slot was closed",
            )),
        }
    }

    /// Wait for the next joiner under the same ticket, once the session
    /// this listener admitted has ended (or its accept failed). While a
    /// session is open the slot stays empty and joiners are refused as
    /// busy; `net_busy` if another listener already waits.
    pub fn rearm(&mut self) -> Result<(), NetError> {
        let mut waiting = self.inner.waiting.lock().unwrap();
        if waiting.as_ref().is_some_and(|w| !w.is_closed()) {
            return Err(NetError::new(Code::Busy, "a listener is already waiting"));
        }
        if self.inner.sessions.load(Ordering::SeqCst) > 0 {
            return Err(NetError::new(Code::Busy, "a session is open"));
        }
        let (tx, rx) = oneshot::channel();
        *waiting = Some(tx);
        self.rx = rx;
        Ok(())
    }
}
