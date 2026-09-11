//! One session over one bidirectional stream: the handshake, then a
//! reader task that frames bytes and a main task that answers pings,
//! writes what the caller queues and reports how the session ended.

use std::sync::Arc;
use std::time::{Duration, Instant};

use iroh::endpoint::{
    ApplicationClose, ConnectError, ConnectingError, Connection, ConnectionError, ReadError,
    ReadExactError, RecvStream, SendStream, TransportErrorCode, WriteError,
};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::proto::{self, close, Frame};
use crate::{Code, Inner, NetError, RUNTIME_VERSION};

/// Which side of the stream this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Listener,
    Joiner,
}

/// What a session delivers, in order. After `PeerClosed`, `PeerLost` or
/// `Error` nothing more arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The peer sent a `Text`.
    Text(String),
    /// The peer sent a `Data`: a cart message.
    Data(Vec<u8>),
    /// The peer sent `Bye` or closed the connection cleanly.
    PeerClosed,
    /// The transport lost the peer; the reason is for the log.
    PeerLost(String),
    /// This side detected a violation or a deadline and has closed.
    Error(NetError),
}

/// What the caller can ask the main task to do.
pub(crate) enum Cmd {
    /// An encoded `Text` or `Data` frame.
    Text(Vec<u8>),
    Ping(oneshot::Sender<Duration>),
    Bye,
}

/// A session after the handshake, before the caller wraps it.
pub(crate) struct Parts {
    peer_id: String,
    peer_runtime: String,
    cmd: mpsc::Sender<Cmd>,
    events: mpsc::Receiver<Event>,
    abort: oneshot::Sender<NetError>,
    task: JoinHandle<()>,
}

/// One open session. Every call is blocking with a bound.
pub struct Session {
    inner: Arc<Inner>,
    peer_id: String,
    peer_runtime: String,
    cmd: mpsc::Sender<Cmd>,
    events: mpsc::Receiver<Event>,
    /// Ends the session with code 3 when a caller-side deadline (a
    /// ping or a full queue) passes; bypasses the command queue,
    /// which is what is full in that case.
    abort: Option<oneshot::Sender<NetError>>,
    task: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Session(peer {})", self.peer_id)
    }
}

impl Session {
    pub(crate) fn new(inner: Arc<Inner>, parts: Parts) -> Session {
        inner
            .sessions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Session {
            inner,
            peer_id: parts.peer_id,
            peer_runtime: parts.peer_runtime,
            cmd: parts.cmd,
            events: parts.events,
            abort: Some(parts.abort),
            task: Some(parts.task),
        }
    }

    /// The peer's endpoint id.
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// The runtime version the peer announced; informational.
    pub fn peer_runtime(&self) -> &str {
        &self.peer_runtime
    }

    /// Queue a `Text`; waits up to [`proto::SEND_DEADLINE`] for room,
    /// and ends the session with `net_timeout` when there is none.
    pub fn send_text(&mut self, text: &str) -> Result<(), NetError> {
        let bytes = proto::encode(&Frame::Text(text.to_string()))?;
        self.send_cmd(Cmd::Text(bytes), true)
    }

    /// Queue a `Data` frame the same way.
    pub fn send_data(&mut self, data: &[u8]) -> Result<(), NetError> {
        let bytes = proto::encode(&Frame::Data(data.to_vec()))?;
        self.send_cmd(Cmd::Text(bytes), true)
    }

    /// Queue a `Data` frame without waiting: `net_queue_full` at once
    /// when the outgoing queue has no room, and the session goes on.
    pub fn try_send_data(&mut self, data: &[u8]) -> Result<(), NetError> {
        let bytes = proto::encode(&Frame::Data(data.to_vec()))?;
        match self.cmd.try_send(Cmd::Text(bytes)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(NetError::new(
                Code::QueueFull,
                format!("the outgoing queue holds {} frames", proto::OUTGOING_QUEUE),
            )),
            Err(TrySendError::Closed(_)) => {
                Err(NetError::new(Code::Connect, "the session has ended"))
            }
        }
    }

