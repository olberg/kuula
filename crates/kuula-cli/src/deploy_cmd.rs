//! `kuula deploy`: push a development cart to another desktop over Iroh.
//! A host command like
//! `kuula net`: nothing here is cart API, and the worker never reaches
//! it. Only with the `net` feature; the handheld build says so.
//!
//! - `kuula deploy id` prints this installation's development endpoint
//!   id, the one a receiver approves.
//! - `kuula deploy approve <id>`, `revoke <id>` and `approved` edit the
//!   receiver's list of approved developers.
//! - `kuula deploy receive [--carts DIR] [--bind ADDR] [--relay URL]
//!   [--relay-only] [--once]` prints `ticket: ...` and `id: ...`, then a
//!   line per event. It has no console, so a restart is `not_run`;
//!   `kuula shell --dev-receiver` is the receiver that reloads.
//! - `kuula deploy push <cart> --to <ticket> [--relay URL] [--relay-only]`
//!   sends a cart directory (packed as `kuula build` packs it) or a
//!   `.zip`/`.cart` file, after validating it here.
//!
//! Exit codes: 0 installed (or a clean receiver), 1 refused by the
//! receiver or by the local checks, 2 usage (a bad id, ticket or cart
//! path), 3 network failure.

use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Subcommand;
use kuula_core::{Snapshot, SnapshotLimits};
use kuula_net::deploy::{
    DeployCode, DeployError, DeployEvent, DeployReceiver, DeployStore, Package, PushReport,
    ReceiverConfig, Refusal, StoreError,
};
use kuula_net::Code;

use crate::net_cmd::{install_ctrl_handler, interrupted, RelayArgs, EXIT_NET};
use crate::{EXIT_FAULT, EXIT_OK, EXIT_USAGE};

/// How often a wait checks for Ctrl+C.
const POLL: Duration = Duration::from_millis(200);

#[derive(Subcommand)]
pub enum DeployCommand {
    /// Print this installation's development endpoint id.
    Id,
    /// Approve a developer endpoint id to push carts to this receiver.
    Approve { id: String },
    /// Take an approval back.
    Revoke { id: String },
    /// List the approved developer ids.
    Approved,
    /// Receive carts pushed by approved developers, with no console.
    Receive {
        /// Where installed carts go (default: carts).
        #[arg(long)]
        carts: Option<PathBuf>,
        /// Bind this address only (default: every interface).
        #[arg(long)]
        bind: Option<SocketAddr>,
        #[command(flatten)]
        relay: RelayArgs,
        /// Exit after one transfer attempt.
        #[arg(long)]
        once: bool,
    },
    /// Push a cart to a receiver.
    Push {
        /// A cart directory, or a .zip/.cart file.
        cart: PathBuf,
        /// The receiver's ticket.
        #[arg(long)]
        to: String,
        #[command(flatten)]
        relay: RelayArgs,
    },
}

/// The installation's deploy files, under the directory `settings.kv`
/// lives in.
pub fn store() -> Result<DeployStore, String> {
    crate::settings::data_dir()
        .map(|dir| DeployStore::new(&dir))
        .ok_or_else(|| {
            "no data directory (set KUULA_SAVE_ROOT, or LOCALAPPDATA / HOME)".to_string()
        })
}

fn say(line: impl std::fmt::Display) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

/// A local failure with its code and the exit it earns.
fn fail(code: &str, detail: impl std::fmt::Display, exit: u8) -> u8 {
    eprintln!("error: {code} {detail}");
    exit
}

fn store_failure(e: StoreError) -> u8 {
    match e {
        StoreError::BadId(why) => {
            eprintln!("error: {why}");
            EXIT_USAGE
        }
        other => fail(DeployCode::Install.as_str(), other, EXIT_FAULT),
    }
}

