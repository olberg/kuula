//! Headless host: a scripted list of `FrameInput`s in, indexed PNG frames,
//! a WAV of the audio, per-frame canonical hashes and a run summary out.
//! No window, no clock.
//! The CLI's `--headless` and `screenshot` and the conformance tests all
//! sit on this. The thing being stepped is a [`Stepper`], so a console in
//! this process and a console in a worker process run the same loop.

pub mod hash;
pub mod script;

use std::path::Path;
use std::time::{Duration, Instant};

use kuula_core::net::{Event, Link};
use kuula_core::{
    Category, Console, ConsoleState, Fault, FrameInput, FrameProfile, CATEGORY_COUNT, PALETTE_SIZE,
};

pub use hash::Hasher;
pub use script::InputScript;

/// One frame, owned: what a host needs to present, hash or encode it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedFrame {
    pub frame: u64,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub palette: [[u8; 3]; PALETTE_SIZE],
    pub log: Vec<String>,
    pub state: ConsoleState,
    pub profile: FrameProfile,
    /// The frame's PCM: 44.1 kHz mono 16-bit, `SAMPLES_PER_FRAME` long.
    pub audio: Vec<i16>,
}

impl OwnedFrame {
    pub fn fault(&self) -> Option<&Fault> {
        self.state.fault()
    }
}

/// Something that runs one frame at a time.
pub trait Stepper {
    fn step(&mut self, input: FrameInput) -> Result<OwnedFrame, StepError>;
}

/// The stepper itself failed, as opposed to the cart faulting: a worker
/// process died, spoke garbage or stopped answering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepError {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for StepError {}

/// The console's current frame, owned.
pub fn owned_frame(console: &Console) -> OwnedFrame {
    let out = console.output();
    OwnedFrame {
        frame: out.frame,
        width: out.width,
        height: out.height,
        pixels: out.screen.to_vec(),
        palette: *out.palette,
        log: out.log.to_vec(),
        profile: out.profile.clone(),
        audio: out.audio.to_vec(),
        state: console.state().clone(),
    }
}

/// One step with the network events the host offers, as an owned
/// frame. The worker and the in-process stepper both go through here.
pub fn step_console(console: &mut Console, input: FrameInput, events: Vec<Event>) -> OwnedFrame {
    console.step_with(input, events);
    owned_frame(console)
}

impl Stepper for Console {
    fn step(&mut self, input: FrameInput) -> Result<OwnedFrame, StepError> {
        Ok(step_console(self, input, Vec::new()))
    }
}

/// Told the ticket when hosting begins.
type OnHosting<'a> = Box<dyn FnMut(&str) + 'a>;

/// A console stepped through a network [`Link`]: what a headless run
/// with `--net` uses. Reports the ticket when hosting begins. Wrap it
/// in [`Paced`] to hold it to real time.
pub struct Linked<'a> {
    console: &'a mut Console,
    link: &'a mut Link,
    on_hosting: Option<OnHosting<'a>>,
}

impl<'a> Linked<'a> {
    pub fn new(console: &'a mut Console, link: &'a mut Link) -> Linked<'a> {
        Linked {
            console,
            link,
            on_hosting: None,
        }
    }

    /// Called with the ticket when a `hosting` event is admitted.
    pub fn on_hosting(mut self, f: impl FnMut(&str) + 'a) -> Linked<'a> {
        self.on_hosting = Some(Box::new(f));
        self
    }
}

impl Stepper for Linked<'_> {
    fn step(&mut self, input: FrameInput) -> Result<OwnedFrame, StepError> {
        let mut events = Vec::new();
        self.link.poll(&mut events, self.console.net_room());
        if let Some(f) = self.on_hosting.as_mut() {
            for e in &events {
                if let Event::Hosting { ticket } = e {
                    f(ticket);
                }
            }
        }
        let frame = step_console(self.console, input, events);
        let commands = self.console.take_net_commands();
        if !commands.is_empty() {
            self.link.push(commands);
        }
        Ok(frame)
    }
}

