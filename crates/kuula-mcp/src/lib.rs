//! `kuula-mcp`: the console for agents, over stdio.
//!
//! A newline-delimited JSON-RPC 2.0 server speaking MCP as clients speak
//! it today: `initialize`, `ping`, `tools/list`, `tools/call`,
//! `resources/list`, `resources/read`. Hand-written on
//! serde_json; no SDK. The tools mirror the CLI: `validate`, `run`,
//! `replay`, `step`, `input`, `screenshot`, `state`, `logs`, `profile`,
//! `stop` on server-minted console handles.
//!
//! Layout: [`protocol`] frames and dispatches, [`tools`] holds the tool
//! schemas and handlers, [`session`] owns the live consoles and their
//! bounded histories. Everything the cart wrote (log lines, fault
//! messages, titles) crosses this boundary as escaped plain text with
//! control characters removed. Only stderr is used for
//! logging; stdout carries protocol frames and nothing else.

pub mod protocol;
pub mod session;
pub mod tools;

use std::io::Write;
use std::path::PathBuf;

pub use protocol::{serve, Server, MAX_LINE_BYTES, PROTOCOL_VERSION};
pub use session::{Session, ToolError, TransportFactory, MAX_CONSOLES};

/// The API reference, served as `kuula://docs/api.md`.
pub const API_MD: &str = include_str!("../../../docs/api.md");

/// The cart author's constraints, served as `kuula://docs/skill.md`.
pub const SKILL_MD: &str = include_str!("../../../docs/skill.md");

/// Serve on the process's stdin and stdout until stdin closes. Cart
/// paths are resolved under `root`, or the current directory. With
/// `transports`, the `run` tool's `net` argument can host or join;
/// without, it is refused (this crate opens no socket itself).
pub fn run_stdio(
    root: Option<PathBuf>,
    transports: Option<TransportFactory>,
) -> std::io::Result<()> {
    let root = match root {
        Some(r) => r,
        None => std::env::current_dir()?,
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let result = serve(stdin.lock(), &mut out, root, transports);
    out.flush()?;
    result
}