pub fn main(command: DeployCommand) -> u8 {
    install_ctrl_handler();
    let store = match store() {
        Ok(s) => s,
        Err(why) => return fail(DeployCode::Install.as_str(), why, EXIT_FAULT),
    };
    match command {
        DeployCommand::Id => match store.id() {
            Ok(id) => {
                say(id);
                EXIT_OK
            }
            Err(e) => store_failure(e),
        },
        DeployCommand::Approve { id } => match store.approve(&id) {
            Ok(added) => {
                let id = kuula_net::deploy::store::parse_id(&id).unwrap_or(id);
                say(format!(
                    "{}: {id}",
                    if added {
                        "approved"
                    } else {
                        "already approved"
                    }
                ));
                EXIT_OK
            }
            Err(e) => store_failure(e),
        },
        DeployCommand::Revoke { id } => match store.revoke(&id) {
            Ok(removed) => {
                let id = kuula_net::deploy::store::parse_id(&id).unwrap_or(id);
                say(format!(
                    "{}: {id}",
                    if removed {
                        "revoked"
                    } else {
                        "was not approved"
                    }
                ));
                EXIT_OK
            }
            Err(e) => store_failure(e),
        },
        DeployCommand::Approved => {
            for id in store.approved() {
                say(id);
            }
            EXIT_OK
        }
        DeployCommand::Receive {
            carts,
            bind,
            relay,
            once,
        } => receive(store, carts, relay.config(bind), once),
        DeployCommand::Push { cart, to, relay } => push(&store, &cart, &to, relay.config(None)),
    }
}

/// One line for an event a receiver reports; the same text in `receive`
/// and in the shell's `--dev-receiver`.
pub fn describe(event: &DeployEvent) -> String {
    match event {
        DeployEvent::Unpaired { id } => {
            format!("unpaired: {id} was refused; approve it with: kuula deploy approve {id}")
        }
        DeployEvent::Busy { id } => format!("busy: {id} was refused during a transfer"),
        DeployEvent::Offered { id, name, bytes } => {
            format!("offer: {name} {bytes} bytes from {id}")
        }
        DeployEvent::Finished {
            id,
            name,
            bytes,
            code,
            restart,
            detail,
        } => {
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!(" ({detail})")
            };
            let name = if name.is_empty() { "-" } else { name };
            format!("finished: {code} {name} {bytes} bytes from {id}, restart {restart}{detail}")
        }
    }
}

fn receive(
    store: DeployStore,
    carts: Option<PathBuf>,
    net: kuula_net::NetConfig,
    once: bool,
) -> u8 {
    let carts = carts.unwrap_or_else(|| PathBuf::from("carts"));
    let receiver = match DeployReceiver::start(ReceiverConfig {
        carts: carts.clone(),
        store,
        net,
        restart: None,
    }) {
        Ok(r) => r,
        Err(e) => return net_failure(&e),
    };
    say(format!("ticket: {}", receiver.ticket()));
    say(format!("id: {}", receiver.id()));
    let addresses: Vec<String> = receiver.addresses().iter().map(|a| a.to_string()).collect();
    say(format!("addresses: {}", addresses.join(" ")));
    say(format!("carts: {}", carts.display()));
    loop {
        if interrupted() {
            say("interrupted");
            return EXIT_OK;
        }
        let Some(event) = receiver.next_event(POLL) else {
            continue;
        };
        say(describe(&event));
        if let DeployEvent::Finished { code, .. } = event {
            if once {
                return if code == DeployCode::Ok {
                    EXIT_OK
                } else {
                    EXIT_FAULT
                };
            }
        }
    }
}

