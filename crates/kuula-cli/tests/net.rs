//! `kuula net listen` and `kuula net join` as two processes on
//! loopback: a clean session, a killed joiner, bad tickets, an absent
//! peer, and Ctrl+Break exiting within its bound.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

fn kuula() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kuula"))
}

/// A child whose stdout lines arrive on a channel as they are printed.
struct Peer {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl Peer {
    fn spawn(mut cmd: Command) -> Peer {
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Peer {
            child,
            lines,
            seen: Vec::new(),
        }
    }

    /// Wait for a line starting with `prefix`; the rest of that line.
    fn wait_for(&mut self, prefix: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if let Some(rest) = line.strip_prefix(prefix) {
                        return rest.trim().to_string();
                    }
                }
                Err(_) => panic!("no {prefix:?} line within {timeout:?}; saw {:?}", self.seen),
            }
        }
    }

    /// Wait for the process to exit; its status, stdout and stderr.
    fn finish(mut self, timeout: Duration) -> (ExitStatus, String, String) {
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                break s;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("no exit within {timeout:?}; saw {:?}", self.seen);
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        while let Ok(line) = self.lines.recv_timeout(Duration::from_millis(200)) {
            self.seen.push(line);
        }
        let mut stderr = String::new();
        if let Some(mut e) = self.child.stderr.take() {
            use std::io::Read;
            let _ = e.read_to_string(&mut stderr);
        }
        (status, self.seen.join("\n"), stderr)
    }
}

fn listener(name: &str) -> Peer {
    let mut cmd = kuula();
    cmd.args(["net", "listen", "--bind", "127.0.0.1:0", "--name", name]);
    Peer::spawn(cmd)
}

fn joiner(ticket: &str, name: &str, say: Option<&str>) -> Command {
    let mut cmd = kuula();
    cmd.args([
        "net",
        "join",
        ticket,
        "--bind",
        "127.0.0.1:0",
        "--name",
        name,
    ]);
    if let Some(text) = say {
        cmd.args(["--say", text]);
    }
    cmd
}

const WAIT: Duration = Duration::from_secs(10);

#[test]
fn listen_and_join_exchange_greetings_and_a_ping() {
    let mut alice = listener("alice");
    let ticket = alice.wait_for("ticket:", WAIT);
    assert!(ticket.starts_with("endpoint"), "{ticket}");

    let t = Instant::now();
    let bob = joiner(&ticket, "bob", Some("nice to meet you"))
        .output()
        .unwrap();
    let bob_out = String::from_utf8_lossy(&bob.stdout);
    let bob_err = String::from_utf8_lossy(&bob.stderr);
    assert_eq!(bob.status.code(), Some(0), "{bob_out}\n{bob_err}");
    assert!(bob_out.contains("peer says: hello from alice"), "{bob_out}");
    assert!(bob_out.contains("ping:"), "{bob_out}");
    assert!(bob_out.contains("bye"), "{bob_out}");

    let (status, out, err) = alice.finish(WAIT);
    assert_eq!(status.code(), Some(0), "{out}\n{err}");
    assert!(out.contains("peer says: hello from bob"), "{out}");
    assert!(out.contains("peer says: nice to meet you"), "{out}");
    assert!(out.contains("ping:"), "{out}");
    assert!(out.contains("peer closed"), "{out}");
    assert!(t.elapsed() < WAIT, "{:?}", t.elapsed());
}

#[test]
fn a_killed_joiner_is_a_lost_peer_within_the_idle_timeout() {
    let mut alice = listener("alice");
    let ticket = alice.wait_for("ticket:", WAIT);
    let mut bob = Peer::spawn(joiner(&ticket, "bob", None));
    bob.wait_for("ping:", WAIT);
    alice.wait_for("ping:", WAIT);
    let t = Instant::now();
    bob.child.kill().unwrap();
    let (status, out, err) = alice.finish(WAIT);
    assert_eq!(status.code(), Some(3), "{out}\n{err}");
    assert!(err.contains("error: net_connect peer lost"), "{err}");
    assert!(
        t.elapsed() < Duration::from_secs(8),
        "lost peer noticed after {:?}",
        t.elapsed()
    );
    let _ = bob.finish(WAIT);
}

