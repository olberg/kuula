//! Stateless bounded pair runs, with artifacts kept outside cart directories.

use super::{arg_str, invalid, ToolOutput};
use crate::session::{Session, ToolError};
use kuula_host_headless::net_sim::{simulate, Guest, Scenario};
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_RUN: AtomicU64 = AtomicU64::new(0);

pub(super) fn run(
    session: &mut Session,
    args: &Map<String, Value>,
) -> Result<ToolOutput, ToolError> {
    let cart = arg_str(args, "cart")?;
    let peer = match args.get("peer_cart") {
        None => cart,
        Some(Value::String(s)) => s,
        _ => return Err(invalid("peer_cart must be a string")),
    };
    let scenario: Scenario =
        serde_json::from_value(args.get("scenario").cloned().unwrap_or_else(|| json!({})))
            .map_err(|e| invalid(e.to_string()))?;
    let inputs = scenario.inputs().map_err(invalid)?;
    let snapshots = [session.snapshot(cart)?, session.snapshot(peer)?];
    let guest = Guest::new(kuula_lua::LuaGuest::factory, kuula_lua::RANDOM_SEED);
    let run = simulate(&guest, snapshots, scenario.config, inputs)
        .map_err(|e| ToolError::new("net_sim_error", e))?;
    let n = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("kuula-net-sim-{}-{n}", std::process::id()));
    run.write(&dir)
        .map_err(|e| ToolError::new("artifact_write_error", e))?;
    let mut result =
        ToolOutput::json(json!({"report": run.report, "artifacts": dir.to_string_lossy()}));
    for capture in &run.captures {
        result.content.push(json!({"type": "image", "mimeType": "image/png", "data": kuula_core::codec::base64_encode(capture)}));
    }
    Ok(result)
}