/// A stepper held to real time: sleeps so steps come no faster than
/// one per `frame`, then steps the inner one. How a headless host
/// waits for a human peer. Host-side only, never seen by the cart; put
/// a timer inside it, not around it, so `--timing` measures the work
/// and not the wait.
pub struct Paced<'a> {
    inner: &'a mut dyn Stepper,
    frame: Duration,
    next: Option<Instant>,
}

impl<'a> Paced<'a> {
    pub fn new(inner: &'a mut dyn Stepper, frame: Duration) -> Paced<'a> {
        Paced {
            inner,
            frame,
            next: None,
        }
    }
}

impl Stepper for Paced<'_> {
    fn step(&mut self, input: FrameInput) -> Result<OwnedFrame, StepError> {
        let now = Instant::now();
        let due = self.next.unwrap_or(now);
        if due > now {
            std::thread::sleep(due - now);
        }
        self.next = Some(due.max(now - self.frame) + self.frame);
        self.inner.step(input)
    }
}

/// Cycles over a whole run, by category: the profiler's output
///, written as `profile.json` and printed by
/// [`profile_table`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProfileSummary {
    /// The smallest budget any frame had: the first frame has a larger
    /// one, so this is the steady-state number even for a one-frame run.
    pub budget: u64,
    pub frames: u64,
    pub total: [u64; CATEGORY_COUNT],
    /// The frame that spent the most cycles, and its breakdown.
    pub peak_frame: u64,
    pub peak: [u64; CATEGORY_COUNT],
    pub lua_mem_peak: u64,
}

impl ProfileSummary {
    fn add(&mut self, frame: u64, p: &FrameProfile) {
        self.frames += 1;
        self.budget = if self.frames == 1 {
            p.budget
        } else {
            self.budget.min(p.budget)
        };
        for (t, c) in self.total.iter_mut().zip(p.cycles) {
            *t = t.saturating_add(c);
        }
        if p.total() > self.peak_cycles() || self.peak_frame == 0 {
            self.peak = p.cycles;
            self.peak_frame = frame;
        }
        self.lua_mem_peak = self.lua_mem_peak.max(p.lua_mem);
    }

    /// Saturating, like `FrameProfile::total`: a category can hold
    /// `u64::MAX`, and a worker's reply is not trusted to be smaller.
    pub fn total_cycles(&self) -> u64 {
        self.total.iter().fold(0u64, |a, &b| a.saturating_add(b))
    }

    pub fn peak_cycles(&self) -> u64 {
        self.peak.iter().fold(0u64, |a, &b| a.saturating_add(b))
    }
}

/// What one headless run produced, written as `run.json` by
/// [`write_summary`] and returned to callers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RunSummary {
    pub frames_requested: u64,
    pub frames_run: u64,
    pub width: u32,
    pub height: u32,
    /// Canonical hash over every frame in order.
    pub hash: String,
    pub per_frame: Vec<String>,
    /// `running`, `faulted` or `error`.
    pub state: String,
    pub fault: Option<FaultSummary>,
    /// Set when the stepper failed rather than the cart.
    pub error: Option<String>,
    pub log: Vec<String>,
    pub profile: ProfileSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FaultSummary {
    pub code: String,
    pub file: String,
    pub line: Option<u32>,
    pub message: String,
    pub frame: u64,
}

