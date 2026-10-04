//! `deploy`: push a cart directory to another desktop's development
//! receiver. This crate opens no socket: the binary injects the push
//! (`DeployFn`), and without it the tool answers `deploy_unavailable`.

use super::{arg_str, invalid, ToolOutput};
use crate::session::{clean_text, DeployRequest, Session, ToolError};
use serde_json::{json, Map, Value};

pub(super) fn run(
    session: &mut Session,
    args: &Map<String, Value>,
) -> Result<ToolOutput, ToolError> {
    let cart = arg_str(args, "cart")?;
    let to = arg_str(args, "to")?;
    if to.len() > kuula_core::net::MAX_TICKET {
        return Err(invalid(format!(
            "to must be a ticket of at most {} bytes",
            kuula_core::net::MAX_TICKET
        )));
    }
    let push = session.deploy_fn().ok_or_else(|| {
        ToolError::new(
            "deploy_unavailable",
            "this server was started without networking",
        )
    })?;
    let dir = session.resolve(cart)?;
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let snapshot = session.snapshot(cart)?;
    let out = push(DeployRequest {
        name,
        snapshot,
        to: to.to_string(),
    })?;
    Ok(ToolOutput::json(json!({
        "cart": cart,
        "name": out.name,
        "bytes": out.bytes,
        "digest": out.digest,
        "ok": out.code == "deploy_ok",
        "code": out.code,
        "detail": clean_text(&out.detail),
        "transfer": out.transfer,
        "validation": out.validation,
        "install": out.install,
        "restart": out.restart,
    })))
}