/// The name and package of a cart given on the command line.
fn package(cart: &Path) -> Result<Package, u8> {
    let refused = |r: &Refusal| {
        // A name the wire cannot carry is a mistake in the invocation.
        let exit = if r.code == DeployCode::Offer {
            EXIT_USAGE
        } else {
            EXIT_FAULT
        };
        fail(r.code.as_str(), &r.detail, exit)
    };
    if crate::is_archive(cart) {
        let stem = cart.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        return Package::from_zip_file(stem, cart).map_err(|r| refused(&r));
    }
    if !cart.is_dir() {
        eprintln!(
            "error: {} is not a directory or a .zip/.cart file",
            cart.display()
        );
        return Err(EXIT_USAGE);
    }
    // `.` and `..` have no name of their own.
    let full = std::fs::canonicalize(cart).unwrap_or_else(|_| cart.to_path_buf());
    let name = full.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let snap = Snapshot::from_dir(cart, SnapshotLimits::default()).map_err(|e| {
        let path = if e.path.is_empty() {
            String::new()
        } else {
            format!(" {}", e.path)
        };
        fail(
            DeployCode::Invalid.as_str(),
            format!("{}{path}: {}", e.code, e.message),
            EXIT_FAULT,
        )
    })?;
    Package::from_snapshot(name, &snap).map_err(|r| refused(&r))
}

fn net_failure(e: &DeployError) -> u8 {
    match e {
        // A ticket that does not parse is a mistake in the invocation.
        DeployError::Net(n) if n.code == Code::Ticket => fail(e.code(), e.detail(), EXIT_USAGE),
        DeployError::Net(n) if n.code == Code::Cancelled && interrupted() => {
            fail(e.code(), "interrupted", EXIT_NET)
        }
        DeployError::Net(_) => fail(e.code(), e.detail(), EXIT_NET),
        DeployError::Refused(_) => fail(e.code(), e.detail(), EXIT_FAULT),
    }
}

fn print_report(report: &PushReport) {
    let stages = report.stages();
    say(format!(
        "push: {} {} bytes sha256 {}",
        report.name, report.bytes, report.digest
    ));
    say(format!("transfer: {}", stages.transfer));
    say(format!("validation: {}", stages.validation));
    say(format!("install: {}", stages.install));
    if report.installed() && !report.detail.is_empty() {
        say(format!("restart: {} {}", report.restart, report.detail));
    } else {
        say(format!("restart: {}", report.restart));
    }
    say(format!("result: {}", report.code));
}

/// The MCP `deploy` tool's push: the same transfer as `kuula deploy
/// push`, direct addresses only (no relay is configured for a tool
/// call), failures as tool errors with their `net_*` or `deploy_*` code.
pub fn mcp_push(
    request: kuula_mcp::DeployRequest,
) -> Result<kuula_mcp::DeployOutcome, kuula_mcp::ToolError> {
    use kuula_mcp::ToolError;
    let store = store().map_err(|why| ToolError::new(DeployCode::Install.as_str(), why))?;
    let secret = store
        .secret_key()
        .map_err(|e| ToolError::new(DeployCode::Install.as_str(), e.to_string()))?;
    let package = Package::from_snapshot(&request.name, &request.snapshot)
        .map_err(|r| ToolError::new(r.code.as_str(), r.detail))?;
    let net = kuula_net::NetConfig {
        enabled: true,
        ..Default::default()
    };
    let report = kuula_net::deploy::push(&net, secret, &request.to, &package)
        .map_err(|e| ToolError::new(e.code(), e.detail()))?;
    let stages = report.stages();
    Ok(kuula_mcp::DeployOutcome {
        name: report.name,
        bytes: report.bytes,
        digest: report.digest,
        code: report.code.as_str().to_string(),
        detail: report.detail,
        transfer: stages.transfer.into(),
        validation: stages.validation.into(),
        install: stages.install.into(),
        restart: report.restart.as_str().to_string(),
    })
}

fn push(store: &DeployStore, cart: &Path, to: &str, net: kuula_net::NetConfig) -> u8 {
    let package = match package(cart) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let secret = match store.secret_key() {
        Ok(k) => k,
        Err(e) => return store_failure(e),
    };
    match kuula_net::deploy::push(&net, secret, to, &package) {
        Ok(report) => {
            print_report(&report);
            if report.installed() {
                EXIT_OK
            } else {
                fail(report.code.as_str(), &report.detail, EXIT_FAULT)
            }
        }
        Err(e) => net_failure(&e),
    }
}