/// Step for `frames` frames with `inputs` (missing entries are no input),
/// calling `sink` with each frame and its 1-based sequence number, which
/// is the number `hashes.txt` and `frame_NNNNNN.png` use. The sequence
/// number is counted here rather than read from the frame, so a console
/// that faulted at load (its counter stays at 0) and a worker's reply
/// both get the same numbering as the hash list. Stops early on a fault,
/// after handing the faulting frame to the sink, or on a stepper error.
pub fn run(
    stepper: &mut dyn Stepper,
    frames: u64,
    inputs: &[FrameInput],
    mut sink: impl FnMut(u64, &OwnedFrame),
) -> RunSummary {
    let mut hasher = Hasher::new();
    let mut per_frame = Vec::new();
    let mut log = Vec::new();
    let mut frames_run = 0;
    let (mut width, mut height) = (0, 0);
    let mut fault = None;
    let mut error = None;
    let mut profile = ProfileSummary::default();
    for i in 0..frames {
        let input = inputs.get(i as usize).copied().unwrap_or(FrameInput::NONE);
        let out = match stepper.step(input) {
            Ok(out) => out,
            Err(e) => {
                error = Some(e.to_string());
                break;
            }
        };
        frames_run = i + 1;
        width = out.width;
        height = out.height;
        log.extend(out.log.iter().cloned());
        profile.add(frames_run, &out.profile);
        let mut one = Hasher::new();
        one.frame(&out);
        per_frame.push(one.hex());
        hasher.frame(&out);
        sink(frames_run, &out);
        if let Some(f) = out.fault() {
            fault = Some(FaultSummary {
                code: f.code.clone(),
                file: f.file.clone(),
                line: f.line,
                message: f.message.clone(),
                frame: out.frame,
            });
            break;
        }
    }
    let state = if error.is_some() {
        "error"
    } else if fault.is_some() {
        "faulted"
    } else {
        "running"
    };
    RunSummary {
        frames_requested: frames,
        frames_run,
        width,
        height,
        hash: hasher.hex(),
        per_frame,
        state: state.to_string(),
        fault,
        error,
        log,
        profile,
    }
}

/// The profile as a text table: one row per category with the run
/// total, the mean per frame, the peak frame's cycles and that peak as a
/// share of the frame budget, then a total row.
pub fn profile_table(p: &ProfileSummary) -> String {
    let frames = p.frames.max(1);
    let budget = p.budget.max(1) as f64;
    let mut out = format!(
        "{:<8} {:>12} {:>12} {:>12} {:>8}\n",
        "category", "total", "per frame", "peak frame", "peak %"
    );
    let mut row = |name: &str, total: u64, peak: u64| {
        out.push_str(&format!(
            "{:<8} {:>12} {:>12} {:>12} {:>7.1}%\n",
            name,
            total,
            total / frames,
            peak,
            peak as f64 * 100.0 / budget
        ));
    };
    for (i, c) in Category::ALL.iter().enumerate() {
        row(c.name(), p.total[i], p.peak[i]);
    }
    row("total", p.total_cycles(), p.peak_cycles());
    out.push_str(&format!(
        "frames {} budget {} peak frame {} lua heap peak {} bytes\n",
        p.frames, p.budget, p.peak_frame, p.lua_mem_peak
    ));
    out
}

/// Encode a frame as an indexed PNG. Indices are masked to the palette's
/// 7 bits, exactly as the SDL host displays them, so a cart that pokes a
/// value above 127 into the screen yields the same picture on both paths
/// and a PNG whose every index has a palette entry.
pub fn frame_png(out: &OwnedFrame) -> Vec<u8> {
    let pixels: Vec<u8> = out.pixels.iter().map(|p| p & 0x7f).collect();
    kuula_core::assets::encode_indexed_png(out.width, out.height, &pixels, &out.palette)
}

/// `frame_000001.png` and friends.
pub fn frame_file_name(frame: u64) -> String {
    format!("frame_{frame:06}.png")
}

/// The file the whole run's audio is written to beside the frames.
pub const AUDIO_FILE: &str = "audio.wav";

/// A 16-bit mono 44.1 kHz WAV: the 44-byte canonical header, then the
/// samples little-endian.
pub fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let rate = kuula_core::audio::SAMPLE_RATE;
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Write `samples` as a WAV file at `path`.
pub fn write_wav(path: &Path, samples: &[i16]) -> std::io::Result<()> {
    std::fs::write(path, wav_bytes(samples))
}

/// Write `hashes.txt`, `run.json` and `profile.json` for a finished run.
pub fn write_summary(dir: &Path, summary: &RunSummary) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("hashes.txt"), hashes_text(&summary.per_frame))?;
    let json = serde_json::to_string_pretty(summary).expect("summary serialises");
    std::fs::write(dir.join("run.json"), json)?;
    let json = serde_json::to_string_pretty(&summary.profile).expect("profile serialises");
    std::fs::write(dir.join("profile.json"), json)
}

