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

use kuula_core::net::{Event, NetEnv, MAX_BATCH};
use kuula_core::{ConsoleState, DrawState, Fault, FrameInput, Guest, SharedRecorder};
use kuula_host_headless::StepError;

use crate::broker::{Reply, Worker};

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
    /// The events the step in flight carried, for the recorder.
    sent_events: Vec<Event>,
    /// Control events (a permission change) the console's mirror
    /// admitted on a tick the worker did not see, because a step was
    /// in flight; they go first on the next step. Room is zero while a
    /// step is in flight, so nothing else can arrive then. At most
    /// [`MAX_BATCH`], the newest kept.
    carried: Vec<Event>,
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
            sent_events: Vec::new(),
            carried: Vec::new(),
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
        // The worker's cart gets the permission and invite this one has;
        // a cart without the service gets none.
        let net = match &state.net {
            Some(n) => NetEnv {
                permitted: n.permitted,
                invite: n.invite.clone(),
            },
            None => NetEnv::default(),
        };
        let mut worker = Worker::spawn(&self.config.exe, self.config.sandboxed).map_err(fault)?;
        let (w, h) = worker.load(snapshot, saves, net).map_err(fault)?;
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
fn apply(state: &mut DrawState, reply: Reply) {
    let Reply {
        frame,
        saves,
        commands,
        net_room,
    } = reply;
    let frame = &frame;
    if let Some(net) = state.net.as_mut() {
        // The worker's console is the real one: its commands become
        // this frame's outbox and its room bounds the next step. The
        // mirror's own inbox is never read by a cart, so it is emptied
        // here; left to fill, it would make the mirror drop events the
        // worker had room for.
        net.outbox = commands;
        net.remote_room = Some(net_room);
        net.inbox.clear();
    }
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
            let events = match state.net.as_mut() {
                Some(net) => {
                    // No room while a step is in flight: nothing offered
                    // then is lost when the console's mirror sees a
                    // tick the worker does not.
                    net.remote_room = Some(0);
                    let mut events = std::mem::take(&mut self.carried);
                    events.extend(net.admitted.iter().cloned());
                    // The carried events and this tick's batch are each
                    // within the bound, but not together: the worker
                    // takes one batch, so the tail rides the next step
                    // and the recording stays what the cart saw.
                    if events.len() > MAX_BATCH {
                        self.carried = events.split_off(MAX_BATCH);
                    }
                    events
                }
                None => Vec::new(),
            };
            if let Some(r) = &self.config.recorder {
                let mut r = r.borrow_mut();
                if let Some(net) = &state.net {
                    r.enable_net(&NetEnv {
                        permitted: net.permitted,
                        invite: net.invite.clone(),
                    });
                }
                r.record(input);
            }
            worker.send_step(input, events.clone()).map_err(fault)?;
            self.sent_events = events;
            self.pending = Some(Instant::now());
        } else if let Some(net) = state.net.as_mut() {
            // A tick the worker does not see: what the mirror admitted
            // (control events only, the room being zero) would be
            // cleared by the next tick, so keep it for the next step.
            self.carried.append(&mut net.admitted);
            if self.carried.len() > MAX_BATCH {
                let excess = self.carried.len() - MAX_BATCH;
                self.carried.drain(..excess);
            }
        }
        let sent = self.pending.expect("a step is in flight");
        let wait = if self.config.responsive {
            RESPONSIVE_POLL
        } else {
            worker.step_timeout()
        };
        match worker.poll_frame(wait) {
            Ok(Some(reply)) => {
                self.pending = None;
                let outcome = reply.frame.state.clone();
                apply(state, reply);
                if let Some(net) = state.net.as_mut() {
                    // The carried events ride the next step, within the
                    // same batch bound, so the room offered shrinks by
                    // as many.
                    if let Some(r) = net.remote_room {
                        net.remote_room = Some(r.saturating_sub(self.carried.len()));
                    }
                }
                let outcome = match outcome {
                    ConsoleState::Faulted(f) => Err(f),
                    ConsoleState::Running => Ok(()),
                };
                if let (Some(r), Some(net)) = (&self.config.recorder, &state.net) {
                    // A faulting frame's commands are discarded, as in
                    // process.
                    let none = Vec::new();
                    let commands = if outcome.is_ok() { &net.outbox } else { &none };
                    r.borrow_mut().record_net(&self.sent_events, commands);
                }
                outcome
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
