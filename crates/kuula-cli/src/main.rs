//! `kuula`: the command line.
//!
//! - `kuula run <dir> [--scale N] [--in-process] [--no-sandbox]
//!   [--record out.kr]` opens a window; the cart runs in a worker process
//!   under the sandbox unless `--in-process`.
//! - `kuula run <dir> --headless [--frames N] [--input script.json]
//!   [--out dir/] [--worker [--no-sandbox]] [--record out.kr]
//!   [--replay in.kr [--replay-any-cart]] [--timing]` steps without a
//!   window and writes PNG frames, per-frame hashes and a run summary.
//! - `--net host` or `--net join <ticket>` on either form permits a
//!   cart that declares `services = ["net"]` to use the network for
//!   that run; a headless host prints `ticket: ...` and is paced to
//!   real time so a peer can join it.
//! - `kuula screenshot <dir> --out file.png [--frame N] [--input script]`
//!   writes one frame.
//! - `kuula build <dir> --out cart.zip` packs the served entries of a
//!   cart directory deterministically; `run` and `screenshot` accept a
//!   `.zip` or `.cart` file as well as a directory.
//! - `kuula worker` is the hidden child process behind the worker.
//! - `kuula mcp [--root DIR]` serves the MCP tools on stdin/stdout.
//! - `kuula net listen|join` is the hidden networking diagnostic
//!   (`net_cmd`), separate from cart play.
//! - `kuula deploy id|approve|revoke|approved|receive|push` pushes a
//!   development cart to another desktop (`deploy_cmd`), and
//!   `kuula shell --dev-receiver` is the receiver that reloads it.
//!
//! Exit codes: 0 when the window is closed or the headless run finishes
//! with the cart still running, 1 when the cart faulted or the worker
//! failed, 2 on a usage error, 3 when `net` failed.

mod adb;
mod broker;
#[cfg(feature = "net")]
mod deploy_cmd;
mod headless;
mod ipc;
#[cfg(feature = "net")]
mod net_cmd;
mod net_sim;
mod netlink;
mod probe;
mod remote;
mod sandbox;
mod settings;
mod shell;
mod worker;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use clap::{Parser, Subcommand};
use kuula_core::net::NetEnv;
use kuula_core::transcript::{Header, Transcript, MAX_FILE_BYTES};
use kuula_core::{
    Console, Guest, MemoryStore, Preload, Recorder, RecordingGuest, SaveStore, SharedRecorder,
    Snapshot, SnapshotLimits, WriteThroughStore,
};
use kuula_host_common::carts::is_archive;
use kuula_host_sdl::HostOptions;
use kuula_lua::LuaGuest;

use headless::{declares_net, run_headless, HeadlessRun};
use netlink::{net_link, RelayConfig};
use remote::{RemoteConfig, RemoteGuest};

const EXIT_OK: u8 = 0;
const EXIT_FAULT: u8 = 1;
const EXIT_USAGE: u8 = 2;

/// Frames a headless run steps when `--frames` is not given.
const DEFAULT_FRAMES: u64 = 60;

#[derive(Parser)]
#[command(name = "kuula", version, about = "Kuula fantasy console")]
struct Cli {
    /// With no subcommand the shell boots, as `shell` does.
    #[command(subcommand)]
    command: Option<Command>,
    /// Shell only: also receive carts pushed by approved developers
    /// (see `kuula deploy`).
    #[arg(long)]
    dev_receiver: bool,
    /// With --dev-receiver: bind this address only.
    #[arg(long, requires = "dev_receiver")]
    bind: Option<std::net::SocketAddr>,
}

