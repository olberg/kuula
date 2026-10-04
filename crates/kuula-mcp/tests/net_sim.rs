use kuula_mcp::{tools, Session};
use serde_json::json;

#[test]
fn simulation_is_available_offline_and_returns_both_captures() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scenario: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("examples/marbles/scenarios/win.json")).unwrap(),
    )
    .unwrap();
    let mut session = Session::new(root);
    let out = tools::call(
        &mut session,
        "net_sim",
        &json!({"cart":"examples/marbles", "scenario":scenario}),
    )
    .unwrap();
    assert_eq!(
        out.content.iter().filter(|b| b["type"] == "image").count(),
        2
    );
    let report = out.structured.unwrap();
    assert_eq!(report["report"]["mode"], "memory");
    assert!(report["report"]["peers"][0]["state"]
        .as_str()
        .unwrap()
        .contains("winner = 1"));
    assert_eq!(session.live_count(), 0);
    std::fs::remove_dir_all(report["artifacts"].as_str().unwrap()).unwrap();
    assert!(tools::call(
        &mut session,
        "net_sim",
        &json!({"cart":"examples/marbles", "scenario":{"config":{"frames":0}}})
    )
    .is_err());
    assert!(tools::call(&mut session, "net_sim", &json!({"cart":"../outside"})).is_err());
}
