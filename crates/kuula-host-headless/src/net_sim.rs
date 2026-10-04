//! A bounded two-console harness shared by CLI and MCP. Memory runs have
//! no clock. Both event batches are polled before either console steps.

use crate::{frame_png, owned_frame, Hasher, OwnedFrame};
use kuula_core::console::GuestFactory;
use kuula_core::net::identity::{Identity, VerifiedTransport, HANDSHAKE_POLLS};
use kuula_core::net::{
    Command, Event, FailCode, MemoryTransport, NetEnv, Reason, Transport, INBOX_CAP, MEMORY_TICKET,
};
use kuula_core::transcript::{Header, Transcript};
use kuula_core::{Console, FrameInput, Recorder, RecordingGuest, SharedRecorder, Snapshot};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::rc::Rc;

pub const MAX_FRAMES: u64 = 3600;
pub const MAX_LOG_BYTES: usize = 64 * 1024;
/// The longest delivery delay and the longest contiguous stall, in
/// polls: under half the compatibility greeting timeout, so identical
/// carts always finish greeting each other within the simulated pacing.
pub const MAX_DELAY: u64 = HANDSHAKE_POLLS as u64 / 2 - 1;

/// The guest both peers run: the factory the console builds carts
/// with and the seed the transcripts record. The harness itself is
/// guest-agnostic; the CLI and MCP pass the Lua guest.
#[derive(Clone)]
pub struct Guest {
    pub factory: GuestFactory,
    pub seed: i64,
}

