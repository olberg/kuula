//! End-to-end checks of the `kuula` binary in its headless modes.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

fn kuula() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kuula"))
}

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join(name)
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh scratch directory under the target dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("kuula-cli-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn hash_line(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout)
        .lines()
        .find(|l| l.starts_with("frames "))
        .map(|l| l.to_string())
        .unwrap_or_default()
}

#[test]
fn missing_directory_is_a_usage_error() {
    let out = kuula()
        .args(["run", "examples/does-not-exist", "--frames", "1"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("usage:"), "{stderr}");
    assert!(stderr.contains("does-not-exist"), "{stderr}");
}

#[test]
fn directory_without_main_lua_is_a_usage_error() {
    let out = kuula()
        .args(["run", env!("CARGO_MANIFEST_DIR"), "--frames", "1"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("main.lua"));
}

#[test]
fn bad_scale_is_a_usage_error() {
    let out = kuula()
        .args([
            "run",
            example("hello").to_str().unwrap(),
            "--scale",
            "5",
            "--frames",
            "1",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn broken_cart_exits_one_with_file_and_line() {
    let out = kuula()
        .args([
            "run",
            example("broken").to_str().unwrap(),
            "--frames",
            "120",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("runtime_error"), "{stderr}");
    assert!(stderr.contains("main.lua:12:"), "{stderr}");
    assert!(stderr.contains("attempt to index"), "{stderr}");
    assert!(hash_line(&out.stdout).ends_with("state faulted"));
}

#[test]
fn hello_cart_runs_ten_frames_and_exits_zero() {
    let out = kuula()
        .args(["run", example("hello").to_str().unwrap(), "--frames", "10"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(hash_line(&out.stdout).starts_with("frames 10 hash 0x"));
}

#[test]
fn headless_run_writes_frames_hashes_and_summary() {
    let dir = Scratch::new();
    let script = dir.0.join("script.json");
    std::fs::write(
        &script,
        r#"[{"frames": 5, "buttons": ["right"]}, {"frames": 2, "buttons": ["a"]}]"#,
    )
    .unwrap();
    let out_dir = dir.0.join("out");
    let out = kuula()
        .args([
            "run",
            example("tiles").to_str().unwrap(),
            "--headless",
            "--frames",
            "7",
            "--input",
            script.to_str().unwrap(),
            "--out",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for n in 1..=7 {
        assert!(
            out_dir.join(format!("frame_{n:06}.png")).is_file(),
            "frame {n}"
        );
    }
    assert!(!out_dir.join("frame_000008.png").exists());
    let hashes = std::fs::read_to_string(out_dir.join("hashes.txt")).unwrap();
    assert_eq!(hashes.lines().count(), 7);
    let run = std::fs::read_to_string(out_dir.join("run.json")).unwrap();
    assert!(run.contains("\"frames_run\": 7"), "{run}");
    assert!(run.contains("\"width\": 320"), "{run}");
    assert!(run.contains("\"state\": \"running\""), "{run}");
    // The PNG is indexed with the cart's screen size.
    let png = std::fs::read(out_dir.join("frame_000007.png")).unwrap();
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let reader = decoder.read_info().unwrap();
    assert_eq!((reader.info().width, reader.info().height), (320, 240));
    assert_eq!(reader.info().color_type, png::ColorType::Indexed);
}

#[test]
fn screenshot_writes_one_png_of_the_requested_frame() {
    let dir = Scratch::new();
    let file = dir.0.join("shot.png");
    let out = kuula()
        .args([
            "screenshot",
            example("hello").to_str().unwrap(),
            "--out",
            file.to_str().unwrap(),
            "--frame",
            "3",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(hash_line(&out.stdout).starts_with("frames 3 "));
    let png = std::fs::read(&file).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let out = kuula()
        .args([
            "screenshot",
            example("hello").to_str().unwrap(),
            "--out",
            file.to_str().unwrap(),
            "--frame",
            "0",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// A frame that cannot be written fails the run: automation must not read
/// exit 0 as "all frames are on disk".
#[test]
fn an_unwritable_frame_fails_the_run() {
    let dir = Scratch::new();
    let out_dir = dir.0.join("out");
    // A directory where the first frame's PNG should go.
    std::fs::create_dir_all(out_dir.join("frame_000001.png")).unwrap();
    let out = kuula()
        .args([
            "run",
            example("hello").to_str().unwrap(),
            "--headless",
            "--frames",
            "2",
            "--out",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot write"), "{stderr}");
    assert!(stderr.contains("frame_000001.png"), "{stderr}");
}

/// A cart that faults before its first frame still yields one capture, and
/// its PNG carries the same number as its line in `hashes.txt`.
#[test]
fn a_load_fault_numbers_its_only_frame_like_the_hash_list() {
    let dir = Scratch::new();
    let cart = dir.0.join("cart");
    std::fs::create_dir_all(&cart).unwrap();
    std::fs::write(cart.join("main.lua"), "local x = = 1\n").unwrap();
    let out_dir = dir.0.join("out");
    let out = kuula()
        .args([
            "run",
            cart.to_str().unwrap(),
            "--headless",
            "--frames",
            "5",
            "--out",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("compile_error"), "{stderr}");
    assert!(out_dir.join("frame_000001.png").is_file());
    assert!(!out_dir.join("frame_000000.png").exists());
    let hashes = std::fs::read_to_string(out_dir.join("hashes.txt")).unwrap();
    assert_eq!(hashes.lines().count(), 1);
    assert!(hashes.starts_with("000001 "), "{hashes}");
    assert!(hash_line(&out.stdout).starts_with("frames 1 "));
}

#[test]
fn bad_input_script_is_a_usage_error() {
    let dir = Scratch::new();
    let script = dir.0.join("script.json");
    std::fs::write(&script, r#"[{"buttons": ["start"]}]"#).unwrap();
    let out = kuula()
        .args([
            "run",
            example("hello").to_str().unwrap(),
            "--headless",
            "--input",
            script.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("start"));
}

#[test]
fn worker_run_matches_the_in_process_hash() {
    let direct = kuula()
        .args([
            "run",
            example("tiles").to_str().unwrap(),
            "--headless",
            "--frames",
            "30",
        ])
        .output()
        .unwrap();
    let via_worker = kuula()
        .args([
            "run",
            example("tiles").to_str().unwrap(),
            "--headless",
            "--frames",
            "30",
            "--worker",
        ])
        .output()
        .unwrap();
    assert_eq!(direct.status.code(), Some(0));
    assert_eq!(
        via_worker.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&via_worker.stderr)
    );
    let a = hash_line(&direct.stdout);
    let b = hash_line(&via_worker.stdout);
    assert!(a.starts_with("frames 30 hash 0x"), "{a}");
    assert_eq!(a, b);
    // The cart's log lines cross the pipe too.
    assert_eq!(direct.stdout, via_worker.stdout);
}

#[test]
fn worker_reports_faults_like_the_in_process_run() {
    let out = kuula()
        .args([
            "run",
            example("broken").to_str().unwrap(),
            "--headless",
            "--frames",
            "120",
            "--worker",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("runtime_error main.lua:12:"), "{stderr}");
}

/// The worker refuses garbage on its pipe and exits with the protocol
/// code instead of hanging or crashing.
#[test]
fn worker_rejects_a_malformed_frame() {
    use std::io::Write;
    let mut child = kuula()
        .arg("worker")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        // Length far over the limit.
        stdin.write_all(&0xffff_ffffu32.to_le_bytes()).unwrap();
        stdin.write_all(&[3]).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("protocol"));
}

/// A worker that dies mid-run ends the cart with a `worker_error` fault
/// on the frame it died (the worker is the cart's guest,
/// so its death is the cart's fault, shown on the error screen), the
/// broker exits 1 and does not hang. `KUULA_TEST_WORKER_CRASH` is a
/// hidden test hook that makes the worker exit on its first step.
#[test]
fn a_dead_worker_is_reported_not_fatal() {
    let out = kuula()
        .args([
            "run",
            example("hello").to_str().unwrap(),
            "--headless",
            "--frames",
            "5",
            "--worker",
        ])
        .env("KUULA_TEST_WORKER_CRASH", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("worker_error worker:"), "{stderr}");
    let line = hash_line(&out.stdout);
    assert!(
        line.starts_with("frames 1 ") && line.ends_with("state faulted"),
        "{line}"
    );
}

#[test]
fn mcp_answers_initialize_and_tools_list_over_stdio() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    let mut child = kuula()
        .args(["mcp", "--root", example("").to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"cli-test","version":"0"}}}}}}"#
    )
    .unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"validate","arguments":{{"cart":"hello"}}}}}}"#
    )
    .unwrap();
    drop(stdin);
    let mut lines = Vec::new();
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        lines.push(line.unwrap());
    }
    let status = child.wait().unwrap();
    assert!(status.success());
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(
        lines[0].contains(r#""id":1"#) && lines[0].contains(r#""serverInfo""#),
        "{}",
        lines[0]
    );
    assert!(
        lines[0].contains(r#""protocolVersion":"2025-06-18""#),
        "{}",
        lines[0]
    );
    assert!(lines[1].contains(r#""name":"screenshot""#), "{}", lines[1]);
    assert!(lines[2].contains(r#""ok":true"#), "{}", lines[2]);
}
