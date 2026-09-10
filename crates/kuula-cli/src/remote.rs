//! A cart guest that lives in a worker process. The console, the shell
//! and the hosts step it like any
//! other [`Guest`]; each step sends the frame's input to the worker and
//! copies the frame that comes back (pixels, palette, log, profile, PCM
//! and the saves the cart made) into the draw state, so everything above
//! it is unchanged. The worker is spawned on the first step, when the
//! draw state can supply the snapshot and the save slots; a spawn or
//! load failure is the cart's fault (`sandbox_unavailable`,
//! `worker_error`), a stalled worker `watchdog_timeout`.
//!
//! In `responsive` mode (the windowed host) a step waits only a few
//! milliseconds for the frame; when it has not arrived the step returns
//! with the previous picture, silent, without sending a new input, and
//! keeps waiting on later steps until the watchdog deadline. The window
//! therefore keeps pumping events, and close, quit and restart stay
//! usable while a worker is slow or hung.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use kuula_core::{ConsoleState, DrawState, Fault, FrameInput, Guest, SharedRecorder};
use kuula_host_headless::{OwnedFrame, StepError};

use crate::broker::Worker;

/// How long a responsive step waits for the frame before letting the
/// host loop go round again.
pub const RESPONSIVE_POLL: Duration = Duration::from_millis(12);

/// The file a worker fault is reported against.
pub const FAULT_FILE: &str = "worker";

#[derive(Clone)]
pub struct RemoteConfig {
    /// The executable to run as `kuula worker`: this one.
    pub exe: PathBuf,
    /// Under the AppContainer token, or plainly with `--no-sandbox`.
    pub sandboxed: bool,
    /// Bounded waits per step (windowed) or the full watchdog (headless).
    pub responsive: bool,
    /// Where the inputs actually sent to the worker are recorded.
    pub recorder: Option<SharedRecorder>,
}

pub struct RemoteGuest {
    config: RemoteConfig,
    worker: Option<Worker>,
    /// When the step in flight was sent.
    pending: Option<Instant>,
}

fn fault(e: StepError) -> Fault {
    Fault::new(&e.code, FAULT_FILE, None, e.message)
}

impl RemoteGuest {
    pub fn new(config: RemoteConfig) -> RemoteGuest {
        RemoteGuest {
            config,
            worker: None,
            pending: None,
        }
    }

    /// A factory with the signature the console wants; the source text
    /// is ignored, the worker compiles the cart itself from the snapshot.
    pub fn factory(config: RemoteConfig) -> impl Fn(&str, &str) -> Result<Box<dyn Guest>, Fault> {
        move |_, _| Ok(Box::new(RemoteGuest::new(config.clone())) as Box<dyn Guest>)
    }

    fn spawn(&mut self, state: &mut DrawState) -> Result<(), Fault> {
        // Hold the source by its Rc, not a copy of the snapshot, so the
        // borrow of the saves below is disjoint and a large cart is
        // serialised for the worker without being cloned first.
        let cart = state.cart.clone();
        let snapshot = cart.snapshot().ok_or_else(|| {
            Fault::new(
                crate::broker::WORKER_ERROR,
                FAULT_FILE,
                None,
                "the cart source is not a snapshot",
            )
        })?;
        let saves = state.saves.all_slots();
        let mut worker = Worker::spawn(&self.config.exe, self.config.sandboxed).map_err(fault)?;
        let (w, h) = worker.load(snapshot, saves).map_err(fault)?;
        if (w, h) != (state.width(), state.height()) {
            worker.stop();
            return Err(Fault::new(
                crate::broker::WORKER_ERROR,
                FAULT_FILE,
                None,
                format!(
                    "worker reports a {w}x{h} screen, the manifest says {}x{}",
                    state.width(),
                    state.height()
                ),
            ));
        }
        self.worker = Some(worker);
        Ok(())
    }
}

/// Copy a worker's frame into the draw state.
fn apply(state: &mut DrawState, frame: &OwnedFrame, saves: Vec<(u8, Vec<u8>)>) {
    let screen = state.screen_pixels_mut();
    let n = screen.len().min(frame.pixels.len());
    screen[..n].copy_from_slice(&frame.pixels[..n]);
    state.palette.set_cart_entries(&frame.palette);
    for line in &frame.log {
        state.log_line(line.clone());
    }
    state.profile = frame.profile.clone();
    state.audio.set_external(&frame.audio);
    for (slot, bytes) in saves {
        // The slot and size were checked by the decoder; the broker's
        // store keeps its own disk failures for the host.
        let _ = state.saves.write(slot, &bytes);
    }
}

impl Guest for RemoteGuest {
    fn step(&mut self, state: &mut DrawState, input: FrameInput, _frame: u64) -> Result<(), Fault> {
        if self.worker.is_none() {
            self.spawn(state)?;
        }
        let worker = self.worker.as_mut().expect("spawned above");
        if self.pending.is_none() {
            if let Some(r) = &self.config.recorder {
                r.borrow_mut().record(input);
            }
            worker.send_step(input).map_err(fault)?;
            self.pending = Some(Instant::now());
        }
        let sent = self.pending.expect("a step is in flight");
        let wait = if self.config.responsive {
            RESPONSIVE_POLL
        } else {
            worker.step_timeout()
        };
        match worker.poll_frame(wait) {
            Ok(Some((frame, saves))) => {
                self.pending = None;
                apply(state, &frame, saves);
                match frame.state {
                    ConsoleState::Faulted(f) => Err(f),
                    ConsoleState::Running => Ok(()),
                }
            }
            Ok(None) => {
                let waited = sent.elapsed();
                if waited >= worker.step_timeout() {
                    Err(fault(worker.watchdog_expired(waited)))
                } else {
                    // Nothing new: the last picture stays, this frame is
                    // silent, and the input is not consumed.
                    state.audio.set_external(&[]);
                    Ok(())
                }
            }
            Err(e) => Err(fault(e)),
        }
    }
}

impl Drop for RemoteGuest {
    fn drop(&mut self) {
        if let Some(w) = self.worker.take() {
            w.stop();
        }
    }
}