impl Guest {
    pub fn new<F: kuula_core::console::FactoryFn + 'static>(factory: F, seed: i64) -> Guest {
        Guest {
            factory: Rc::new(factory),
            seed,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scenario {
    pub config: Config,
    pub host_input: serde_json::Value,
    pub join_input: serde_json::Value,
}

impl Scenario {
    /// Both input scripts expanded to frames. The config is checked by
    /// the simulation itself.
    pub fn inputs(&self) -> Result<[Vec<FrameInput>; 2], String> {
        let parse = |v: &serde_json::Value| -> Result<Vec<FrameInput>, String> {
            let value = if v.is_null() {
                serde_json::Value::Array(Vec::new())
            } else {
                v.clone()
            };
            Ok(crate::InputScript::from_value(value)
                .map_err(|e| e.message)?
                .frames)
        };
        Ok([parse(&self.host_input)?, parse(&self.join_input)?])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stall {
    /// Peer 0 is host; peer 1 is joiner.
    pub peer: usize,
    pub start: u64,
    /// Exclusive end; frames count from one.
    pub end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub frames: u64,
    /// Delivery polls, each one unstalled frame with inbox room.
    pub delay: u64,
    pub capacity: usize,
    pub stalls: Vec<Stall>,
    /// End both sides at the start of this frame, discarding old queues.
    pub disconnect: Option<u64>,
    pub state: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            frames: 180,
            delay: 0,
            capacity: INBOX_CAP,
            stalls: Vec::new(),
            disconnect: None,
            state: vec!["game".into()],
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.frames == 0 || self.frames > MAX_FRAMES {
            return Err(format!("frames must be 1..={MAX_FRAMES}"));
        }
        if self.delay > MAX_DELAY || self.capacity == 0 || self.capacity > INBOX_CAP {
            return Err(format!(
                "delay must be <={MAX_DELAY} polls and capacity 1..=256"
            ));
        }
        if self.state.len() > 128 || self.state.iter().any(|s| s.is_empty() || s.len() > 128) {
            return Err("state accepts at most 128 names of 1..128 bytes".into());
        }
        if self.stalls.len() > 128
            || self
                .stalls
                .iter()
                .any(|s| s.peer > 1 || s.start == 0 || s.start >= s.end || s.end > self.frames + 1)
        {
            return Err(
                "stalls need peer 0/1 and 1 <= start < end <= frames+1; at most 128 ranges".into(),
            );
        }
        // A peer stalled longer than the greeting timeout would be refused
        // as unresponsive, so contiguous stalls are bounded like the delay.
        for peer in 0..2 {
            let mut runs: Vec<(u64, u64)> = self
                .stalls
                .iter()
                .filter(|s| s.peer == peer)
                .map(|s| (s.start, s.end))
                .collect();
            runs.sort_unstable();
            let mut merged: Vec<(u64, u64)> = Vec::new();
            for (start, end) in runs {
                match merged.last_mut() {
                    Some(last) if start <= last.1 => last.1 = last.1.max(end),
                    _ => merged.push((start, end)),
                }
            }
            if merged.iter().any(|(start, end)| end - start > MAX_DELAY) {
                return Err(format!(
                    "a peer may stall for at most {MAX_DELAY} contiguous frames"
                ));
            }
        }
        if self.disconnect.is_some_and(|f| f == 0 || f > self.frames) {
            return Err("disconnect must fall within the run".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub frame: u64,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct PeerReport {
    pub title: String,
    pub author: String,
    pub license: Option<kuula_core::manifest::License>,
    pub digest: String,
    pub frames: u64,
    pub hash: String,
    pub per_frame: Vec<String>,
    pub state: String,
    pub state_hash: String,
    pub fault: Option<crate::FaultSummary>,
    pub state_error: Option<String>,
    pub net_status: String,
    pub log: Vec<LogLine>,
    pub logs_truncated: bool,
}

#[derive(Serialize)]
pub struct Report {
    pub mode: String,
    pub config: Config,
    pub peers: [PeerReport; 2],
}

pub struct Run {
    pub report: Report,
    pub transcripts: [Transcript; 2],
    pub captures: [Vec<u8>; 2],
}

struct Peer {
    console: Console,
    recorder: SharedRecorder,
    hash: Hasher,
    per_frame: Vec<String>,
    log: Vec<LogLine>,
    log_bytes: usize,
    truncated: bool,
    last: Option<OwnedFrame>,
    identity: Identity,
}

impl Peer {
    fn new(snapshot: Snapshot, invite: Option<String>, guest: &Guest) -> Self {
        let identity = Identity::new(&snapshot);
        let recorder = Recorder::shared(Header::new(&snapshot, guest.seed, Vec::new()));
        let factory = guest.factory.clone();
        let mut console = Console::new(
            Rc::new(snapshot),
            RecordingGuest::factory(
                move |text: &str, name: &str| factory(text, name),
                Some(recorder.clone()),
            ),
        );
        console.set_net_env(NetEnv {
            permitted: true,
            invite,
        });
        Self {
            console,
            recorder,
            hash: Hasher::new(),
            per_frame: Vec::new(),
            log: Vec::new(),
            log_bytes: 0,
            truncated: false,
            last: None,
            identity,
        }
    }

    fn step(&mut self, frame: u64, input: FrameInput, events: Vec<Event>) {
        self.console.step_with(input, events);
        let out = owned_frame(&self.console);
        self.hash.frame(&out);
        let mut hash = Hasher::new();
        hash.frame(&out);
        self.per_frame.push(hash.hex());
        for text in &out.log {
            if self.log_bytes + text.len() <= MAX_LOG_BYTES && self.log.len() < 4096 {
                self.log_bytes += text.len();
                self.log.push(LogLine {
                    frame,
                    text: clean(text),
                });
            } else {
                self.truncated = true;
            }
        }
        self.last = Some(out);
    }

    fn finish(mut self, names: &[String]) -> Result<(PeerReport, Transcript, Vec<u8>), String> {
        let (state, state_error) = match self.console.state_dump(names) {
            Ok(state) => (state, None),
            Err(f) => (
                String::new(),
                Some(format!("{}: {}", f.code, clean(&f.message))),
            ),
        };
        let mut hash = Hasher::new();
        hash.bytes(state.as_bytes());
        let manifest = self.console.manifest();
        let report = PeerReport {
            title: clean(&manifest.title),
            author: clean(&manifest.author),
            license: manifest.license.clone(),
            digest: self.identity.hex(),
            frames: self.per_frame.len() as u64,
            hash: self.hash.hex(),
            per_frame: self.per_frame,
            state,
            state_hash: hash.hex(),
            fault: self.console.state().fault().map(|f| crate::FaultSummary {
                code: f.code.clone(),
                file: clean(&f.file),
                line: f.line,
                message: clean(&f.message),
                frame: self.console.frame(),
            }),
            state_error,
            net_status: self
                .console
                .net_state()
                .map_or("off", |s| s.status.as_str())
                .into(),
            log: self.log,
            logs_truncated: self.truncated,
        };
        if self.recorder.borrow().overflowed() {
            return Err("simulation transcript limit exceeded".into());
        }
        let transcript = self.recorder.borrow().transcript();
        let capture = frame_png(self.last.as_ref().expect("at least one frame"));
        Ok((report, transcript, capture))
    }
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// What both peers see after `config.disconnect`: the link is gone, so
/// a cart that hosts or joins again is told so instead of waiting for
/// an answer that never comes.
struct Severed {
    pending: Vec<Event>,
}

impl Transport for Severed {
    fn push(&mut self, cmd: Command) {
        if matches!(cmd, Command::Host | Command::Join { .. }) {
            self.pending
                .push(Event::failed(FailCode::Connect, "link ended by scenario"));
        }
    }
    fn poll(&mut self, out: &mut Vec<Event>, max: usize) {
        let n = self.pending.len().min(max);
        out.extend(self.pending.drain(..n));
    }
}

/// Reproducible oracle. Nothing constructs an endpoint or reads a clock.
pub fn simulate(
    guest: &Guest,
    snapshots: [Snapshot; 2],
    config: Config,
    inputs: [Vec<FrameInput>; 2],
) -> Result<Run, String> {
    let (a, b) = MemoryTransport::pair();
    a.set_delay(config.delay);
    a.set_capacity(config.capacity);
    simulate_with(
        guest,
        snapshots,
        config,
        inputs,
        [Box::new(a), Box::new(b)],
        MEMORY_TICKET.into(),
        "memory",
        |_| {},
    )
}

/// Adapter check: the caller provides transports, a join ticket and any
/// pacing. It must label the run separately from the deterministic oracle.
#[allow(clippy::too_many_arguments)]
pub fn simulate_with(
    guest: &Guest,
    snapshots: [Snapshot; 2],
    config: Config,
    inputs: [Vec<FrameInput>; 2],
    transports: [Box<dyn Transport>; 2],
    ticket: String,
    mode: &str,
    mut before_frame: impl FnMut(u64),
) -> Result<Run, String> {
    config.validate()?;
    if inputs.iter().any(|i| i.len() > MAX_FRAMES as usize) {
        return Err(format!("input exceeds {MAX_FRAMES} frames"));
    }
    let [a, b] = snapshots;
    let mut peers = [Peer::new(a, None, guest), Peer::new(b, Some(ticket), guest)];
    if peers.iter().any(|p| p.console.net_state().is_none()) {
        return Err("both carts must declare services = [\"net\"]".into());
    }
    let [a, b] = transports;
    let mut transports: [Box<dyn Transport>; 2] = [
        Box::new(VerifiedTransport::new(a, peers[0].identity.clone())),
        Box::new(VerifiedTransport::new(b, peers[1].identity.clone())),
    ];
    for frame in 1..=config.frames {
        before_frame(frame);
        let mut batches = [Vec::new(), Vec::new()];
        if config.disconnect == Some(frame) {
            transports = [
                Box::new(Severed {
                    pending: Vec::new(),
                }),
                Box::new(Severed {
                    pending: Vec::new(),
                }),
            ];
            for events in &mut batches {
                events.push(Event::Disconnected {
                    reason: Reason::Lost,
                });
            }
        }
        // Poll both before either executes: no same-frame advantage.
        for i in 0..2 {
            let stalled = config
                .stalls
                .iter()
                .any(|s| s.peer == i && s.start <= frame && frame < s.end);
            if !stalled {
                transports[i].poll(&mut batches[i], peers[i].console.net_room());
            }
        }
        let [a, b] = batches;
        peers[0].step(
            frame,
            inputs[0]
                .get(frame as usize - 1)
                .copied()
                .unwrap_or(FrameInput::NONE),
            a,
        );
        peers[1].step(
            frame,
            inputs[1]
                .get(frame as usize - 1)
                .copied()
                .unwrap_or(FrameInput::NONE),
            b,
        );
        for i in 0..2 {
            for command in peers[i].console.take_net_commands() {
                transports[i].push(command);
            }
        }
        if peers.iter().any(|p| p.console.state().fault().is_some()) {
            break;
        }
    }
    drop(transports);
    let [a, b] = peers;
    let (a, ta, ca) = a.finish(&config.state)?;
    let (b, tb, cb) = b.finish(&config.state)?;
    Ok(Run {
        report: Report {
            mode: mode.into(),
            config,
            peers: [a, b],
        },
        transcripts: [ta, tb],
        captures: [ca, cb],
    })
}

impl Run {
    pub fn write(&self, dir: &Path) -> Result<(), String> {
        let transcripts = self
            .transcripts
            .iter()
            .map(Transcript::encode)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        for (i, name) in ["host", "join"].iter().enumerate() {
            std::fs::write(dir.join(format!("{name}.kr")), &transcripts[i])
                .map_err(|e| e.to_string())?;
            std::fs::write(dir.join(format!("{name}.png")), &self.captures[i])
                .map_err(|e| e.to_string())?;
            std::fs::write(
                dir.join(format!("{name}-hashes.txt")),
                self.report.peers[i].per_frame.join("\n") + "\n",
            )
            .map_err(|e| e.to_string())?;
        }
        std::fs::write(
            dir.join("run.json"),
            serde_json::to_vec_pretty(&self.report).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }
}
