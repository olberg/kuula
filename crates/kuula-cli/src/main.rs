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
//!   (`net_cmd`), the only path that opens a socket.
//!
//! Exit codes: 0 when the window is closed or the headless run finishes
//! with the cart still running, 1 when the cart faulted or the worker
//! failed, 2 on a usage error, 3 when `net` failed.

mod broker;
mod ipc;
mod net_cmd;
mod probe;
mod remote;
mod sandbox;
mod settings;
mod shell;
mod worker;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use std::time::Duration;

use clap::{Parser, Subcommand};
use kuula_core::net::{Link, NetEnv, Transport};
use kuula_core::transcript::{Header, NetRecord, Transcript, MAX_FILE_BYTES};
use kuula_core::{
    Console, FrameInput, Guest, MemoryStore, Preload, Recorder, RecordingGuest, ReplayGuest,
    SaveStore, SharedRecorder, Snapshot, SnapshotLimits, WriteThroughStore,
};
use kuula_host_headless::{InputScript, Linked, OwnedFrame, Paced, RunSummary, StepError, Stepper};
use kuula_host_sdl::HostOptions;
use kuula_lua::LuaGuest;
use kuula_net::IrohTransport;

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
}

#[derive(Subcommand)]
enum Command {
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
        /// Window scale, 1 to 4; the saved setting when absent.
        #[arg(long, value_parser = kuula_host_sdl::scale::parse)]
        scale: Option<u32>,
        /// Run carts in this process instead of a worker.
        #[arg(long)]
        in_process: bool,
        /// Debugging only: run the worker without its AppContainer token.
        #[arg(long, conflicts_with = "in_process")]
        no_sandbox: bool,
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
    #[command(hide = true)]
    Net {
        #[command(subcommand)]
        command: net_cmd::NetCommand,
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

/// The link every networked run steps through: an Iroh transport built
/// on the cart's first `host` or `join`, permitted from the start.
fn iroh_link() -> Link {
    let mut link = Link::new(Box::new(|| {
        Box::new(IrohTransport::new(None)) as Box<dyn Transport>
    }));
    link.set_permitted(true);
    link
}

/// A stepper that measures each step's wall time, for `--timing`.
struct Timed<'a> {
    inner: &'a mut dyn Stepper,
    times: Vec<Duration>,
}

impl Stepper for Timed<'_> {
    fn step(&mut self, input: FrameInput) -> Result<OwnedFrame, StepError> {
        let t = std::time::Instant::now();
        let out = self.inner.step(input);
        self.times.push(t.elapsed());
        out
    }
}

fn timing_line(mut times: Vec<Duration>) -> String {
    if times.is_empty() {
        return "timing: no steps".to_string();
    }
    times.sort();
    let median = times[times.len() / 2];
    let max = times[times.len() - 1];
    format!(
        "timing: {} steps, median {:.3} ms, max {:.3} ms",
        times.len(),
        median.as_secs_f64() * 1000.0,
        max.as_secs_f64() * 1000.0
    )
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
    let command = cli.command.unwrap_or(Command::Shell {
        carts: None,
        scale: None,
        in_process: false,
        no_sandbox: false,
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
            timing,
        } => {
            let headless =
                headless || frames.is_some() || worker || profile || replay.is_some() || timing;
            let net = match net.as_deref().map(NetMode::parse) {
                None => None,
                Some(Ok(mode)) => Some(mode),
                Some(Err(e)) => return ExitCode::from(usage(&e)),
            };
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
                        timing,
                    }),
                }
            } else {
                match CartRunner::new(in_process, no_sandbox, true) {
                    Err(code) => code,
                    Ok(runner) => run_window(&dir, scale, runner, record.as_deref(), net),
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
                    timing: false,
                })
            }
        }
        Command::Mcp { root } => match kuula_mcp::run_stdio(
            root,
            Some(std::rc::Rc::new(|| {
                Box::new(IrohTransport::new(None)) as Box<dyn Transport>
            })),
        ) {
            Ok(()) => EXIT_OK,
            Err(e) => {
                eprintln!("mcp: {e}");
                EXIT_FAULT
            }
        },
        Command::Shell {
            carts,
            scale,
            in_process,
            no_sandbox,
        } => match CartRunner::new(in_process, no_sandbox, true) {
            Err(code) => code,
            Ok(runner) => shell::run(carts.as_deref(), scale, runner),
        },
        Command::Build { dir, out } => build(&dir, &out),
        Command::Worker => worker::main(),
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

