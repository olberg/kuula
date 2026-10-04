//! Headless CLI runs, replay, capture and host timing.

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use kuula_core::net::NetEnv;
use kuula_core::transcript::{Header, NetRecord};
use kuula_core::{Console, FrameInput, MemoryStore, Recorder, ReplayGuest, SaveStore, Snapshot};
use kuula_host_headless::{InputScript, Linked, OwnedFrame, Paced, RunSummary, StepError, Stepper};

use super::{
    factory, net_link, read_transcript, snapshot, usage, write_transcript, CartRunner, NetMode,
    DEFAULT_FRAMES, EXIT_FAULT, EXIT_OK,
};

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

pub(super) fn load_script(path: Option<&Path>) -> Result<Vec<FrameInput>, u8> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let text = std::fs::read_to_string(path)
        .map_err(|e| usage(&format!("cannot read {}: {e}", path.display())))?;
    InputScript::parse(&text)
        .map(|s| s.frames)
        .map_err(|e| usage(&e.message))
}

pub(super) struct HeadlessRun<'a> {
    pub(super) dir: &'a Path,
    pub(super) frames: Option<u64>,
    pub(super) input: Option<&'a Path>,
    pub(super) out: Option<&'a Path>,
    pub(super) runner: CartRunner,
    pub(super) profile: bool,
    /// `screenshot` writes only the last frame as a PNG.
    pub(super) screenshot: Option<&'a Path>,
    pub(super) record: Option<&'a Path>,
    pub(super) replay: Option<&'a Path>,
    pub(super) replay_any_cart: bool,
    pub(super) net: Option<NetMode>,
    pub(super) relay: crate::netlink::RelayConfig,
    pub(super) timing: bool,
}

/// Whether the manifest declares the `net` service; a manifest that
/// does not parse declares nothing (the console reports it).
pub(super) fn declares_net(snap: &Snapshot) -> bool {
    snap.get(kuula_core::manifest::MANIFEST_FILE)
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|t| kuula_core::Manifest::parse(t).ok())
        .is_some_and(|m| m.has_service(kuula_core::Service::Net))
}

/// A headless run, in this process or through a worker, optionally
/// recording or replaying a transcript.
pub(super) fn run_headless(run: HeadlessRun<'_>) -> u8 {
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
    let identity = kuula_core::net::identity::Identity::new(&snap);
    let mut link = run.net.as_ref().map(|_| {
        let relay = run.relay.clone();
        net_link(
            move || relay.clone(),
            move || identity.clone(),
            Default::default,
        )
    });
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
