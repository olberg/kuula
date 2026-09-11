//! Off Windows there is no worker sandbox: `--worker` still runs the
//! cart in a separate process, plainly, and prints one warning line
//! saying so instead of refusing the run with `sandbox_unavailable`.

#![cfg(not(windows))]

use std::path::PathBuf;
use std::process::Command;

fn hello() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("hello")
}

#[test]
fn the_worker_runs_plainly_and_warns_where_there_is_no_sandbox() {
    let out = Command::new(env!("CARGO_BIN_EXE_kuula"))
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
    assert!(
        stderr.lines().any(|l| l
            == "warning: no OS sandbox on this platform: the worker runs as a plain child process"),
        "{stderr}"
    );
    assert!(!stderr.contains("sandbox_unavailable"), "{stderr}");
}