/// Whether a cart path names a packed cart rather than a directory.
fn is_archive(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("zip") || e.eq_ignore_ascii_case("cart"))
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
) -> u8 {
    let snap = match snapshot(dir) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let env = net.as_ref().map(NetMode::env).unwrap_or_default();
    let link = net.as_ref().map(|_| iroh_link());
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

fn load_script(path: Option<&Path>) -> Result<Vec<FrameInput>, u8> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let text = std::fs::read_to_string(path)
        .map_err(|e| usage(&format!("cannot read {}: {e}", path.display())))?;
    InputScript::parse(&text)
        .map(|s| s.frames)
        .map_err(|e| usage(&e.message))
}

struct HeadlessRun<'a> {
    dir: &'a Path,
    frames: Option<u64>,
    input: Option<&'a Path>,
    out: Option<&'a Path>,
    runner: CartRunner,
    profile: bool,
    /// `screenshot` writes only the last frame as a PNG.
    screenshot: Option<&'a Path>,
    record: Option<&'a Path>,
    replay: Option<&'a Path>,
    replay_any_cart: bool,
    net: Option<NetMode>,
    timing: bool,
}

/// Whether the manifest declares the `net` service; a manifest that
/// does not parse declares nothing (the console reports it).
fn declares_net(snap: &Snapshot) -> bool {
    snap.get(kuula_core::manifest::MANIFEST_FILE)
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|t| kuula_core::Manifest::parse(t).ok())
        .is_some_and(|m| m.has_service(kuula_core::Service::Net))
}

