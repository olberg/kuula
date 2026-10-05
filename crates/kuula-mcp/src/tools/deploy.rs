//! `deploy`: push a cart directory to another desktop's development
//! receiver, or to the Kuula app on an Android device over `adb`. This
//! crate opens no socket and starts no program: the binary injects the
//! push (`DeployFn`), and without it the tool answers
//! `deploy_unavailable`.

use super::{arg_str, base64, invalid, ToolOutput};
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
    let screenshot = match args.get("screenshot") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(on)) => *on,
        Some(_) => return Err(invalid("screenshot must be a boolean")),
    };
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
        screenshot,
    })?;
    let log: Vec<String> = out.log.iter().map(|line| clean_text(line)).collect();
    let mut output = ToolOutput::json(json!({
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
        "log": log,
        "screenshot": out.screenshot.is_some(),
    }));
    if let Some(png) = &out.screenshot {
        output
            .content
            .push(json!({"type": "image", "data": base64(png), "mimeType": "image/png"}));
    }
    Ok(output)
}