#[test]
fn a_second_joiner_is_refused_as_busy() {
    let mut alice = listener("alice");
    let ticket = alice.wait_for("ticket:", WAIT);
    let mut bob = Peer::spawn(joiner(&ticket, "bob", None));
    bob.wait_for("ping:", WAIT);
    let carol = joiner(&ticket, "carol", Some("hi")).output().unwrap();
    let err = String::from_utf8_lossy(&carol.stderr);
    assert_eq!(carol.status.code(), Some(3), "{err}");
    assert!(err.contains("error: net_busy"), "{err}");
    bob.child.kill().unwrap();
    let _ = bob.finish(WAIT);
    let _ = alice.finish(WAIT);
}

#[test]
fn a_malformed_ticket_is_net_ticket() {
    let out = joiner("nonsense", "bob", Some("hi")).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(err.starts_with("error: net_ticket"), "{err}");
}

#[test]
fn an_absent_peer_is_a_bounded_net_connect() {
    let mut alice = listener("alice");
    let ticket = alice.wait_for("ticket:", WAIT);
    alice.child.kill().unwrap();
    let _ = alice.finish(WAIT);
    let t = Instant::now();
    let out = joiner(&ticket, "bob", Some("hi")).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{err}");
    assert!(err.starts_with("error: net_connect"), "{err}");
    assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
}

#[test]
fn usage_errors_exit_2() {
    let out = kuula().args(["net", "join"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = kuula()
        .args(["net", "listen", "--bind", "nope"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// Ctrl+Break reaches a child in its own process group on the same
/// console; the handler treats it as Ctrl+C. Without a console (a CI
/// runner) the event cannot be sent and the test says so and passes.
#[cfg(windows)]
#[test]
fn ctrl_break_exits_within_two_seconds_waiting_and_in_a_session() {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Console::{GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT};
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    let mut cmd = kuula();
    cmd.args(["net", "listen", "--bind", "127.0.0.1:0"]);
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    let mut alice = Peer::spawn(cmd);
    alice.wait_for("ticket:", WAIT);

    // SAFETY: plain Win32 call on a pid we own.
    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, alice.child.id()) };
    if sent == 0 {
        eprintln!("no console: Ctrl+Break cannot be sent here; skipping");
        alice.child.kill().unwrap();
        return;
    }
    let t = Instant::now();
    let (status, out, err) = alice.finish(WAIT);
    assert_eq!(status.code(), Some(0), "{out}\n{err}");
    assert!(out.contains("interrupted"), "{out}");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    eprintln!("ctrl+break while waiting: exit after {:?}", t.elapsed());

    // And with a session open: the joiner gets a bye.
    let mut cmd = kuula();
    cmd.args(["net", "listen", "--bind", "127.0.0.1:0"]);
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    let mut alice = Peer::spawn(cmd);
    let ticket = alice.wait_for("ticket:", WAIT);
    let mut bob = Peer::spawn(joiner(&ticket, "bob", None));
    bob.wait_for("ping:", WAIT);
    alice.wait_for("ping:", WAIT);
    let t = Instant::now();
    unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, alice.child.id()) };
    let (status, out, err) = alice.finish(WAIT);
    assert_eq!(status.code(), Some(0), "{out}\n{err}");
    assert!(out.contains("bye"), "{out}");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    eprintln!("ctrl+break in a session: exit after {:?}", t.elapsed());
    let (status, out, err) = bob.finish(WAIT);
    assert_eq!(status.code(), Some(0), "{out}\n{err}");
    assert!(out.contains("peer closed"), "{out}");

    // And while connecting to a peer that is gone: the 5 s connect
    // deadline does not hold the exit.
    let mut alice = listener("alice");
    let ticket = alice.wait_for("ticket:", WAIT);
    alice.child.kill().unwrap();
    let _ = alice.finish(WAIT);
    let mut cmd = joiner(&ticket, "bob", None);
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    let mut bob = Peer::spawn(cmd);
    bob.wait_for("id:", WAIT);
    std::thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, bob.child.id()) };
    let (status, out, err) = bob.finish(WAIT);
    assert_eq!(status.code(), Some(0), "{out}\n{err}");
    assert!(out.contains("interrupted"), "{out}");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    eprintln!("ctrl+break while connecting: exit after {:?}", t.elapsed());
}