    /// The next event if one has arrived, without waiting. `Ok(None)`
    /// when nothing has; `net_connect` once the session's task is gone
    /// and nothing more can arrive.
    pub fn try_recv(&mut self) -> Result<Option<Event>, NetError> {
        match self.events.try_recv() {
            Ok(e) => Ok(Some(e)),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                Err(NetError::new(Code::Connect, "the session has ended"))
            }
        }
    }

    /// Send a `Ping` and wait for its `Pong`, at most
    /// [`proto::PING_DEADLINE`]; a missing `Pong` ends the session
    /// with `net_timeout`.
    pub fn ping(&mut self) -> Result<Duration, NetError> {
        let (tx, rx) = oneshot::channel();
        self.send_cmd(Cmd::Ping(tx), true)?;
        match self.inner.timed(proto::PING_DEADLINE, rx)? {
            Ok(Ok(rtt)) => Ok(rtt),
            Ok(Err(_)) => Err(NetError::new(Code::Connect, "the session ended")),
            Err(_) => Err(self.abort(NetError::new(
                Code::Timeout,
                format!("no pong within {} s", proto::PING_DEADLINE.as_secs()),
            ))),
        }
    }

    /// The next event, or `None` when `timeout` passed first. After the
    /// session has ended every call returns `None` at once.
    pub fn recv(&mut self, wait: Duration) -> Option<Event> {
        match self.inner.timed(wait, self.events.recv()) {
            Ok(Ok(event)) => event,
            _ => None,
        }
    }

    /// Send `Bye`, close the connection and wait for the task to end.
    /// The cancel flag does not cut this short, so a cancelled session
    /// still says goodbye. The wait covers one stuck write, the wait
    /// for the peer's close and the delivery of the last event; a task
    /// that outlives it is told to close with code 3 and the call
    /// reports `net_timeout`.
    pub fn close(mut self) -> Result<(), NetError> {
        self.send_cmd(Cmd::Bye, false)?;
        let Some(mut task) = self.task.take() else {
            return Ok(());
        };
        let bound = proto::SEND_DEADLINE + proto::BYE_DEADLINE + crate::SHUTDOWN_BOUND;
        if self.inner.timed_final(bound, &mut task)?.is_ok() {
            return Ok(());
        }
        let e = self.abort(NetError::new(
            Code::Timeout,
            "the session did not close in time",
        ));
        let _ = self.inner.timed_final(crate::SHUTDOWN_BOUND, &mut task)?;
        Err(e)
    }

    fn send_cmd(&mut self, cmd: Cmd, cancellable: bool) -> Result<(), NetError> {
        let queued = if cancellable {
            self.inner.timed(proto::SEND_DEADLINE, self.cmd.send(cmd))?
        } else {
            self.inner
                .timed_final(proto::SEND_DEADLINE, self.cmd.send(cmd))?
        };
        match queued {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(NetError::new(Code::Connect, "the session has ended")),
            Err(_) => Err(self.abort(NetError::new(
                Code::Timeout,
                format!(
                    "the outgoing queue stayed full for {} s",
                    proto::SEND_DEADLINE.as_secs()
                ),
            ))),
        }
    }

    /// Tell the task to end the session with code 3 for `e`; the same
    /// error comes back as the caller's, and as an `Event::Error`.
    fn abort(&mut self, e: NetError) -> NetError {
        if let Some(abort) = self.abort.take() {
            let _ = abort.send(e.clone());
        }
        e
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.inner
            .sessions
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

// ----- handshake -------------------------------------------------------------

/// Open (joiner) or accept (listener) the stream, exchange handshakes
/// under [`proto::HANDSHAKE_DEADLINE`], and start the tasks.
pub(crate) async fn establish(conn: Connection, side: Side) -> Result<Parts, NetError> {
    let result = timeout(proto::HANDSHAKE_DEADLINE, handshake(&conn, side)).await;
    let (send, recv, peer_runtime) = match result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            let code = match e.code {
                Code::Handshake => close::PROTOCOL,
                Code::Timeout => close::TIMEOUT,
                _ => close::SHUTDOWN,
            };
            conn.close(code.into(), close::reason(code));
            return Err(e);
        }
        Err(_) => {
            conn.close(close::TIMEOUT.into(), close::reason(close::TIMEOUT));
            return Err(NetError::new(
                Code::Timeout,
                format!(
                    "no handshake within {} s",
                    proto::HANDSHAKE_DEADLINE.as_secs()
                ),
            ));
        }
    };
    let peer_id = conn.remote_id().to_string();
    let (cmd_tx, cmd_rx) = mpsc::channel(proto::OUTGOING_QUEUE);
    let (event_tx, event_rx) = mpsc::channel(proto::EVENT_QUEUE);
    let (abort_tx, abort_rx) = oneshot::channel();
    let task = tokio::spawn(run(conn, send, recv, cmd_rx, event_tx, abort_rx));
    Ok(Parts {
        peer_id,
        peer_runtime,
        cmd: cmd_tx,
        events: event_rx,
        abort: abort_tx,
        task,
    })
}