#[derive(Subcommand)]
enum Command {
    /// Run two headless carts with a reproducible network schedule.
    #[command(name = "net_sim", alias = "net-sim")]
    NetSim(net_sim::Options),
    /// Run a cart directory containing main.lua.
    Run {
        /// Directory containing main.lua.
        dir: PathBuf,
        /// Window scale, 1 to 4. Ctrl+1 to Ctrl+4 change it at runtime.
        #[arg(long, default_value_t = kuula_host_sdl::scale::DEFAULT_SCALE, value_parser = kuula_host_sdl::scale::parse)]
        scale: u32,
        /// Step without a window; see --frames, --input and --out.
        #[arg(long)]
        headless: bool,
        /// Frames to step headless (default 60, or the transcript's
        /// length with --replay). Implies --headless.
        #[arg(long)]
        frames: Option<u64>,
        /// JSON input script for a headless run.
        #[arg(long, conflicts_with = "replay")]
        input: Option<PathBuf>,
        /// Directory for frame PNGs, hashes.txt and run.json.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Headless: run the cart in a separate worker process.
        #[arg(long)]
        worker: bool,
        /// Windowed: run the cart in this process instead of a worker.
        #[arg(long, conflicts_with = "worker")]
        in_process: bool,
        /// Debugging only: run the worker without its AppContainer token.
        #[arg(long)]
        no_sandbox: bool,
        /// Print the cycle profile by category after a headless run.
        /// With --out, profile.json is written regardless.
        #[arg(long)]
        profile: bool,
        /// Write a transcript (.kr) of the inputs the cart saw.
        #[arg(long)]
        record: Option<PathBuf>,
        /// Headless: take the inputs and the initial saves from a
        /// transcript, in an isolated save store. Implies --headless.
        #[arg(long)]
        replay: Option<PathBuf>,
        /// Replay even when the cart is not the one recorded; the
        /// result then verifies nothing.
        #[arg(long, requires = "replay")]
        replay_any_cart: bool,
        /// Permit networking for this run: `host`, or `join <ticket>`.
        /// Only a cart that declares `services = ["net"]` can use it.
        #[arg(long, num_args = 1..=2, value_names = ["MODE", "TICKET"], conflicts_with = "replay")]
        net: Option<Vec<String>>,
        /// Explicit self-hosted relay URL; no public relay defaults.
        #[arg(long, requires = "net", conflicts_with = "replay")]
        relay: Option<String>,
        /// Disable IP transports and use only the configured relay.
        #[arg(long, requires = "relay")]
        relay_only: bool,
        /// Headless: print the median and maximum wall time of a step at
        /// exit (host side only; never cart-visible).
        #[arg(long)]
        timing: bool,
    },
    /// Step a cart headless and write one frame as a PNG.
    Screenshot {
        /// Directory containing main.lua.
        dir: PathBuf,
        /// PNG file to write.
        #[arg(long)]
        out: PathBuf,
        /// Which frame to capture, counting from 1.
        #[arg(long, default_value_t = 1)]
        frame: u64,
        /// JSON input script.
        #[arg(long)]
        input: Option<PathBuf>,
    },
    /// Serve the MCP tools (validate, run, step, screenshot, ...) on
    /// stdin/stdout for an agent client.
    Mcp {
        /// Directory cart paths are resolved under (default: cwd).
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Boot the shell: the cart list, pause overlay and settings.
    Shell {
        /// Directory the shell lists carts from (default: carts/, or
        /// examples/ when carts/ is missing).
        #[arg(long)]
        carts: Option<PathBuf>,
        /// One cart, a directory or a .zip or .cart file: the shell opens
        /// it at once and ends when it is quit, instead of listing carts.
        #[arg(long, conflicts_with_all = ["carts", "dev_receiver"])]
        cart: Option<PathBuf>,
        /// Window scale, 1 to 4; the saved setting when absent.
        #[arg(long, value_parser = kuula_host_sdl::scale::parse)]
        scale: Option<u32>,
        /// Run carts in this process instead of a worker.
        #[arg(long)]
        in_process: bool,
        /// Debugging only: run the worker without its AppContainer token.
        #[arg(long, conflicts_with = "in_process")]
        no_sandbox: bool,
        /// Also receive carts pushed by approved developers with
        /// `kuula deploy push`, and reload the running cart.
        #[arg(long)]
        dev_receiver: bool,
        /// With --dev-receiver: bind this address only.
        #[arg(long, requires = "dev_receiver")]
        bind: Option<std::net::SocketAddr>,
    },
    /// Pack a cart directory into a zip that `run` accepts.
    Build {
        /// Directory containing main.lua.
        dir: PathBuf,
        /// Zip file to write.
        #[arg(long)]
        out: PathBuf,
    },
    /// Internal: the worker process behind `run --worker`.
    #[command(hide = true)]
    Worker,
    /// Internal: the networking diagnostic, `listen` and `join`.
    #[cfg(feature = "net")]
    #[command(hide = true)]
    Net {
        #[command(subcommand)]
        command: net_cmd::NetCommand,
    },
    /// Push a development cart to another desktop: `id`, `approve`,
    /// `revoke`, `approved`, `receive` and `push`.
    #[cfg(feature = "net")]
    Deploy {
        #[command(subcommand)]
        command: deploy_cmd::DeployCommand,
    },
    /// Push a development cart: to an Android device over adb (`push
    /// <cart> --to adb`); this build has no networking for the rest.
    #[cfg(not(feature = "net"))]
    Deploy {
        #[command(subcommand)]
        command: Option<adb::OfflineDeploy>,
    },
    /// Internal: the hostile probe behind the sandbox tests.
    #[command(hide = true)]
    SandboxProbe {
        read: PathBuf,
        write: PathBuf,
        endpoint: String,
    },
    /// Internal: run this executable with `args` through the worker's
    /// own sandbox launcher.
    #[command(hide = true)]
    SandboxExec {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// How carts run: in this process, or in a worker under the sandbox.
#[derive(Clone)]
pub struct CartRunner {
    exe: Option<PathBuf>,
    sandboxed: bool,
    responsive: bool,
}

impl CartRunner {
    /// In-process when `in_process`; otherwise a worker, sandboxed unless
    /// `no_sandbox`, with bounded per-step waits when `responsive` (the
    /// windowed host). Locating the executable can fail.
    fn new(in_process: bool, no_sandbox: bool, responsive: bool) -> Result<CartRunner, u8> {
        let exe = if in_process {
            None
        } else {
            Some(std::env::current_exe().map_err(|e| {
                eprintln!(
                    "{}: cannot locate own executable: {e}",
                    broker::WORKER_ERROR
                );
                EXIT_FAULT
            })?)
        };
        Ok(CartRunner {
            exe,
            sandboxed: !no_sandbox,
            responsive,
        })
    }

    fn in_process() -> CartRunner {
        CartRunner {
            exe: None,
            sandboxed: true,
            responsive: false,
        }
    }

    /// Whether the broker decodes a cart's assets: only for in-process
    /// carts, a worker decodes its own.
    pub fn preload(&self) -> Preload {
        if self.exe.is_some() {
            Preload::Skip
        } else {
            Preload::Decode
        }
    }

    /// The worker configuration, or `None` for in-process carts.
    pub fn remote(&self, recorder: Option<SharedRecorder>) -> Option<RemoteConfig> {
        self.exe.as_ref().map(|exe| RemoteConfig {
            exe: exe.clone(),
            sandboxed: self.sandboxed,
            responsive: self.responsive,
            recorder,
        })
    }
}

/// A boxed guest factory with the console's signature.
type Factory = Box<dyn Fn(&str, &str) -> Result<Box<dyn Guest>, kuula_core::Fault>>;

/// A guest factory for the console: a worker, or the Lua guest in this
/// process, recording when asked.
fn factory(runner: &CartRunner, recorder: Option<SharedRecorder>) -> Factory {
    match runner.remote(recorder.clone()) {
        Some(config) => Box::new(RemoteGuest::factory(config)),
        None => Box::new(RecordingGuest::factory(LuaGuest::factory, recorder)),
    }
}

/// How a run was permitted to use the network.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NetMode {
    Host,
    Join(String),
}

impl NetMode {
    /// `--net host` or `--net join <ticket>`.
    fn parse(args: &[String]) -> Result<NetMode, String> {
        match args {
            [mode] if mode == "host" => Ok(NetMode::Host),
            [mode, ticket] if mode == "join" => {
                if ticket.len() > kuula_core::net::MAX_TICKET {
                    return Err("the ticket is too long".into());
                }
                Ok(NetMode::Join(ticket.clone()))
            }
            [mode] if mode == "join" => Err("--net join needs a ticket".into()),
            _ => Err("--net takes `host` or `join <ticket>`".into()),
        }
    }

    fn env(&self) -> NetEnv {
        NetEnv {
            permitted: true,
            invite: match self {
                NetMode::Host => None,
                NetMode::Join(t) => Some(t.clone()),
            },
        }
    }
}

/// The desktop save store for a cart: memory semantics for the cart,
/// the file store keyed by the cart's path behind it. Without a save
/// root the slots live in memory.
pub fn desktop_save_store(cart_path: &Path) -> Box<dyn SaveStore> {
    let identity = kuula_core::save::save_identity(cart_path);
    match kuula_core::save::default_save_root() {
        Some(root) => match kuula_core::FileStore::new(root, &identity) {
            Ok(store) => Box::new(WriteThroughStore::new(Box::new(store))),
            Err(e) => {
                eprintln!("saves stay in memory: {e}");
                Box::new(MemoryStore::new())
            }
        },
        None => Box::new(MemoryStore::new()),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.dev_receiver && !matches!(cli.command, None | Some(Command::Shell { .. })) {
        return ExitCode::from(plain_usage("--dev-receiver belongs to the shell"));
    }
    let command = cli.command.unwrap_or(Command::Shell {
        carts: None,
        cart: None,
        scale: None,
        in_process: false,
        no_sandbox: false,
        dev_receiver: false,
        bind: None,
    });
    let code = match command {
        Command::Run {
            dir,
            scale,
            headless,
            frames,
            input,
            out,
            worker,
            in_process,
            no_sandbox,
            profile,
            record,
            replay,
            replay_any_cart,
            net,
            relay,
            relay_only,
            timing,
        } => {
            let headless =
                headless || frames.is_some() || worker || profile || replay.is_some() || timing;
            let net = match net.as_deref().map(NetMode::parse) {
                None => None,
                Some(Ok(mode)) => Some(mode),
                Some(Err(e)) => return ExitCode::from(usage(&e)),
            };
            if let (Some(_), Some(why)) = (&net, netlink::unavailable()) {
                return ExitCode::from(usage(why));
            }
            let relay = RelayConfig {
                url: relay,
                only: relay_only,
            };
            if let Err(e) = relay.validate() {
                return ExitCode::from(usage(&e.to_string()));
            }
            if headless && in_process {
                usage("--in-process applies to windowed runs; headless runs are in-process unless --worker")
            } else if no_sandbox && (if headless { !worker } else { in_process }) {
                usage("--no-sandbox applies to a worker: --worker headless, or a windowed run without --in-process")
            } else if headless {
                match CartRunner::new(!worker, no_sandbox, false) {
                    Err(code) => code,
                    Ok(runner) => run_headless(HeadlessRun {
                        dir: &dir,
                        frames,
                        input: input.as_deref(),
                        out: out.as_deref(),
                        runner,
                        profile,
                        screenshot: None,
                        record: record.as_deref(),
                        replay: replay.as_deref(),
                        replay_any_cart,
                        net,
                        relay,
                        timing,
                    }),
                }
            } else {
                match CartRunner::new(in_process, no_sandbox, true) {
                    Err(code) => code,
                    Ok(runner) => run_window(&dir, scale, runner, record.as_deref(), net, relay),
                }
            }
        }
        Command::Screenshot {
            dir,
            out,
            frame,
            input,
        } => {
            if frame == 0 {
                eprintln!("error: --frame counts from 1");
                EXIT_USAGE
            } else {
                run_headless(HeadlessRun {
                    dir: &dir,
                    frames: Some(frame),
                    input: input.as_deref(),
                    out: None,
                    runner: CartRunner::in_process(),
                    profile: false,
                    screenshot: Some(&out),
                    record: None,
                    replay: None,
                    replay_any_cart: false,
                    net: None,
                    relay: Default::default(),
                    timing: false,
                })
            }
        }
        Command::Mcp { root } => match kuula_mcp::run_stdio(root, mcp_transport(), mcp_deploy()) {
            Ok(()) => EXIT_OK,
            Err(e) => {
                eprintln!("mcp: {e}");
                EXIT_FAULT
            }
        },
        Command::Shell {
            carts,
            cart,
            scale,
            in_process,
            no_sandbox,
            dev_receiver,
            bind,
        } => {
            // The flag may come before or after the `shell` word.
            let dev = (dev_receiver || cli.dev_receiver).then_some(shell::DevReceiver {
                bind: bind.or(cli.bind),
            });
            match (&dev, netlink::unavailable()) {
                // The flag before the `shell` word is another argument to
                // the parser, which only knows the one after it conflicts.
                (Some(_), _) if cart.is_some() => plain_usage(
                    "--cart and --dev-receiver do not go together: a shell on one cart installs no others",
                ),
                (Some(_), Some(why)) => plain_usage(why),
                _ => match CartRunner::new(in_process, no_sandbox, true) {
                    Err(code) => code,
                    Ok(runner) => {
                        let carts = match &cart {
                            Some(one) => shell::Carts::One(one),
                            None => shell::Carts::Listed(carts.as_deref()),
                        };
                        shell::run(carts, scale, runner, dev)
                    }
                },
            }
        }
        #[cfg(feature = "net")]
        Command::Deploy { command } => deploy_cmd::main(command),
        #[cfg(not(feature = "net"))]
        Command::Deploy { command } => match command {
            Some(adb::OfflineDeploy::Push {
                cart,
                to,
                screenshot,
            }) => match adb::target(&to) {
                Some(Ok(target)) => adb::push_cli(&cart, &target, screenshot.as_deref()),
                Some(Err(why)) => plain_usage(&why),
                // A receiver's ticket: that is networking.
                None => plain_usage(netlink::unavailable().unwrap_or_default()),
            },
            _ => plain_usage(netlink::unavailable().unwrap_or_default()),
        },
        Command::Build { dir, out } => build(&dir, &out),
        Command::NetSim(options) => net_sim::run(options),
        Command::Worker => worker::main(),
        #[cfg(feature = "net")]
        Command::Net { command } => net_cmd::main(command),
        Command::SandboxProbe {
            read,
            write,
            endpoint,
        } => probe::main(&read, &write, &endpoint),
        Command::SandboxExec { args } => sandbox::exec(&args),
    };
    ExitCode::from(code)
}

/// The transport the MCP server offers a networked cart: Iroh, or none
/// without the `net` feature.
#[cfg(feature = "net")]
fn mcp_transport() -> Option<kuula_mcp::TransportFactory> {
    Some(Rc::new(|| {
        Box::new(kuula_net::IrohTransport::new(None)) as Box<dyn kuula_core::net::Transport>
    }))
}

#[cfg(not(feature = "net"))]
fn mcp_transport() -> Option<kuula_mcp::TransportFactory> {
    None
}

/// The push the MCP `deploy` tool uses: to an Android device over `adb`
/// when the target says so, in any build, and otherwise to a receiver
/// over Iroh, which a build without the `net` feature has not.
fn mcp_deploy() -> Option<kuula_mcp::DeployFn> {
    Some(Rc::new(|request| match adb::target(&request.to) {
        Some(target) => adb::mcp_push(target, request),
        None => mcp_deploy_net(request),
    }))
}

#[cfg(feature = "net")]
fn mcp_deploy_net(
    request: kuula_mcp::DeployRequest,
) -> Result<kuula_mcp::DeployOutcome, kuula_mcp::ToolError> {
    deploy_cmd::mcp_push(request)
}

#[cfg(not(feature = "net"))]
fn mcp_deploy_net(
    _: kuula_mcp::DeployRequest,
) -> Result<kuula_mcp::DeployOutcome, kuula_mcp::ToolError> {
    Err(kuula_mcp::ToolError::new(
        "deploy_unavailable",
        "this server was started without networking",
    ))
}

/// `kuula build`: snapshot the directory and write it as a deterministic
/// zip. Only served entries are packed.
fn build(dir: &Path, out: &Path) -> u8 {
    if !dir.is_dir() {
        return usage(&format!("{} is not a directory", dir.display()));
    }
    let snap = match snapshot(dir) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let bytes = match kuula_core::zipsource::pack(&snap) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot pack {}: {e}", dir.display());
            return EXIT_FAULT;
        }
    };
    if let Err(e) = std::fs::write(out, &bytes) {
        eprintln!("cannot write {}: {e}", out.display());
        return EXIT_FAULT;
    }
    println!(
        "packed {} entries, {} bytes into {}",
        snap.len(),
        bytes.len(),
        out.display()
    );
    EXIT_OK
}

/// An error line and the usage exit code, for options that are not
/// about running a cart.
fn plain_usage(msg: &str) -> u8 {
    eprintln!("error: {msg}");
    EXIT_USAGE
}

fn usage(msg: &str) -> u8 {
    eprintln!("error: {msg}");
    eprintln!(
        "usage: kuula run <dir> [--scale N] [--headless ...]   (<dir> must contain main.lua)"
    );
    EXIT_USAGE
}

/// Validate the cart path, a directory or a `.zip`/`.cart` file, and take
/// its snapshot.
fn snapshot(dir: &Path) -> Result<Snapshot, u8> {
    if is_archive(dir) {
        let snap = Snapshot::from_zip(dir, SnapshotLimits::default()).map_err(|e| {
            eprintln!("cart_read_error {}: {}", e.path, e.message);
            EXIT_FAULT
        })?;
        if snap.get(kuula_core::console::MAIN_FILE).is_none() {
            return Err(usage(&format!(
                "{} does not contain main.lua",
                dir.display()
            )));
        }
        return Ok(snap);
    }
    if !dir.is_dir() {
        return Err(usage(&format!(
            "{} is not a directory or a .zip/.cart file",
            dir.display()
        )));
    }
    if !dir.join(kuula_core::console::MAIN_FILE).is_file() {
        return Err(usage(&format!(
            "{} does not contain main.lua",
            dir.display()
        )));
    }
    Snapshot::from_dir(dir, SnapshotLimits::default()).map_err(|e| {
        eprintln!("cart_read_error {}: {}", e.path, e.message);
        EXIT_FAULT
    })
}

/// Write the recorder's transcript to `path`; a partial recording is
/// written and said so.
fn write_transcript(path: &Path, recorder: &SharedRecorder) -> u8 {
    let recorder = recorder.borrow();
    if recorder.overflowed() {
        eprintln!(
            "warning: the run outgrew a transcript; {} holds its first {} frames",
            path.display(),
            recorder.frames()
        );
    }
    let text = match recorder.transcript().encode() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot encode transcript: {e}");
            return EXIT_FAULT;
        }
    };
    if let Err(e) = std::fs::write(path, text) {
        eprintln!("cannot write {}: {e}", path.display());
        return EXIT_FAULT;
    }
    println!(
        "recorded {} frames into {}",
        recorder.frames(),
        path.display()
    );
    EXIT_OK
}

