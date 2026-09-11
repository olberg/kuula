//! `kuula net`: the networking diagnostic a person runs in two
//! terminals. Hidden from `--help` like `worker`; nothing here is a
//! player command or cart API.
//!
//! - `kuula net listen [--bind ADDR] [--name TEXT]` prints `ticket: ...`
//!   and waits for one peer; on connect it prints the peer, sends its
//!   greeting, measures a ping, prints every text it receives, and
//!   exits 0 when the peer says bye or Ctrl+C is pressed.
//! - `kuula net join TICKET [--bind ADDR] [--name TEXT] [--say TEXT]`
//!   connects, does the same, and with `--say` sends that text and
//!   leaves; without it, stays until Ctrl+C or the peer closes.
//!
//! Exit codes: 0 for a clean session, 2 for usage (clap), 3 for every
//! `net_*` failure, printed as `error: <code> <detail>` on stderr. A
//! lost peer is `net_connect peer lost: ...`, distinct from a clean
//! close. This is the only place in the binary that constructs a
//! `Net`; `run`, `shell`, `mcp` and `worker` never reach this module.

use std::io::Write;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use clap::Subcommand;
use kuula_net::{Code, Event, Net, NetConfig, NetError, Session};

/// Exit code for a `net_*` failure.
pub const EXIT_NET: u8 = 3;

/// How often a wait checks for Ctrl+C.
const POLL: Duration = Duration::from_millis(200);

/// How long `--say` waits for the peer's greeting before leaving.
const SAY_GRACE: Duration = Duration::from_secs(2);

#[derive(Subcommand)]
pub enum NetCommand {
    /// Print a ticket and wait for one peer.
    Listen {
        /// Bind this address only (default: every interface).
        #[arg(long)]
        bind: Option<SocketAddr>,
        /// Name in the greeting.
        #[arg(long, default_value = "listener")]
        name: String,
    },
    /// Connect to a ticket printed by `listen`.
    Join {
        ticket: String,
        /// Bind this address only (default: every interface).
        #[arg(long)]
        bind: Option<SocketAddr>,
        /// Name in the greeting.
        #[arg(long, default_value = "joiner")]
        name: String,
        /// Send this text after the greeting, then say bye and exit.
        #[arg(long)]
        say: Option<String>,
        /// Send the --say text this many times (measurements).
        #[arg(long, default_value_t = 1, requires = "say")]
        repeat: u32,
        /// After --say, stay connected instead of saying bye.
        #[arg(long, requires = "say")]
        stay: bool,
    },
}

/// Set by Ctrl+C; the `Net` is given the same flag as its cancel flag,
/// so a connect, a handshake wait, a ping or a full queue returns
/// `net_cancelled` within its poll interval instead of running to its
/// own deadline.
static INTERRUPTED: LazyLock<Arc<AtomicBool>> = LazyLock::new(|| Arc::new(AtomicBool::new(false)));

fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

fn config(bind: Option<SocketAddr>) -> NetConfig {
    NetConfig {
        enabled: true,
        bind,
        cancel: Some(INTERRUPTED.clone()),
    }
}

/// Ctrl+C and Ctrl+Break set a flag the waits below poll, so an open
/// session gets its `Bye` before the process leaves.
#[cfg(windows)]
fn install_ctrl_handler() {
    use windows_sys::Win32::System::Console::{
        SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_C_EVENT,
    };
    unsafe extern "system" fn handler(kind: u32) -> i32 {
        if kind == CTRL_C_EVENT || kind == CTRL_BREAK_EVENT {
            INTERRUPTED.store(true, Ordering::SeqCst);
            1
        } else {
            0
        }
    }
    // SAFETY: the handler touches only an atomic.
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(not(windows))]
fn install_ctrl_handler() {}

pub fn main(command: NetCommand) -> u8 {
    install_ctrl_handler();
    let result = match command {
        NetCommand::Listen { bind, name } => listen(bind, &name),
        NetCommand::Join {
            ticket,
            bind,
            name,
            say,
            repeat,
            stay,
        } => join(
            &ticket,
            bind,
            &name,
            say.as_deref().map(|s| (s, repeat, stay)),
        ),
    };
    match result {
        Ok(code) => code,
        Err(e) if e.code == Code::Cancelled && interrupted() => {
            say("interrupted");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            EXIT_NET
        }
    }
}

