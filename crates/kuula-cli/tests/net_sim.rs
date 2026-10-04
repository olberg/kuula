use std::path::PathBuf;
use std::process::Command;

#[test]
fn cli_saves_both_peers_and_replay_matches_the_hashes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!("kuula-sim-cli-{}", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_kuula"))
        .current_dir(&root)
        .args([
            "net_sim",
            "examples/marbles",
            "--scenario",
            "examples/marbles/scenarios/win.json",
            "--out",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(report["mode"], "memory");
    for (i, name) in ["host", "join"].iter().enumerate() {
        assert!(report["peers"][i]["state"]
            .as_str()
            .unwrap()
            .contains("winner = 1"));
        let output = Command::new(env!("CARGO_BIN_EXE_kuula"))
            .current_dir(&root)
            .args(["run", "examples/marbles", "--headless", "--replay"])
            .arg(dir.join(format!("{name}.kr")))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout)
            .contains(report["peers"][i]["hash"].as_str().unwrap()));
        assert!(dir.join(format!("{name}.png")).is_file());
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(feature = "net")]
#[test]
fn real_loopback_completes_two_rounds_with_the_same_host_ticket() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!("kuula-sim-loopback-{}", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_kuula"))
        .current_dir(&root)
        .args([
            "net_sim",
            "examples/marbles",
            "--loopback",
            "--scenario",
            "examples/marbles/scenarios/rematch.json",
            "--out",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(report["mode"], "iroh-loopback");
    let log = report["peers"][0]["log"].as_array().unwrap();
    assert_eq!(
        log.iter()
            .filter(|l| l["text"].as_str().unwrap().starts_with("winner "))
            .count(),
        2,
        "{report}"
    );
    assert_eq!(
        log.iter()
            .filter(|l| l["text"].as_str().unwrap().starts_with("ticket:"))
            .count(),
        1
    );
    std::fs::remove_dir_all(dir).unwrap();
}