async fn handshake(
    conn: &Connection,
    side: Side,
) -> Result<(SendStream, RecvStream, String), NetError> {
    let (mut send, mut recv) = match side {
        Side::Joiner => conn.open_bi().await.map_err(lost)?,
        Side::Listener => conn.accept_bi().await.map_err(lost)?,
    };
    send.write_all(&proto::handshake(RUNTIME_VERSION))
        .await
        .map_err(write_lost)?;
    let mut header = [0u8; proto::HANDSHAKE_HEADER];
    recv.read_exact(&mut header).await.map_err(read_lost)?;
    let len = proto::handshake_header(&header)?;
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body).await.map_err(read_lost)?;
    let peer_runtime = proto::handshake_body(&body)?;
    Ok((send, recv, peer_runtime))
}

// ----- the tasks -------------------------------------------------------------

/// How the reader saw the stream end.
enum End {
    /// The peer said `Bye` (as a frame or a code-0 close).
    Closed,
    /// The transport lost the peer.
    Lost(String),
    /// The peer broke the protocol or a deadline; we close.
    Fault(NetError),
    /// We closed it ourselves; nothing to report.
    Local,
}

enum Inbound {
    Frame(Frame),
    End(End),
}

async fn run(
    conn: Connection,
    mut send: SendStream,
    recv: RecvStream,
    mut cmds: mpsc::Receiver<Cmd>,
    events: mpsc::Sender<Event>,
    mut abort: oneshot::Receiver<NetError>,
) {
    let (in_tx, mut in_rx) = mpsc::channel::<Inbound>(8);
    let reader = tokio::spawn(read_frames(recv, in_tx));
    let mut next_nonce: u64 = 0;
    let mut pending: Option<(u64, Instant, oneshot::Sender<Duration>)> = None;
    // A text the caller has no room for yet. While one waits the
    // reader is not drained, so QUIC flow control stalls the peer;
    // commands (a `Bye` above all) and the abort still go through.
    let mut stalled: Option<Event> = None;

    // Biased: the abort first, then the peer's frames, so a `Ping` is
    // answered ahead of the queued outgoing texts as `docs/net.md`
    // promises; the outgoing queue is served when the peer is quiet.
    let end = loop {
        tokio::select! {
            biased;
            aborted = &mut abort => break match aborted {
                Ok(e) => Outcome::Close(close::TIMEOUT, Some(Event::Error(e))),
                Err(_) => Outcome::Close(close::SHUTDOWN, None),
            },
            inbound = in_rx.recv(), if stalled.is_none() => match inbound {
                None => break Outcome::Close(close::SHUTDOWN, None),
                Some(Inbound::Frame(Frame::Ping(n))) => {
                    let bytes = proto::encode(&Frame::Pong(n)).expect("pong encodes");
                    if let Some(o) = write(&mut send, &bytes).await { break o; }
                }
                Some(Inbound::Frame(Frame::Pong(n))) => {
                    if pending.as_ref().is_some_and(|(nonce, ..)| *nonce == n) {
                        let (_, sent, tx) = pending.take().expect("checked");
                        let _ = tx.send(sent.elapsed());
                    }
                }
                Some(Inbound::Frame(Frame::Text(text))) => {
                    match events.try_send(Event::Text(text)) {
                        Ok(()) => {}
                        Err(TrySendError::Full(event)) => stalled = Some(event),
                        Err(TrySendError::Closed(_)) => {
                            break Outcome::Close(close::SHUTDOWN, None);
                        }
                    }
                }
                Some(Inbound::Frame(Frame::Data(data))) => {
                    match events.try_send(Event::Data(data)) {
                        Ok(()) => {}
                        Err(TrySendError::Full(event)) => stalled = Some(event),
                        Err(TrySendError::Closed(_)) => {
                            break Outcome::Close(close::SHUTDOWN, None);
                        }
                    }
                }
                Some(Inbound::Frame(Frame::Bye)) => break outcome_of(End::Closed),
                Some(Inbound::End(end)) => break outcome_of(end),
            },
            permit = events.reserve(), if stalled.is_some() => match permit {
                Ok(permit) => permit.send(stalled.take().expect("stalled")),
                Err(_) => break Outcome::Close(close::SHUTDOWN, None),
            },
            cmd = cmds.recv() => match cmd {
                None => break Outcome::Close(close::SHUTDOWN, None),
                Some(Cmd::Text(bytes)) => {
                    if let Some(o) = write(&mut send, &bytes).await { break o; }
                }
                Some(Cmd::Ping(tx)) => {
                    next_nonce += 1;
                    let bytes = proto::encode(&Frame::Ping(next_nonce)).expect("ping encodes");
                    let sent = Instant::now();
                    if let Some(o) = write(&mut send, &bytes).await { break o; }
                    pending = Some((next_nonce, sent, tx));
                }
                Some(Cmd::Bye) => {
                    let bytes = proto::encode(&Frame::Bye).expect("bye encodes");
                    if let Some(o) = write(&mut send, &bytes).await { break o; }
                    let _ = send.finish();
                    // Closing at once would discard what is still in
                    // flight; the peer closes with code 0 once it has
                    // read the Bye, and that close acknowledges it all.
                    let _ = timeout(proto::BYE_DEADLINE, conn.closed()).await;
                    break Outcome::Close(close::BYE, None);
                }
            },
        }
    };
    let event = match end {
        Outcome::Close(code, event) => {
            conn.close(code.into(), close::reason(code));
            event
        }
        Outcome::Report(event) => Some(event),
    };
    reader.abort();
    // A stalled text goes out only if there is room now; the end
    // event waits for room under a bound, so a caller that has stopped
    // reading cannot keep the task alive past `close`.
    if let Some(stalled) = stalled {
        let _ = events.try_send(stalled);
    }
    if let Some(event) = event {
        let _ = timeout(crate::SHUTDOWN_BOUND, events.send(event)).await;
    }
}