fn say(line: impl std::fmt::Display) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

fn listen(bind: Option<SocketAddr>, name: &str) -> Result<u8, NetError> {
    let net = Net::new(&config(bind))?;
    let mut listener = net.listen()?;
    say(format!("ticket: {}", listener.ticket()));
    say(format!("id: {}", net.id()));
    let addresses: Vec<String> = listener.addresses().iter().map(|a| a.to_string()).collect();
    say(format!("addresses: {}", addresses.join(" ")));
    let session = loop {
        if interrupted() {
            say("interrupted while waiting; no peer came");
            return Ok(0);
        }
        if let Some(session) = listener.accept(POLL)? {
            break session;
        }
    };
    chat(greet(session, name)?)
}

fn join(
    ticket: &str,
    bind: Option<SocketAddr>,
    name: &str,
    say_text: Option<(&str, u32, bool)>,
) -> Result<u8, NetError> {
    let net = Net::new(&config(bind))?;
    say(format!("id: {}", net.id()));
    let session = net.join(ticket)?;
    let mut session = greet(session, name)?;
    let Some((text, repeat, stay)) = say_text else {
        return chat(session);
    };
    for _ in 0..repeat.max(1) {
        if let Err(e) = session.send_text(text) {
            return Err(farewell(session, e));
        }
    }
    if stay {
        return chat(session);
    }
    // Give the peer's greeting time to arrive so both sides print
    // the other's, then leave.
    let deadline = Instant::now() + SAY_GRACE;
    let mut heard = false;
    while !heard && Instant::now() < deadline && !interrupted() {
        match session.recv(POLL) {
            None => {}
            Some(Event::Text(t)) => {
                say(format!("peer says: {t}"));
                heard = true;
            }
            Some(Event::Data(d)) => say(format!("peer sent data: {} bytes", d.len())),
            Some(other) => return ended(other),
        }
    }
    session.close()?;
    say("bye");
    Ok(0)
}

/// Print the peer, send the greeting, measure one ping.
fn greet(mut session: Session, name: &str) -> Result<Session, NetError> {
    say(format!(
        "peer: {} ({})",
        session.peer_id(),
        session.peer_runtime()
    ));
    let opened = session
        .send_text(&format!("hello from {name}"))
        .and_then(|()| session.ping());
    match opened {
        Ok(rtt) => {
            say(format!("ping: {:.2} ms", rtt.as_secs_f64() * 1000.0));
            Ok(session)
        }
        Err(e) => Err(farewell(session, e)),
    }
}

/// A failure with a session open: when it is Ctrl+C, the session still
/// gets its `Bye`; the error is returned either way.
fn farewell(session: Session, e: NetError) -> NetError {
    if e.code == Code::Cancelled {
        say("interrupted; saying bye");
        let _ = session.close();
    }
    e
}

/// Print what arrives until the peer leaves or Ctrl+C.
fn chat(mut session: Session) -> Result<u8, NetError> {
    loop {
        if interrupted() {
            say("interrupted; saying bye");
            session.close()?;
            say("bye");
            return Ok(0);
        }
        match session.recv(POLL) {
            None => {}
            Some(Event::Text(t)) => say(format!("peer says: {t}")),
            // A cart peer's messages are not the diagnostic's business.
            Some(Event::Data(d)) => say(format!("peer sent data: {} bytes", d.len())),
            Some(other) => return ended(other),
        }
    }
}

/// The exit for an event that ended the session.
fn ended(event: Event) -> Result<u8, NetError> {
    match event {
        Event::Text(_) | Event::Data(_) => unreachable!("texts are printed, not ends"),
        Event::PeerClosed => {
            say("peer closed");
            Ok(0)
        }
        Event::PeerLost(why) => Err(NetError::new(Code::Connect, format!("peer lost: {why}"))),
        Event::Error(e) => Err(e),
    }
}
