//! `kuula deploy receive --once` and `kuula deploy push` as two
//! processes with separate data directories (`KUULA_SAVE_ROOT`) on
//! loopback: an unpaired push is refused and reported, an approved one
//! installs a cart byte-identical to what `kuula build` packs, and the
//! local checks, the usage errors and the exit codes hold.
#![cfg(feature = "net")]

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(20);

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Two installations side by side in a scratch directory, removed on drop.
struct Site {
    root: PathBuf,
}

impl Site {
    fn new() -> Site {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("kuula-deploy-cli-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Site { root }
    }

    /// A command for the installation `side` ("rx" or "tx"): its own save
    /// root, so its own `settings.kv` directory, key and approved list.
    fn kuula(&self, side: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kuula"));
        cmd.env("KUULA_SAVE_ROOT", self.root.join(side).join("saves"));
        cmd
    }

    fn carts(&self) -> PathBuf {
        self.root.join("carts")
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join(name)
}

fn text(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
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

    fn wait_for(&mut self, prefix: &str) -> String {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if let Some(rest) = line.strip_prefix(prefix) {
                        return rest.trim().to_string();
                    }
                }
                Err(_) => panic!("no {prefix:?} line within {WAIT:?}; saw {:?}", self.seen),
            }
        }
    }

    fn finish(mut self) -> (ExitStatus, String, String) {
        let deadline = Instant::now() + WAIT;
        let status = loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                break s;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("no exit within {WAIT:?}; saw {:?}", self.seen);
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        while let Ok(line) = self.lines.recv_timeout(Duration::from_millis(200)) {
            self.seen.push(line);
        }
        let mut stderr = String::new();
        if let Some(mut e) = self.child.stderr.take() {
            let _ = e.read_to_string(&mut stderr);
        }
        (status, self.seen.join("\n"), stderr)
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn receiver(site: &Site) -> Peer {
    let mut cmd = site.kuula("rx");
    cmd.args([
        "deploy",
        "receive",
        "--once",
        "--bind",
        "127.0.0.1:0",
        "--carts",
    ])
    .arg(site.carts());
    Peer::spawn(cmd)
}

fn push(site: &Site, cart: &Path, ticket: &str) -> Output {
    site.kuula("tx")
        .args(["deploy", "push"])
        .arg(cart)
        .args(["--to", ticket])
        .output()
        .unwrap()
}

#[test]
fn an_unpaired_push_is_refused_and_an_approved_one_installs_the_built_cart() {
    let site = Site::new();

    // The sender's development id: stable across calls, 64 hex digits.
    let id = site.kuula("tx").args(["deploy", "id"]).output().unwrap();
    assert_eq!(id.status.code(), Some(0));
    let id = String::from_utf8_lossy(&id.stdout).trim().to_string();
    assert_eq!(id.len(), 64, "{id}");
    assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
    let again = site.kuula("tx").args(["deploy", "id"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&again.stdout).trim(), id);
    assert!(site.root.join("tx").join("deploy").join("key").is_file());

    let mut rx = receiver(&site);
    let ticket = rx.wait_for("ticket:");
    assert!(ticket.starts_with("endpoint"), "{ticket}");
    let rx_id = rx.wait_for("id:");
    assert_ne!(rx_id, id, "two installations, two identities");

    // Not approved: refused with the deploy code, exit 1, nothing written,
    // and the receiver names the id so a person can approve it.
    let refused = push(&site, &example("hello"), &ticket);
    let (out, err) = text(&refused);
    assert_eq!(refused.status.code(), Some(1), "{out}\n{err}");
    assert!(err.contains("deploy_unpaired"), "{err}");
    let line = rx.wait_for("unpaired:");
    assert!(line.contains(&id), "{line}");
    assert!(!site.carts().join("hello.cart").exists());

    // Approve it at the receiver (its own data directory), then push.
    let approve = site
        .kuula("rx")
        .args(["deploy", "approve", &id])
        .output()
        .unwrap();
    assert_eq!(approve.status.code(), Some(0));
    assert!(
        text(&approve).0.contains("approved:"),
        "{:?}",
        text(&approve)
    );
    let list = site
        .kuula("rx")
        .args(["deploy", "approved"])
        .output()
        .unwrap();
    assert_eq!(text(&list).0.trim(), id);

    let pushed = push(&site, &example("hello"), &ticket);
    let (out, err) = text(&pushed);
    assert_eq!(pushed.status.code(), Some(0), "{out}\n{err}");
    for line in [
        "transfer: ok",
        "validation: ok",
        "install: ok",
        "restart: not_run",
        "result: deploy_ok",
    ] {
        assert!(out.contains(line), "{line:?} missing from\n{out}");
    }
    assert!(out.contains("push: hello "), "{out}");

    let (status, rx_out, rx_err) = rx.finish();
    assert_eq!(status.code(), Some(0), "{rx_out}\n{rx_err}");
    assert!(rx_out.contains("offer: hello "), "{rx_out}");
    assert!(rx_out.contains("finished: deploy_ok hello "), "{rx_out}");

    // The installed file is what `kuula build` packs from the directory.
    let built = site.root.join("built.zip");
    let build = Command::new(env!("CARGO_BIN_EXE_kuula"))
        .args(["build"])
        .arg(example("hello"))
        .arg("--out")
        .arg(&built)
        .output()
        .unwrap();
    assert!(build.status.success());
    assert_eq!(
        std::fs::read(site.carts().join("hello.cart")).unwrap(),
        std::fs::read(&built).unwrap()
    );
    let staging = std::fs::read_dir(site.carts().join(".staging"))
        .unwrap()
        .count();
    assert_eq!(staging, 0);

    // Revoking takes the approval back.
    let revoke = site
        .kuula("rx")
        .args(["deploy", "revoke", &id])
        .output()
        .unwrap();
    assert!(text(&revoke).0.contains("revoked:"));
    let list = site
        .kuula("rx")
        .args(["deploy", "approved"])
        .output()
        .unwrap();
    assert!(text(&list).0.trim().is_empty());
}

#[test]
fn local_checks_and_usage_errors_come_before_the_network() {
    let site = Site::new();
    // A cart that is not a cart, a name the wire cannot carry, a bad
    // ticket, a bad id: all refused here, with their exit codes, and the
    // ticket is never dialled.
    let junk = site.root.join("junk.zip");
    std::fs::write(&junk, b"not a zip").unwrap();
    let out = push(&site, &junk, "endpointnothing");
    let (_, err) = text(&out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("deploy_invalid"), "{err}");

    let upper = site.root.join("Upper");
    std::fs::create_dir_all(&upper).unwrap();
    std::fs::write(upper.join("main.lua"), "x = 1").unwrap();
    let out = push(&site, &upper, "endpointnothing");
    let (_, err) = text(&out);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("deploy_offer"), "{err}");

    let out = push(&site, &example("hello"), "not a ticket");
    let (_, err) = text(&out);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("net_ticket"), "{err}");

    let out = push(&site, &site.root.join("missing"), "endpointnothing");
    assert_eq!(out.status.code(), Some(2));

    let out = site
        .kuula("rx")
        .args(["deploy", "approve", "nonsense"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{:?}", text(&out));

    // An unreachable receiver is a network failure: exit 3.
    let ticket = {
        let mut rx = receiver(&site);
        let t = rx.wait_for("ticket:");
        rx.kill();
        t
    };
    let started = Instant::now();
    let out = push(&site, &example("hello"), &ticket);
    assert_eq!(out.status.code(), Some(3), "{:?}", text(&out));
    assert!(started.elapsed() < Duration::from_secs(15));
}

#[test]
fn the_deploy_options_belong_to_the_shell() {
    let out = Command::new(env!("CARGO_BIN_EXE_kuula"))
        .args(["--dev-receiver", "build", "x", "--out", "y"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{:?}", text(&out));
    assert!(text(&out).1.contains("--dev-receiver belongs to the shell"));
}
