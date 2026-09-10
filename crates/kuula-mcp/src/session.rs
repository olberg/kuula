//! Console handles and their bounded histories.
//!
//! A [`Session`] mints handles (`c1`, `c2`, ...) for live consoles, caps
//! how many exist at once, resolves cart paths under its root and keeps
//! per-console logs, profiles and queued inputs within fixed bounds
//!. Stopped handles are forgotten; a call on one is a
//! `stale_handle` error. Nothing here knows about JSON.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use kuula_core::transcript::{Header, Transcript};
use kuula_core::{
    Console, Fault, FrameInput, FrameProfile, MemoryStore, Recorder, RecordingGuest, SaveStore,
    SharedRecorder, Snapshot, SnapshotLimits,
};
use kuula_host_headless::OwnedFrame;
use kuula_lua::LuaGuest;

/// Most consoles alive at once per server.
pub const MAX_CONSOLES: usize = 8;

/// Log lines kept per console; the oldest are dropped first.
pub const MAX_LOG_LINES: usize = 2000;

/// Frame profiles kept per console.
pub const MAX_PROFILES: usize = 600;

/// Frames of input one console may have queued.
pub const MAX_QUEUED_INPUTS: usize = 100_000;

/// Frames one `run` or `step` call may advance: ten minutes at 60 Hz.
pub const MAX_FRAMES_PER_CALL: u64 = 36_000;

/// A tool-level failure with a stable code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub code: String,
    pub message: String,
}

