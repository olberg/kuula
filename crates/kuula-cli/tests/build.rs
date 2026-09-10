//! `kuula build` and running a packed cart.

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

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("kuula-build-{}-{n}", std::process::id()));
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

fn build(dir: &str, out: &std::path::Path) -> std::process::Output {
    kuula()
        .args(["build", example(dir).to_str().unwrap(), "--out"])
        .arg(out)
        .output()
        .unwrap()
}

#[test]
fn build_then_run_matches_the_directory_hash() {
    let s = Scratch::new();
    let zip = s.0.join("hello.zip");
    let out = build("hello", &zip);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(zip.is_file());

    let from_zip = kuula()
        .args(["run", zip.to_str().unwrap(), "--frames", "10"])
        .output()
        .unwrap();
    assert_eq!(
        from_zip.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&from_zip.stderr)
    );
    let from_dir = kuula()
        .args(["run", example("hello").to_str().unwrap(), "--frames", "10"])
        .output()
        .unwrap();
    let a = hash_line(&from_zip.stdout);
    let b = hash_line(&from_dir.stdout);
    assert!(a.starts_with("frames 10 hash 0x"), "{a}");
    assert_eq!(a, b);

    // The worker path takes the same snapshot.
    let via_worker = kuula()
        .args(["run", zip.to_str().unwrap(), "--frames", "10", "--worker"])
        .output()
        .unwrap();
    assert_eq!(hash_line(&via_worker.stdout), a);

    // `.cart` is the same thing under a friendlier name.
    let cart = s.0.join("hello.cart");
    std::fs::copy(&zip, &cart).unwrap();
    let png = s.0.join("shot.png");
    let shot = kuula()
        .args(["screenshot", cart.to_str().unwrap(), "--out"])
        .arg(&png)
        .output()
        .unwrap();
    assert_eq!(
        shot.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&shot.stderr)
    );
    assert!(png.is_file());
}

#[test]
fn build_is_byte_deterministic() {
    let s = Scratch::new();
    let a = s.0.join("a.zip");
    let b = s.0.join("b.zip");
    assert_eq!(build("tiles", &a).status.code(), Some(0));
    assert_eq!(build("tiles", &b).status.code(), Some(0));
    let (a, b) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    assert!(!a.is_empty());
    assert_eq!(a, b);
}

#[test]
fn build_packs_only_served_entries() {
    let s = Scratch::new();
    let dir = s.0.join("cart");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("main.lua"), "cls(2)").unwrap();
    std::fs::write(dir.join("src").join("m.lua"), "return 1").unwrap();
    std::fs::write(dir.join("README.md"), "not packed").unwrap();
    std::fs::write(dir.join("notes.txt"), "not packed").unwrap();
    let zip = s.0.join("cart.zip");
    let out = kuula()
        .args([
            "build",
            dir.to_str().unwrap(),
            "--out",
            zip.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("packed 2 entries"), "{stdout}");
    let bytes = std::fs::read(&zip).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("main.lua") && text.contains("src/m.lua"));
    assert!(!text.contains("README") && !text.contains("notes.txt"));
}

#[test]
fn a_file_that_is_not_a_zip_is_a_cart_read_error() {
    let s = Scratch::new();
    let bogus = s.0.join("bogus.zip");
    std::fs::write(&bogus, "not a zip at all").unwrap();
    let out = kuula()
        .args(["run", bogus.to_str().unwrap(), "--frames", "1"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cart_read_error"));

    let out = kuula()
        .args(["build", bogus.to_str().unwrap(), "--out"])
        .arg(s.0.join("x.zip"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "build wants a directory");
}
