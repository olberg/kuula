//! The worker token: the
//! hostile probe is denied the user's files, the broker's temp
//! directory and the network when launched the way the worker is
//! launched, and allowed all three when launched plainly, so the test
//! proves the token and not the machine; the real worker still runs
//! under it; and a launcher that cannot finish setting up ends the run
//! with `sandbox_unavailable` and no orphan. All of that is Windows
//! only; elsewhere the one test is that `--worker` runs plainly and
//! says so.

#![cfg(windows)]

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

fn kuula() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kuula"))
}

fn hello() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("hello")
}

/// A file under the user's profile plus a path in the broker's real
/// temp directory (absolute, so the AppContainer's redirected `%TEMP%`
/// cannot make the write succeed elsewhere); both removed on drop.
struct Fixture {
    read: PathBuf,
    write: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let pid = std::process::id();
        let profile = PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"));
        let read = profile.join(format!("kuula-sandbox-read-{pid}.txt"));
        std::fs::write(&read, "private").unwrap();
        let write = std::env::temp_dir().join(format!("kuula-sandbox-write-{pid}.txt"));
        let _ = std::fs::remove_file(&write);
        assert!(read.is_absolute() && write.is_absolute());
        Fixture { read, write }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.read);
        let _ = std::fs::remove_file(&self.write);
    }
}

/// `read`, `write` and `connect` verdict lines from the probe's stdout.
fn probe(fixture: &Fixture, endpoint: &str, sandboxed: bool) -> Vec<String> {
    let mut cmd = kuula();
    if sandboxed {
        cmd.args(["sandbox-exec", "--"]);
    }
    cmd.arg("sandbox-probe")
        .arg(&fixture.read)
        .arg(&fixture.write)
        .arg(endpoint);
    let out = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "sandboxed={sandboxed}: {stderr}"
    );
    let lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(lines.len(), 3, "sandboxed={sandboxed}: {lines:?}");
    lines
}

#[test]
fn the_probe_is_allowed_plainly_and_denied_under_the_worker_launcher() {
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    // Accept whatever arrives so the positive control's connect completes.
    let accept = std::thread::spawn(move || {
        listener.set_nonblocking(false).unwrap();
        let _ = listener.accept();
    });

    // Positive control: the same probe, the same paths, no token.
    let plain = probe(&fixture, &endpoint, false);
    assert_eq!(plain, ["read ok", "write ok", "connect ok"], "{plain:?}");
    accept.join().unwrap();

    // Through the worker's launcher: denied, and denied for the right
    // reason (access denied, not a missing file).
    let sandboxed = probe(&fixture, &endpoint, true);
    assert_eq!(sandboxed[0], "read denied", "{sandboxed:?}");
    assert_eq!(sandboxed[1], "write denied", "{sandboxed:?}");
    assert!(
        sandboxed[2].starts_with("connect ") && sandboxed[2] != "connect ok",
        "{sandboxed:?}"
    );
    // And the sandboxed write really did not land in the real temp dir.
    assert!(!fixture.write.exists());
    // The read fixture is still there: "denied" was not "deleted".
    assert!(fixture.read.exists());
}

#[test]
fn the_real_worker_runs_under_the_token() {
    let out = kuula()
        .args(["run"])
        .arg(hello())
        .args(["--worker", "--frames", "5"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("frames 5 hash"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!stderr.contains("warning:"), "{stderr}");
}

#[test]
fn no_sandbox_runs_plainly_and_warns() {
    let out = kuula()
        .args(["run"])
        .arg(hello())
        .args(["--worker", "--no-sandbox", "--frames", "5"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr
            .lines()
            .any(|l| l == "warning: --no-sandbox: the worker runs without an AppContainer token"),
        "{stderr}"
    );
    // The flag means nothing without --worker.
    let out = kuula()
        .args(["run"])
        .arg(hello())
        .args(["--headless", "--no-sandbox", "--frames", "1"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn sandbox_setup_failure_ends_the_run_closed() {
    let out = kuula()
        .args(["run"])
        .arg(hello())
        .args(["--worker", "--frames", "5"])
        .env("KUULA_TEST_SANDBOX_FAIL", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    let line = stderr
        .lines()
        .find(|l| l.starts_with("sandbox_unavailable worker: "))
        .unwrap_or_else(|| panic!("{stderr}"));
    // Nothing ran: the cart faulted on its first frame, before any cart
    // code, and no hash of a real frame was produced.
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("frames 1 ")
            && String::from_utf8_lossy(&out.stdout).contains("state faulted"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    // The child that was created suspended is gone.
    let pid: u32 = line
        .split_whitespace()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no pid in {line}"));
    let list = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    let list = String::from_utf8_lossy(&list.stdout);
    assert!(
        !list.contains("kuula"),
        "worker {pid} is still alive: {list}"
    );
}
