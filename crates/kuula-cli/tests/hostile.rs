//! The hostile cart set: every
//! cart under `examples/hostile` must end with the code listed in
//! `expected.txt`, in process and behind the worker, and the binary must
//! be usable afterwards. The watchdog is exercised with the worker's
//! hang hook.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

fn kuula() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kuula"))
}

fn hostile_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("hostile")
}

/// `(cart, code)` pairs from `expected.txt`.
fn expected() -> Vec<(String, String)> {
    let text = std::fs::read_to_string(hostile_dir().join("expected.txt")).unwrap();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut it = l.split_whitespace();
            (
                it.next().unwrap().to_string(),
                it.next().expect("code after name").to_string(),
            )
        })
        .collect()
}

/// The `code location: message` line the CLI prints for a fault.
fn fault_line(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .find(|l| {
            l.split(' ')
                .nth(1)
                .map(|s| s.contains(':'))
                .unwrap_or(false)
        })
        .map(|l| l.to_string())
        .unwrap_or_else(|| String::from_utf8_lossy(stderr).to_string())
}

fn check(name: &str, code: &str, worker: bool) {
    let mut cmd = kuula();
    cmd.args(["run"])
        .arg(hostile_dir().join(name))
        .args(["--headless", "--frames", "300"]);
    if worker {
        cmd.arg("--worker");
    }
    let start = Instant::now();
    let out = cmd.output().unwrap();
    let took = start.elapsed();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{name} (worker={worker}) should exit 1: {stderr}"
    );
    let line = fault_line(&out.stderr);
    assert!(
        line.starts_with(&format!("{code} ")),
        "{name} (worker={worker}) expected {code}, got: {line}"
    );
    assert!(
        line.contains(".lua"),
        "{name} (worker={worker}) names a file: {line}"
    );
    assert!(
        took < Duration::from_secs(20),
        "{name} (worker={worker}) took {took:?}"
    );
}

#[test]
fn every_hostile_cart_ends_with_its_documented_code_in_process() {
    let list = expected();
    assert!(list.len() >= 10, "{list:?}");
    for (name, code) in &list {
        check(name, code, false);
    }
}

#[test]
fn every_hostile_cart_ends_with_its_documented_code_behind_the_worker() {
    for (name, code) in &expected() {
        check(name, code, true);
    }
}

#[test]
fn every_hostile_directory_is_listed() {
    let listed: Vec<String> = expected().into_iter().map(|(n, _)| n).collect();
    for entry in std::fs::read_dir(hostile_dir()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(listed.contains(&name), "{name} is not in expected.txt");
        }
    }
}

#[test]
fn a_hung_worker_is_killed_by_the_watchdog() {
    let hello = hostile_dir().join("..").join("hello");
    let start = Instant::now();
    let out = kuula()
        .args(["run"])
        .arg(&hello)
        .args(["--worker", "--frames", "5"])
        .env("KUULA_TEST_WORKER_HANG", "1")
        .env("KUULA_TEST_WATCHDOG_MS", "500")
        .output()
        .unwrap();
    let took = start.elapsed();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("watchdog_timeout worker:"), "{stderr}");
    // The worker is the cart's guest: its silence is the
    // cart's fault on the frame it stalled.
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("frames 1 ")
            && String::from_utf8_lossy(&out.stdout).contains("state faulted"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(took < Duration::from_secs(10), "{took:?}");
    // And the binary is fine afterwards.
    let out = kuula()
        .args(["run"])
        .arg(&hello)
        .args(["--worker", "--frames", "5"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn profile_prints_a_table_that_adds_up() {
    let tiles = hostile_dir().join("..").join("tiles");
    let out = kuula()
        .args(["run"])
        .arg(&tiles)
        .args(["--headless", "--frames", "10", "--profile"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("category"), "{stdout}");
    let mut sum = 0u64;
    let mut total = 0u64;
    for line in stdout.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 {
            continue;
        }
        let Ok(v) = cols[1].parse::<u64>() else {
            continue;
        };
        if cols[0] == "total" {
            total = v;
        } else if ["lua", "draw", "text", "buf", "asset", "string", "api"].contains(&cols[0]) {
            sum += v;
        }
    }
    assert!(total > 0 && sum == total, "{stdout}");
    assert!(stdout.contains("frames 10 budget 279620"), "{stdout}");
}