/// A headless run, in this process or through a worker, optionally
/// recording or replaying a transcript.
fn run_headless(run: HeadlessRun<'_>) -> u8 {
    let snap = match snapshot(run.dir) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let mut replay_net: Option<NetRecord> = None;
    let (inputs, mut store, frames) = match run.replay {
        Some(path) => {
            let transcript = match read_transcript(path, &snap, run.replay_any_cart) {
                Ok(t) => t,
                Err(code) => return code,
            };
            let store = match MemoryStore::from_slots(&transcript.header.saves) {
                Ok(s) => s,
                Err(e) => return usage(&format!("{}: initial saves: {e}", path.display())),
            };
            let frames = run.frames.unwrap_or(transcript.inputs.len() as u64);
            if transcript.net.is_some() && run.runner.exe.is_some() {
                return usage("a networked transcript replays in process, not with --worker");
            }
            replay_net = transcript.net;
            (transcript.inputs, store, frames)
        }
        None => match load_script(run.input) {
            Ok(inputs) => (
                inputs,
                MemoryStore::new(),
                run.frames.unwrap_or(DEFAULT_FRAMES),
            ),
            Err(code) => return code,
        },
    };
    // The environment: the run's own `--net`, or the recorded one. A
    // replay never builds a link, so no endpoint can be constructed.
    let env = match (&run.net, &replay_net) {
        (Some(mode), _) => mode.env(),
        (None, Some(record)) => record.env.clone(),
        (None, None) => NetEnv::default(),
    };
    let mut link = run.net.as_ref().map(|_| iroh_link());
    if run.net.is_some() && !declares_net(&snap) {
        eprintln!("note: --net given but the cart does not declare services = [\"net\"]");
    }
    if let Some(out) = run.out {
        if let Err(e) = std::fs::create_dir_all(out) {
            return usage(&format!("cannot create {}: {e}", out.display()));
        }
    }
    let recorder = run.record.map(|_| {
        Recorder::shared(Header::new(
            &snap,
            kuula_lua::RANDOM_SEED,
            store.all_slots(),
        ))
    });
    let inner = factory(&run.runner, recorder.clone());
    let mut console = Console::new_with(
        Rc::new(snap),
        ReplayGuest::factory(inner, replay_net),
        run.runner.preload(),
    );
    console.set_save_store(Box::new(store));
    console.set_net_env(env);

    let mut last: Option<OwnedFrame> = None;
    // First frame PNG that failed to write; later frames are not attempted.
    let mut write_error: Option<String> = None;
    // The run's PCM, written as one WAV beside the frames.
    let mut audio: Vec<i16> = Vec::new();
    let out = run.out;
    let screenshot = run.screenshot;
    let times;
    let mut sink = |n: u64, frame: &OwnedFrame| {
        for line in &frame.log {
            println!("{line}");
        }
        if out.is_some() {
            audio.extend_from_slice(&frame.audio);
        }
        if let (Some(out), None) = (out, &write_error) {
            let png = kuula_host_headless::frame_png(frame);
            let path = out.join(kuula_host_headless::frame_file_name(n));
            if let Err(e) = std::fs::write(&path, png) {
                write_error = Some(format!("cannot write {}: {e}", path.display()));
            }
        }
        if screenshot.is_some() {
            last = Some(frame.clone());
        }
    };
    let summary = match link.as_mut() {
        Some(link) => {
            // Paced to real time so a human can join a headless host;
            // the ticket is printed the way the diagnostic prints it.
            // The timer sits inside the pacer, so `--timing` reports
            // the poll and the step, not the wait for the next frame.
            let frame_time = Duration::from_nanos(1_000_000_000 / kuula_core::FRAME_RATE as u64);
            let mut linked = Linked::new(&mut console, link).on_hosting(|ticket| {
                println!("ticket: {ticket}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            });
            let mut timed = Timed {
                inner: &mut linked,
                times: Vec::new(),
            };
            let summary = {
                let mut paced = Paced::new(&mut timed, frame_time);
                kuula_host_headless::run(&mut paced, frames, &inputs, &mut sink)
            };
            times = timed.times;
            summary
        }
        None => {
            let mut timed = Timed {
                inner: &mut console,
                times: Vec::new(),
            };
            let summary = kuula_host_headless::run(&mut timed, frames, &inputs, &mut sink);
            times = timed.times;
            summary
        }
    };
    // The worker, if any, is stopped when the console drops; the link
    // goes with it, saying bye to a peer.
    drop(console);
    drop(link);
    if run.timing {
        eprintln!("{}", timing_line(times));
    }
    if let Some(e) = write_error {
        eprintln!("{e}");
        return EXIT_FAULT;
    }
    if let (Some(path), Some(r)) = (run.record, &recorder) {
        let code = write_transcript(path, r);
        if code != EXIT_OK {
            return code;
        }
    }

    if let (Some(path), Some(frame)) = (screenshot, &last) {
        if let Err(e) = std::fs::write(path, kuula_host_headless::frame_png(frame)) {
            eprintln!("cannot write {}: {e}", path.display());
            return EXIT_FAULT;
        }
    }
    if let Some(out) = out {
        if let Err(e) = kuula_host_headless::write_summary(out, &summary) {
            eprintln!("cannot write summary to {}: {e}", out.display());
            return EXIT_FAULT;
        }
        let path = out.join(kuula_host_headless::AUDIO_FILE);
        if let Err(e) = kuula_host_headless::write_wav(&path, &audio) {
            eprintln!("cannot write {}: {e}", path.display());
            return EXIT_FAULT;
        }
    }
    if run.profile {
        print!("{}", kuula_host_headless::profile_table(&summary.profile));
    }
    report(&summary)
}

/// Print the one-line outcome and pick the exit code.
fn report(summary: &RunSummary) -> u8 {
    println!(
        "frames {} hash {} state {}",
        summary.frames_run, summary.hash, summary.state
    );
    if let Some(e) = &summary.error {
        eprintln!("{e}");
        return EXIT_FAULT;
    }
    if let Some(f) = &summary.fault {
        let location = match f.line {
            Some(line) => format!("{}:{line}", f.file),
            None => f.file.clone(),
        };
        eprintln!("{} {location}: {}", f.code, f.message);
        return EXIT_FAULT;
    }
    EXIT_OK
}