/// Read and check a transcript for replaying `snap`.
fn read_transcript(path: &Path, snap: &Snapshot, any_cart: bool) -> Result<Transcript, u8> {
    let len = std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| usage(&format!("cannot read {}: {e}", path.display())))?;
    if len > MAX_FILE_BYTES as u64 {
        return Err(usage(&format!(
            "{} is over {MAX_FILE_BYTES} bytes",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| usage(&format!("cannot read {}: {e}", path.display())))?;
    let transcript =
        Transcript::decode(&text).map_err(|e| usage(&format!("{}: {e}", path.display())))?;
    if let Err(e) = transcript.check_cart(snap) {
        if any_cart {
            eprintln!("warning: {e}; the replay verifies nothing");
        } else {
            return Err(usage(&format!(
                "{e} (pass --replay-any-cart to replay anyway)"
            )));
        }
    }
    if transcript.header.seed != kuula_lua::RANDOM_SEED {
        return Err(usage(&format!(
            "the transcript was recorded with seed {}, this runtime uses {}",
            transcript.header.seed,
            kuula_lua::RANDOM_SEED
        )));
    }
    if transcript.header.runtime != env!("CARGO_PKG_VERSION") {
        eprintln!(
            "warning: the transcript was recorded by runtime {}, this is {}",
            transcript.header.runtime,
            env!("CARGO_PKG_VERSION")
        );
    }
    Ok(transcript)
}

fn run_window(
    dir: &Path,
    scale: u32,
    runner: CartRunner,
    record: Option<&Path>,
    net: Option<NetMode>,
    relay: RelayConfig,
) -> u8 {
    let snap = match snapshot(dir) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let env = net.as_ref().map(NetMode::env).unwrap_or_default();
    let identity = kuula_core::net::identity::Identity::new(&snap);
    let link = net.as_ref().map(|_| {
        net_link(
            move || relay.clone(),
            move || identity.clone(),
            Default::default,
        )
    });
    if net.is_some() && !declares_net(&snap) {
        eprintln!("note: --net given but the cart does not declare services = [\"net\"]");
    }
    let title = format!(
        "Kuula - {}",
        dir.file_name().and_then(|s| s.to_str()).unwrap_or("cart")
    );
    // Restarting from the error screen rebuilds the console from the same
    // snapshot; an edit on disk after `run` is not picked up.
    let snap = Rc::new(snap);
    // A transcript covers one run of the cart: a restart from the error
    // screen starts the cart over, which the format cannot express, so
    // recording stops at the first restart.
    let mut recorder: Option<SharedRecorder> = None;
    let mut made = 0u32;
    let mut make = || {
        let mut store = desktop_save_store(dir);
        made += 1;
        let this_run = match (record, made) {
            (Some(_), 1) => {
                let slots = store.all_slots();
                let r = Recorder::shared(Header::new(&snap, kuula_lua::RANDOM_SEED, slots));
                recorder = Some(r.clone());
                Some(r)
            }
            (Some(_), _) => {
                eprintln!("note: the cart restarted; the transcript covers its first run");
                None
            }
            (None, _) => None,
        };
        let mut console =
            Console::new_with(snap.clone(), factory(&runner, this_run), runner.preload());
        console.set_save_store(store);
        console.set_net_env(env.clone());
        console
    };
    let opts = HostOptions {
        link,
        ..HostOptions::new(scale, title)
    };
    let outcome = match kuula_host_sdl::run(&mut make, opts) {
        Err(e) => {
            eprintln!("host error: {e}");
            EXIT_FAULT
        }
        // The host already printed the fault when it happened.
        Ok((_, kuula_core::ConsoleState::Running)) => EXIT_OK,
        Ok((_, kuula_core::ConsoleState::Faulted(_))) => EXIT_FAULT,
    };
    if let (Some(path), Some(r)) = (record, &recorder) {
        let code = write_transcript(path, r);
        if code != EXIT_OK {
            return code;
        }
    }
    outcome
}
