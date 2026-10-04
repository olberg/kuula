//! CLI front end to the shared pair harness; optional real loopback adapter.

use clap::Args;
use kuula_host_headless::net_sim::{self, Guest, Scenario};
use std::path::PathBuf;

#[derive(Args)]
pub struct Options {
    pub cart: PathBuf,
    /// Joiner's cart; defaults to the host cart. Mismatches are refused.
    #[arg(long)]
    pub peer_cart: Option<PathBuf>,
    /// JSON {config, host_input, join_input}; inputs use the regular script format.
    #[arg(long)]
    pub scenario: Option<PathBuf>,
    #[arg(long)]
    pub frames: Option<u64>,
    /// Directory for both transcripts, final PNGs, hashes and run.json.
    #[arg(long)]
    pub out: PathBuf,
    /// Real-Iroh adapter check, paced to 60 Hz. Not deterministic.
    /// Needs a build with the `net` feature.
    #[arg(long)]
    pub loopback: bool,
}

pub fn run(options: Options) -> u8 {
    match execute(options) {
        Ok(code) => code,
        Err(Failure::Usage(e)) => super::usage(&e),
        Err(Failure::Exit(code)) => code,
    }
}

/// Why a run stopped before simulating: a usage error still to report,
/// or a cart that could not be read, already reported with its exit
/// code by `snapshot`.
enum Failure {
    Usage(String),
    Exit(u8),
}

impl From<String> for Failure {
    fn from(e: String) -> Failure {
        Failure::Usage(e)
    }
}

impl From<&str> for Failure {
    fn from(e: &str) -> Failure {
        Failure::Usage(e.into())
    }
}

impl From<u8> for Failure {
    fn from(code: u8) -> Failure {
        Failure::Exit(code)
    }
}

fn execute(options: Options) -> Result<u8, Failure> {
    let mut scenario: Scenario = match &options.scenario {
        None => Scenario::default(),
        Some(path) => {
            if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
                return Err("scenario exceeds 1 MiB".into());
            }
            serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?
        }
    };
    if let Some(frames) = options.frames {
        scenario.config.frames = frames;
    }
    let inputs = scenario.inputs()?;
    if options.loopback
        && (scenario.config.delay != 0
            || scenario.config.capacity != kuula_core::net::INBOX_CAP
            || !scenario.config.stalls.is_empty())
    {
        return Err(
            "delay, capacity and stalls belong to memory simulation; omit them for --loopback"
                .into(),
        );
    }
    let host = super::snapshot(&options.cart)?;
    let join = super::snapshot(options.peer_cart.as_ref().unwrap_or(&options.cart))?;
    let guest = Guest::new(kuula_lua::LuaGuest::factory, kuula_lua::RANDOM_SEED);
    let run = if options.loopback {
        loopback::simulate(&guest, [host, join], scenario.config, inputs)?
    } else {
        net_sim::simulate(&guest, [host, join], scenario.config, inputs)?
    };
    run.write(&options.out)?;
    println!(
        "{}",
        serde_json::to_string(&run.report).map_err(|e| e.to_string())?
    );
    Ok(if run.report.peers.iter().any(|p| p.fault.is_some()) {
        1
    } else {
        0
    })
}

#[cfg(feature = "net")]
mod loopback;

/// Without the `net` feature there is no Iroh to loop back through.
#[cfg(not(feature = "net"))]
mod loopback {
    use kuula_core::{FrameInput, Snapshot};
    use kuula_host_headless::net_sim::{Config, Guest, Run};

    pub fn simulate(
        _: &Guest,
        _: [Snapshot; 2],
        _: Config,
        _: [Vec<FrameInput>; 2],
    ) -> Result<Run, String> {
        Err(crate::netlink::unavailable().unwrap_or_default().into())
    }
}
