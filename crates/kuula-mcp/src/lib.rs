//! `kuula-mcp`: the console for agents, over stdio.
//!
//! A newline-delimited JSON-RPC 2.0 server speaking MCP as clients speak
//! it today: `initialize`, `ping`, `tools/list`, `tools/call`,
//! `resources/list`, `resources/read`. Hand-written on
//! serde_json; no SDK. The tools mirror the CLI: `validate`, `run`,
//! `replay`, `step`, `input`, `screenshot`, `state`, `logs`, `profile`,
//! `stop` on server-minted console handles, and `deploy`, which pushes
//! a cart directory to another desktop's development receiver.
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

pub use protocol::{serve, serve_with, Server, MAX_LINE_BYTES, PROTOCOL_VERSION};
pub use session::{
    DeployFn, DeployOutcome, DeployRequest, Session, ToolError, TransportFactory, MAX_CONSOLES,
};

/// The API reference, served as `kuula://docs/api.md`.
pub const API_MD: &str = include_str!("../../../docs/api.md");

/// The cart author's constraints, served as `kuula://docs/skill.md`.
pub const SKILL_MD: &str = include_str!("../../../docs/skill.md");

/// The song files `sfx` and `music` play, served as
/// `kuula://docs/songs.md`: the API reference's appendix.
pub const SONGS_MD: &str = include_str!("../../../docs/songs.md");

/// Serve on the process's stdin and stdout until stdin closes. Cart
/// paths are resolved under `root`, or the current directory. With
/// `transports`, the `run` tool's `net` argument can host or join;
/// without, it is refused (this crate opens no socket itself). With
/// `deploy`, the `deploy` tool can push a cart to a receiver; without,
/// it answers `deploy_unavailable`.
pub fn run_stdio(
    root: Option<PathBuf>,
    transports: Option<TransportFactory>,
    deploy: Option<DeployFn>,
) -> std::io::Result<()> {
    let root = match root {
        Some(r) => r,
        None => std::env::current_dir()?,
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let result = serve_with(stdin.lock(), &mut out, root, transports, deploy);
    out.flush()?;
    result
}