/// The `hashes.txt` format: `000001 0x...` per line.
pub fn hashes_text(per_frame: &[String]) -> String {
    let mut text = String::new();
    for (i, h) in per_frame.iter().enumerate() {
        text.push_str(&format!("{:06} {h}\n", i + 1));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuula_core::{DrawState, Guest};

    struct Stub;

    impl Guest for Stub {
        fn step(
            &mut self,
            state: &mut DrawState,
            input: FrameInput,
            frame: u64,
        ) -> Result<(), Fault> {
            if frame == 4 {
                return Err(Fault::new(
                    Fault::RUNTIME_ERROR,
                    "main.lua",
                    Some(9),
                    "boom",
                ));
            }
            state.cls(input.buttons);
            state.log.push(format!("f{frame}"));
            state.profile = FrameProfile {
                cycles: [frame * 10, 5, 0, 0, 0, 0, 1],
                // The first frame has the larger init budget.
                budget: if frame == 1 { 6000 } else { 100 },
                lua_mem: 1000 * frame,
            };
            Ok(())
        }
    }

    #[test]
    fn run_stops_at_the_fault_and_reports_it() {
        let mut c = Console::from_guest(Box::new(Stub));
        let inputs = [FrameInput::new(1), FrameInput::new(2)];
        let mut seen = Vec::new();
        let s = run(&mut c, 10, &inputs, |n, out| {
            assert_eq!(n, out.frame);
            seen.push((out.frame, out.pixels[0]));
        });
        assert_eq!(seen, [(1, 1), (2, 2), (3, 0), (4, 0)]);
        assert_eq!(s.frames_run, 4);
        assert_eq!(s.frames_requested, 10);
        assert_eq!(s.state, "faulted");
        let f = s.fault.unwrap();
        assert_eq!(
            (f.code.as_str(), f.line, f.frame),
            ("runtime_error", Some(9), 4)
        );
        assert_eq!(s.log, ["f1", "f2", "f3"]);
        assert_eq!(s.per_frame.len(), 4);
        assert_eq!((s.width, s.height), (640, 480));
        assert_eq!(s.error, None);
    }

    #[test]
    fn the_summary_reports_the_steady_budget_and_never_overflows() {
        let mut c = Console::from_guest(Box::new(Stub));
        let s = run(&mut c, 1, &[], |_, _| {});
        assert_eq!(
            s.profile.budget, 6000,
            "a one-frame run has only the init budget"
        );
        let mut p = ProfileSummary::default();
        p.add(
            1,
            &FrameProfile {
                cycles: [u64::MAX, 7, 0, 0, 0, 0, 0],
                budget: 6000,
                lua_mem: 0,
            },
        );
        p.add(
            2,
            &FrameProfile {
                cycles: [u64::MAX, 0, 0, 0, 0, 0, 0],
                budget: 100,
                lua_mem: 0,
            },
        );
        assert_eq!(p.budget, 100);
        assert_eq!(p.total_cycles(), u64::MAX);
        assert_eq!(p.peak_cycles(), u64::MAX);
        assert_eq!(p.peak_frame, 1);
        let table = profile_table(&p);
        assert!(table.contains("budget 100"), "{table}");
    }

    #[test]
    fn the_profile_is_summed_and_the_peak_frame_found() {
        let mut c = Console::from_guest(Box::new(Stub));
        let s = run(&mut c, 3, &[], |_, _| {});
        let p = &s.profile;
        assert_eq!(p.frames, 3);
        assert_eq!(p.budget, 100);
        assert_eq!(p.total[0], 60);
        assert_eq!(p.total[1], 15);
        assert_eq!(p.total_cycles(), 60 + 15 + 3);
        assert_eq!(p.peak_frame, 3);
        assert_eq!(p.peak[0], 30);
        assert_eq!(p.peak_cycles(), 36);
        assert_eq!(p.lua_mem_peak, 3000);
        let table = profile_table(p);
        assert!(table.starts_with("category"), "{table}");
        let rows: Vec<String> = table
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();
        assert!(rows.contains(&"lua 60 20 30 30.0%".to_string()), "{table}");
        assert!(
            rows.contains(&"total 78 26 36 36.0%".to_string()),
            "{table}"
        );
        assert!(
            rows.contains(&"frames 3 budget 100 peak frame 3 lua heap peak 3000 bytes".to_string())
        );
        // A guest without a meter leaves the profile empty, not broken.
        let empty = profile_table(&ProfileSummary::default());
        assert!(empty.contains("total") && empty.contains("0.0%"), "{empty}");
    }

    struct Dying(u32);

    impl Stepper for Dying {
        fn step(&mut self, _: FrameInput) -> Result<OwnedFrame, StepError> {
            self.0 += 1;
            if self.0 == 3 {
                return Err(StepError {
                    code: "worker_error".into(),
                    message: "gone".into(),
                });
            }
            Ok(OwnedFrame {
                frame: self.0 as u64,
                width: 1,
                height: 1,
                pixels: vec![0],
                palette: [[0; 3]; PALETTE_SIZE],
                log: vec![],
                state: ConsoleState::Running,
                profile: FrameProfile::default(),
                audio: vec![],
            })
        }
    }

    #[test]
    fn a_stepper_error_ends_the_run_with_state_error() {
        let s = run(&mut Dying(0), 10, &[], |_, _| {});
        assert_eq!(s.frames_run, 2);
        assert_eq!(s.state, "error");
        assert_eq!(s.error.as_deref(), Some("worker_error: gone"));
    }

    #[test]
    fn frame_png_round_trips_indices_and_palette() {
        let mut c = Console::from_guest(Box::new(Stub));
        let out = Stepper::step(&mut c, FrameInput::new(5)).unwrap();
        let bytes = frame_png(&out);
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let info = reader.info().clone();
        assert_eq!((info.width, info.height), (640, 480));
        assert_eq!(info.color_type, png::ColorType::Indexed);
        let plte = info.palette.as_ref().unwrap();
        assert_eq!(plte.len(), 128 * 3);
        assert_eq!(&plte[5 * 3..6 * 3], &out.palette[5]);
        let mut buf = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut buf).unwrap();
        assert!(buf.iter().all(|&p| p == 5));
    }

    #[test]
    fn frames_carry_a_frame_of_audio_and_the_wav_header_is_right() {
        let mut c = Console::from_guest(Box::new(Stub));
        let out = Stepper::step(&mut c, FrameInput::NONE).unwrap();
        assert_eq!(out.audio.len(), kuula_core::audio::SAMPLES_PER_FRAME);
        let wav = wav_bytes(&[1, -2, 0x1234]);
        assert_eq!(wav.len(), 50);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 42);
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 44100);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 88200);
        assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(&wav[44..], &[1, 0, 0xfe, 0xff, 0x34, 0x12]);
        let dir = std::env::temp_dir().join(format!("kuula-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(AUDIO_FILE);
        write_wav(&path, &[7]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap().len(), 46);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn summary_files_are_written() {
        let dir = std::env::temp_dir().join(format!("kuula-headless-{}", std::process::id()));
        let mut c = Console::from_guest(Box::new(Stub));
        let s = run(&mut c, 2, &[], |_, _| {});
        write_summary(&dir, &s).unwrap();
        let hashes = std::fs::read_to_string(dir.join("hashes.txt")).unwrap();
        assert_eq!(hashes.lines().count(), 2);
        assert!(hashes.starts_with("000001 0x"));
        let json = std::fs::read_to_string(dir.join("run.json")).unwrap();
        assert!(json.contains("\"frames_run\": 2"));
        let profile = std::fs::read_to_string(dir.join("profile.json")).unwrap();
        assert!(profile.contains("\"peak_frame\": 2"), "{profile}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