/// How the main loop ended: close with a code (and maybe report), or
/// only report because the peer already closed.
enum Outcome {
    Close(u32, Option<Event>),
    Report(Event),
}

/// Write one frame under [`proto::SEND_DEADLINE`]; the outcome if the
/// session must end.
async fn write(send: &mut SendStream, bytes: &[u8]) -> Option<Outcome> {
    match timeout(proto::SEND_DEADLINE, send.write_all(bytes)).await {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(outcome_of(write_end(e))),
        Err(_) => Some(Outcome::Close(
            close::TIMEOUT,
            Some(Event::Error(NetError::new(
                Code::Timeout,
                format!(
                    "the peer took no data for {} s",
                    proto::SEND_DEADLINE.as_secs()
                ),
            ))),
        )),
    }
}

/// How a write error ends the session.
fn write_end(e: WriteError) -> End {
    match e {
        WriteError::ConnectionLost(c) => connection_end(&c),
        WriteError::ClosedStream => End::Local,
        other => End::Lost(other.to_string()),
    }
}

/// What the main loop does about an end the reader or a write saw.
fn outcome_of(end: End) -> Outcome {
    match end {
        End::Closed => Outcome::Close(close::BYE, Some(Event::PeerClosed)),
        End::Lost(why) => Outcome::Report(Event::PeerLost(why)),
        End::Fault(e) => {
            let code = match e.code {
                Code::Timeout => close::TIMEOUT,
                Code::Busy => close::SHUTDOWN,
                _ => close::PROTOCOL,
            };
            Outcome::Close(code, Some(Event::Error(e)))
        }
        End::Local => Outcome::Close(close::SHUTDOWN, None),
    }
}

/// Frame the incoming bytes until the stream ends, one way or another.
async fn read_frames(mut recv: RecvStream, out: mpsc::Sender<Inbound>) {
    let end = loop {
        match read_frame(&mut recv).await {
            Ok(frame) => {
                let bye = frame == Frame::Bye;
                if out.send(Inbound::Frame(frame)).await.is_err() || bye {
                    return;
                }
            }
            Err(end) => break end,
        }
    };
    let _ = out.send(Inbound::End(end)).await;
}