impl ToolError {
    pub fn new(code: &str, message: impl Into<String>) -> ToolError {
        ToolError {
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn stale_handle(handle: &str) -> ToolError {
        ToolError::new(
            "stale_handle",
            format!("no live console {handle:?}; it was stopped or never existed"),
        )
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ToolError {}

/// One line the cart logged, and the frame it logged it on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub frame: u64,
    pub text: String,
}

/// What one stepped frame reported back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stepped {
    pub frame: u64,
    pub log: Vec<LogLine>,
    pub fault: Option<Fault>,
}

/// A live console with its bounded histories.
pub struct Live {
    console: Console,
    logs: VecDeque<LogLine>,
    profiles: VecDeque<(u64, FrameProfile)>,
    queued: VecDeque<FrameInput>,
    /// Frame the cart faulted on, once it has.
    fault_frame: Option<u64>,
    /// The inputs the cart saw, when `run` asked for a recording.
    recorder: Option<SharedRecorder>,
}

impl Live {
    fn new(console: Console, recorder: Option<SharedRecorder>) -> Live {
        Live {
            console,
            logs: VecDeque::new(),
            profiles: VecDeque::new(),
            queued: VecDeque::new(),
            fault_frame: None,
            recorder,
        }
    }

    /// The transcript so far, when recording, and whether it holds
    /// every frame (a run past the transcript limit drops the rest).
    pub fn transcript(&self) -> Option<(Transcript, bool)> {
        self.recorder.as_ref().map(|r| {
            let r = r.borrow();
            (r.transcript(), !r.overflowed())
        })
    }

    pub fn console(&self) -> &Console {
        &self.console
    }

    pub fn frame(&self) -> u64 {
        self.console.frame()
    }

    pub fn fault(&self) -> Option<(&Fault, u64)> {
        self.console
            .state()
            .fault()
            .map(|f| (f, self.fault_frame.unwrap_or(self.frame())))
    }

    pub fn is_running(&self) -> bool {
        self.console.state().fault().is_none()
    }

    /// Step one frame with `input`, recording its log and profile.
    pub fn step(&mut self, input: FrameInput) -> Stepped {
        let already_faulted = !self.is_running();
        let out = self.console.step(input);
        let frame = out.frame;
        let log: Vec<LogLine> = out
            .log
            .iter()
            .map(|l| LogLine {
                frame,
                text: clean_text(l),
            })
            .collect();
        let profile = out.profile.clone();
        for line in &log {
            if self.logs.len() >= MAX_LOG_LINES {
                self.logs.pop_front();
            }
            self.logs.push_back(line.clone());
        }
        if !already_faulted {
            if self.profiles.len() >= MAX_PROFILES {
                self.profiles.pop_front();
            }
            self.profiles.push_back((frame, profile));
        }
        let fault = self.console.state().fault().cloned();
        if fault.is_some() && self.fault_frame.is_none() {
            self.fault_frame = Some(frame);
        }
        Stepped { frame, log, fault }
    }

    /// Step `frames` frames taking inputs from `inputs` (missing entries
    /// are no input) or, when `inputs` is `None`, from the queue. Stops
    /// after the faulting frame.
    pub fn step_many(&mut self, frames: u64, inputs: Option<&[FrameInput]>) -> Vec<Stepped> {
        let mut out = Vec::new();
        for i in 0..frames {
            if !self.is_running() {
                break;
            }
            let input = match inputs {
                Some(list) => list.get(i as usize).copied().unwrap_or(FrameInput::NONE),
                None => self.queued.pop_front().unwrap_or(FrameInput::NONE),
            };
            let stepped = self.step(input);
            let faulted = stepped.fault.is_some();
            out.push(stepped);
            if faulted {
                break;
            }
        }
        out
    }

    /// Queue inputs for later steps without inputs of their own.
    pub fn queue_inputs(&mut self, inputs: &[FrameInput]) -> Result<usize, ToolError> {
        if self.queued.len() + inputs.len() > MAX_QUEUED_INPUTS {
            return Err(ToolError::new(
                "too_many_inputs",
                format!("queue would hold more than {MAX_QUEUED_INPUTS} frames; step first"),
            ));
        }
        self.queued.extend(inputs.iter().copied());
        Ok(self.queued.len())
    }

    pub fn queued(&self) -> usize {
        self.queued.len()
    }

    /// Log lines from `since_frame` on, oldest first.
    pub fn logs_since(&self, since_frame: u64) -> impl Iterator<Item = &LogLine> {
        self.logs.iter().filter(move |l| l.frame >= since_frame)
    }

    /// The last `n` frame profiles, oldest first.
    pub fn last_profiles(&self, n: usize) -> impl Iterator<Item = &(u64, FrameProfile)> {
        let skip = self.profiles.len().saturating_sub(n);
        self.profiles.iter().skip(skip)
    }

    /// The current screen as an owned frame, for the PNG encoder.
    pub fn frame_now(&self) -> OwnedFrame {
        let out = self.console.output();
        OwnedFrame {
            frame: out.frame,
            width: out.width,
            height: out.height,
            pixels: out.screen.to_vec(),
            palette: *out.palette,
            log: Vec::new(),
            state: self.console.state().clone(),
            profile: out.profile.clone(),
            audio: out.audio.to_vec(),
        }
    }

    /// The guest's canonical dump of `names`.
    pub fn state_dump(&mut self, names: &[String]) -> Result<String, Fault> {
        self.console.state_dump(names)
    }
}

/// Where the tools' consoles live.
pub struct Session {
    root: PathBuf,
    consoles: HashMap<String, Live>,
    next_handle: u64,
}

impl Session {
    pub fn new(root: PathBuf) -> Session {
        Session {
            root,
            consoles: HashMap::new(),
            next_handle: 1,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn live_count(&self) -> usize {
        self.consoles.len()
    }

    /// Resolve `cart` under the root; it must exist and stay inside.
    pub fn resolve(&self, cart: &str) -> Result<PathBuf, ToolError> {
        if cart.is_empty() {
            return Err(ToolError::new("cart_not_found", "cart path is empty"));
        }
        let root = self.root.canonicalize().map_err(|e| {
            ToolError::new(
                "root_unavailable",
                format!("server root {}: {e}", self.root.display()),
            )
        })?;
        let joined = self.root.join(cart);
        let dir = joined.canonicalize().map_err(|_| {
            ToolError::new(
                "cart_not_found",
                format!("{cart:?} is not a directory under the server root"),
            )
        })?;
        if !dir.starts_with(&root) {
            return Err(ToolError::new(
                "path_outside_root",
                format!("{cart:?} resolves outside the server root"),
            ));
        }
        if !dir.is_dir() {
            return Err(ToolError::new(
                "cart_not_found",
                format!("{cart:?} is not a directory"),
            ));
        }
        Ok(dir)
    }

    /// Snapshot a cart directory.
    pub fn snapshot(&self, cart: &str) -> Result<Snapshot, ToolError> {
        let dir = self.resolve(cart)?;
        Snapshot::from_dir(&dir, SnapshotLimits::default()).map_err(|e| {
            ToolError::new(
                "cart_read_error",
                format!("{} ({}): {}", e.path, e.code, e.message),
            )
        })
    }

    /// Snapshot a cart directory and build a console from it. A console
    /// that faulted at load is still returned, faulted, like the CLI.
    pub fn build(&self, cart: &str) -> Result<Console, ToolError> {
        let snap = self.snapshot(cart)?;
        Session::build_with(snap, false, &[]).map(|(c, _)| c)
    }

    /// Build a console from a snapshot already taken (so a caller that
    /// checked the tree builds from the same read), with an in-memory
    /// save store seeded from `saves` (a replay's initial slots) and,
    /// with `record`, a recorder of every input the cart sees.
    pub fn build_with(
        snap: Snapshot,
        record: bool,
        saves: &[(u8, Vec<u8>)],
    ) -> Result<(Console, Option<SharedRecorder>), ToolError> {
        let mut store = MemoryStore::from_slots(saves)
            .map_err(|e| ToolError::new("invalid_transcript", format!("initial saves: {e}")))?;
        let recorder = record.then(|| {
            Recorder::shared(Header::new(
                &snap,
                kuula_lua::RANDOM_SEED,
                store.all_slots(),
            ))
        });
        let mut console = Console::new(
            Rc::new(snap),
            RecordingGuest::factory(LuaGuest::factory, recorder.clone()),
        );
        console.set_save_store(Box::new(store));
        Ok((console, recorder))
    }

    /// Mint a handle for a console.
    pub fn open(&mut self, console: Console) -> Result<String, ToolError> {
        self.open_with(console, None)
    }

    /// Mint a handle for a console that may be recording.
    pub fn open_with(
        &mut self,
        console: Console,
        recorder: Option<SharedRecorder>,
    ) -> Result<String, ToolError> {
        if self.consoles.len() >= MAX_CONSOLES {
            return Err(ToolError::new(
                "too_many_consoles",
                format!("{MAX_CONSOLES} consoles are live; stop one first"),
            ));
        }
        let handle = format!("c{}", self.next_handle);
        self.next_handle += 1;
        self.consoles
            .insert(handle.clone(), Live::new(console, recorder));
        Ok(handle)
    }

    pub fn get(&mut self, handle: &str) -> Result<&mut Live, ToolError> {
        self.consoles
            .get_mut(handle)
            .ok_or_else(|| ToolError::stale_handle(handle))
    }

    /// Forget a handle; the transcript comes back when it was recording.
    /// Drop a console; a recording one yields its transcript and whether
    /// that transcript is complete.
    pub fn stop(&mut self, handle: &str) -> Result<Option<(Transcript, bool)>, ToolError> {
        self.consoles
            .remove(handle)
            .map(|live| live.transcript())
            .ok_or_else(|| ToolError::stale_handle(handle))
    }
}

/// Cart-provided text as plain text: control characters removed, tabs
/// kept as a space, so no terminal sequence passes through.
pub fn clean_text(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_are_removed() {
        assert_eq!(clean_text("a\x1b[31mb\tc\r\n"), "a[31mb c");
        assert_eq!(clean_text("plain"), "plain");
    }

    #[test]
    fn handles_are_minted_capped_and_forgotten() {
        let mut s = Session::new(PathBuf::from("."));
        let mut handles = Vec::new();
        for _ in 0..MAX_CONSOLES {
            let c = Console::faulted(Fault::new("x", "f", None, ""));
            handles.push(s.open(c).unwrap());
        }
        assert_eq!(handles[0], "c1");
        let c = Console::faulted(Fault::new("x", "f", None, ""));
        assert_eq!(s.open(c).unwrap_err().code, "too_many_consoles");
        s.stop("c1").unwrap();
        assert_eq!(s.stop("c1").unwrap_err().code, "stale_handle");
        assert_eq!(
            s.get("c1").err().map(|e| e.code),
            Some("stale_handle".into())
        );
        let c = Console::faulted(Fault::new("x", "f", None, ""));
        assert_eq!(s.open(c).unwrap(), "c9", "handles are never reused");
    }

    #[test]
    fn paths_stay_under_the_root() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let s = Session::new(root);
        assert!(s.resolve("src").is_ok());
        assert_eq!(s.resolve("..").unwrap_err().code, "path_outside_root");
        assert_eq!(
            s.resolve("../kuula-core").unwrap_err().code,
            "path_outside_root"
        );
        assert_eq!(s.resolve("nope").unwrap_err().code, "cart_not_found");
        assert_eq!(s.resolve("Cargo.toml").unwrap_err().code, "cart_not_found");
        assert_eq!(s.resolve("").unwrap_err().code, "cart_not_found");
    }
}