/// One frame: the first byte without a deadline, the rest of the frame
/// under [`proto::FRAME_DEADLINE`], the header checked before the
/// payload is allocated.
async fn read_frame(recv: &mut RecvStream) -> Result<Frame, End> {
    let mut header = [0u8; proto::FRAME_HEADER];
    recv.read_exact(&mut header[..1]).await.map_err(read_end)?;
    let rest = async {
        recv.read_exact(&mut header[1..]).await.map_err(read_end)?;
        let (ty, len) = proto::frame_header(&header).map_err(End::Fault)?;
        let mut body = vec![0u8; len];
        recv.read_exact(&mut body).await.map_err(read_end)?;
        proto::frame_body(ty, &body).map_err(End::Fault)
    };
    match timeout(proto::FRAME_DEADLINE, rest).await {
        Ok(r) => r,
        Err(_) => Err(End::Fault(NetError::new(
            Code::Timeout,
            format!(
                "a frame stayed incomplete for {} s",
                proto::FRAME_DEADLINE.as_secs()
            ),
        ))),
    }
}

/// How a read error ends the session. A stream that finishes without a
/// `Bye`, between frames or inside one, is a lost peer.
fn read_end(e: ReadExactError) -> End {
    match e {
        ReadExactError::FinishedEarly(_) => End::Lost("the stream ended without a bye".into()),
        ReadExactError::ReadError(ReadError::ConnectionLost(c)) => connection_end(&c),
        ReadExactError::ReadError(ReadError::ClosedStream) => End::Local,
        ReadExactError::ReadError(e) => End::Lost(e.to_string()),
    }
}

/// Classify how the connection went away.
fn connection_end(c: &ConnectionError) -> End {
    match c {
        ConnectionError::ApplicationClosed(ApplicationClose { error_code, reason }) => {
            let code = u64::from(*error_code) as u32;
            match code {
                close::BYE => End::Closed,
                close::BUSY => End::Fault(NetError::new(Code::Busy, "the listener is busy")),
                _ => End::Lost(format!(
                    "the peer closed with code {code} {}",
                    String::from_utf8_lossy(reason)
                )),
            }
        }
        ConnectionError::TransportError(t) if t.code == no_application_protocol() => {
            End::Fault(protocol_mismatch())
        }
        ConnectionError::ConnectionClosed(c) if c.error_code == no_application_protocol() => {
            End::Fault(protocol_mismatch())
        }
        ConnectionError::LocallyClosed => End::Local,
        ConnectionError::TimedOut => End::Lost("the peer stopped answering".into()),
        other => End::Lost(other.to_string()),
    }
}

/// The TLS alert `no_application_protocol` (120) as a QUIC transport
/// error code: the peer does not speak our ALPN.
fn no_application_protocol() -> TransportErrorCode {
    TransportErrorCode::crypto(120)
}

fn protocol_mismatch() -> NetError {
    NetError::new(
        Code::ProtocolMismatch,
        format!(
            "the peer does not speak {}",
            String::from_utf8_lossy(proto::ALPN)
        ),
    )
}

// ----- error mapping ---------------------------------------------------------

/// A stream could not be opened or accepted.
fn lost(c: ConnectionError) -> NetError {
    net_error(connection_end(&c))
}

/// An incoming connection failed before the handshake.
pub(crate) fn map_connection_error(c: ConnectionError) -> NetError {
    lost(c)
}

fn write_lost(e: WriteError) -> NetError {
    match e {
        WriteError::ConnectionLost(c) => lost(c),
        other => NetError::new(Code::Connect, other.to_string()),
    }
}

fn read_lost(e: ReadExactError) -> NetError {
    net_error(read_end(e))
}

fn net_error(end: End) -> NetError {
    match end {
        End::Closed => NetError::new(Code::Connect, "the peer closed during the handshake"),
        End::Lost(why) => NetError::new(Code::Connect, why),
        End::Fault(e) => e,
        End::Local => NetError::new(Code::Connect, "closed"),
    }
}

/// `Endpoint::connect` failed.
pub(crate) fn map_connect_error(e: ConnectError) -> NetError {
    match e {
        ConnectError::Connecting { source, .. } => map_connecting_error(source),
        ConnectError::Connection { source, .. } => lost(source),
        ConnectError::Connect { source, .. } => {
            NetError::new(Code::Connect, format!("cannot connect: {source}"))
        }
        other => NetError::new(Code::Connect, format!("cannot connect: {other}")),
    }
}

/// The QUIC handshake failed; an ALPN the peer does not speak is
/// classified by [`connection_end`].
pub(crate) fn map_connecting_error(e: ConnectingError) -> NetError {
    match e {
        ConnectingError::ConnectionError { source, .. } => lost(source),
        other => NetError::new(Code::Connect, format!("cannot connect: {other}")),
    }
}
