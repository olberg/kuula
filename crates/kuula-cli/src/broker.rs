//! The broker side of the worker split: spawn the
//! same executable as `kuula worker` under an AppContainer token with no
//! capabilities (`sandbox.rs`), created suspended and put in a Job Object
//! that dies with the broker and cannot fork or grow without bound
//! before it runs, hand it the cart snapshot over a pipe, and treat
//! everything it sends back as hostile input. If the token or the job
//! cannot be set up the run ends with `sandbox_unavailable`;
//! `--no-sandbox` is the explicit, warned, plain launch for debugging.
//!
//! Replies are read on a thread so every wait has a deadline: a worker
//! that stops answering is killed and the run ends with
//! `watchdog_timeout`, which is the wall-clock backstop
//! asks for, distinct from the deterministic `budget_exceeded` the meter
//! produces.

use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use kuula_core::{Fault, FrameInput, Snapshot};
use kuula_host_headless::{OwnedFrame, StepError, Stepper};

use crate::ipc::{self, Message, Saves};
use crate::sandbox;

/// Memory the worker process may commit, enforced by the job.
pub const WORKER_MEMORY_LIMIT: usize = 512 * 1024 * 1024;

pub const WORKER_ERROR: &str = "worker_error";

/// Wall time one step may take before the worker is killed. The meter
/// ends a runaway frame in a few milliseconds; this only catches native
/// code the meter cannot see.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(10);
/// Loading compiles and decodes the whole cart.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `stop` waits for a clean exit before killing.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(2);

/// Test hook: overrides both timeouts, in milliseconds, so the watchdog
/// path can be tested without waiting ten seconds.
pub const WATCHDOG_HOOK: &str = "KUULA_TEST_WATCHDOG_MS";

/// Line printed when `--no-sandbox` skips the token.
pub const NO_SANDBOX_WARNING: &str =
    "warning: --no-sandbox: the worker runs without an AppContainer token";

/// The worker process: under the AppContainer launcher by default, a
/// plain `std::process::Child` with `--no-sandbox`.
enum Process {
    Sandboxed(sandbox::Child),
    Plain(std::process::Child),
}

impl Process {
    fn kill(&mut self) {
        match self {
            Process::Sandboxed(c) => {
                let _ = c.kill();
                let _ = c.wait();
            }
            Process::Plain(c) => {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    /// Whether the process has exited; `Err` when that cannot be told.
    fn try_wait(&mut self) -> Result<bool, String> {
        match self {
            Process::Sandboxed(c) => c.try_wait().map(|c| c.is_some()),
            Process::Plain(c) => c.try_wait().map(|s| s.is_some()).map_err(|e| e.to_string()),
        }
    }
}

/// What the job is attached to: either kind of child, borrowed.
pub enum JobTarget<'a> {
    Sandboxed(&'a sandbox::Child),
    Plain(&'a std::process::Child),
}

impl JobTarget<'_> {
    #[cfg(windows)]
    fn raw_handle(&self) -> std::os::windows::io::RawHandle {
        use std::os::windows::io::AsRawHandle;
        match self {
            JobTarget::Sandboxed(c) => c.raw_handle(),
            JobTarget::Plain(c) => c.as_raw_handle(),
        }
    }
}

pub struct Worker {
    child: Process,
    /// `None` once `stop` has closed the pipe.
    stdin: Option<BufWriter<Box<dyn Write>>>,
    /// Replies from the reader thread; `Err` is the reason it stopped.
    replies: Receiver<Result<Message, String>>,
    #[allow(dead_code)]
    job: Option<job::Job>,
    dead: bool,
    step_timeout: Duration,
    load_timeout: Duration,
}

fn worker_error(message: impl Into<String>) -> StepError {
    StepError {
        code: WORKER_ERROR.to_string(),
        message: message.into(),
    }
}

impl Worker {
    /// Spawn `exe worker` with a minimal environment and piped stdio.
    /// Stderr is inherited so the worker's own diagnostics reach the
    /// terminal. With `sandboxed` the child runs under the AppContainer
    /// token and joins the job before its first instruction; any setup
    /// failure kills it and refuses the run with `sandbox_unavailable`.
    /// Without it (`--no-sandbox`) the job is still attached, but after
    /// the child has started, and a warning line is printed.
    pub fn spawn(exe: &Path, sandboxed: bool) -> Result<Worker, StepError> {
        let env = sandbox::minimal_env();
        let (child, stdin, stdout, job): (Process, Box<dyn Write>, Box<dyn Read + Send>, _) =
            if sandboxed {
                let mut job = None;
                let spawned = sandbox::spawn(exe, &["worker"], &env, |child| {
                    job = Some(job::Job::new_and_assign(JobTarget::Sandboxed(child))?);
                    Ok(())
                })
                .map_err(|e| StepError {
                    code: Fault::SANDBOX_UNAVAILABLE.to_string(),
                    message: e,
                })?;
                (
                    Process::Sandboxed(spawned.child),
                    Box::new(spawned.stdin),
                    Box::new(spawned.stdout),
                    job,
                )
            } else {
                eprintln!("{NO_SANDBOX_WARNING}");
                let mut cmd = Command::new(exe);
                cmd.arg("worker")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .env_clear()
                    .envs(env);
                let mut child = cmd.spawn().map_err(|e| {
                    worker_error(format!("could not start worker {}: {e}", exe.display()))
                })?;
                let stdin = child.stdin.take().expect("piped");
                let stdout = child.stdout.take().expect("piped");
                let job = match job::Job::new_and_assign(JobTarget::Plain(&child)) {
                    Ok(j) => Some(j),
                    Err(e) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(worker_error(format!("could not confine worker: {e}")));
                    }
                };
                (
                    Process::Plain(child),
                    Box::new(stdin),
                    Box::new(stdout),
                    job,
                )
            };
        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            loop {
                let item = match ipc::read_frame(&mut reader) {
                    Ok(Some(msg)) => Ok(msg),
                    Ok(None) => Err("worker closed the pipe".to_string()),
                    Err(e) => Err(format!("worker reply refused: {e}")),
                };
                let stop = item.is_err();
                if tx.send(item).is_err() || stop {
                    break;
                }
            }
        });
        let (step_timeout, load_timeout) = match std::env::var(WATCHDOG_HOOK)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            Some(ms) => (Duration::from_millis(ms), Duration::from_millis(ms)),
            None => (STEP_TIMEOUT, LOAD_TIMEOUT),
        };
        Ok(Worker {
            child,
            stdin: Some(BufWriter::new(stdin)),
            replies,
            job,
            dead: false,
            step_timeout,
            load_timeout,
        })
    }

    fn send(&mut self, msg: &Message) -> Result<(), StepError> {
        if self.dead {
            return Err(worker_error("worker is gone"));
        }
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(worker_error("worker has been stopped"));
        };
        ipc::write_frame(stdin, msg).map_err(|e| {
            self.dead = true;
            worker_error(format!("could not write to worker: {e}"))
        })
    }

    fn kill(&mut self) {
        self.dead = true;
        self.child.kill();
    }

    /// Wait for one reply within `timeout`. Anything but a well-formed
    /// message in time kills the worker.
    fn receive(&mut self, timeout: Duration) -> Result<Message, StepError> {
        match self.replies.recv_timeout(timeout) {
            Ok(Ok(msg)) => Ok(msg),
            Ok(Err(reason)) => {
                self.kill();
                Err(worker_error(reason))
            }
            Err(RecvTimeoutError::Timeout) => {
                self.kill();
                Err(StepError {
                    code: Fault::WATCHDOG_TIMEOUT.to_string(),
                    message: format!("worker did not reply within {} ms", timeout.as_millis()),
                })
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.kill();
                Err(worker_error("worker closed the pipe"))
            }
        }
    }

    /// Send the cart and its save slots and wait for the screen size.
    pub fn load(&mut self, snapshot: &Snapshot, saves: Saves) -> Result<(u32, u32), StepError> {
        self.send(&Message::Load {
            snapshot: snapshot.clone(),
            saves,
        })?;
        match self.receive(self.load_timeout)? {
            Message::Ready { width, height } => Ok((width, height)),
            Message::Error { code, message } => Err(StepError { code, message }),
            other => {
                self.kill();
                Err(worker_error(format!(
                    "unexpected reply {}",
                    tag_name(&other)
                )))
            }
        }
    }

    /// The wall-clock bound on one step.
    pub fn step_timeout(&self) -> Duration {
        self.step_timeout
    }

    /// Send one frame's input; the reply is collected by `poll_frame`.
    pub fn send_step(&mut self, input: FrameInput) -> Result<(), StepError> {
        self.send(&Message::Step(input))
    }

    /// Wait up to `timeout` for the frame of the step in flight.
    /// `Ok(None)` means it has not arrived yet, which is not a failure:
    /// the caller decides when the watchdog is up (`watchdog_expired`).
    /// A malformed or unexpected reply kills the worker.
    pub fn poll_frame(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<(OwnedFrame, Saves)>, StepError> {
        if self.dead {
            return Err(worker_error("worker is gone"));
        }
        match self.replies.recv_timeout(timeout) {
            Ok(Ok(Message::Frame { frame, saves })) => Ok(Some((frame, saves))),
            Ok(Ok(Message::Error { code, message })) => Err(StepError { code, message }),
            Ok(Ok(other)) => {
                self.kill();
                Err(worker_error(format!(
                    "unexpected reply {}",
                    tag_name(&other)
                )))
            }
            Ok(Err(reason)) => {
                self.kill();
                Err(worker_error(reason))
            }
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                self.kill();
                Err(worker_error("worker closed the pipe"))
            }
        }
    }

    /// The caller waited `waited` in all for a frame that never came:
    /// kill the worker and name the fault.
    pub fn watchdog_expired(&mut self, waited: Duration) -> StepError {
        self.kill();
        StepError {
            code: Fault::WATCHDOG_TIMEOUT.to_string(),
            message: format!("worker did not reply within {} ms", waited.as_millis()),
        }
    }

    /// Ask the worker to exit, close its pipe and wait a bounded time
    /// for it; kill it if it lingers.
    pub fn stop(mut self) {
        if !self.dead {
            let _ = self.send(&Message::Stop);
        }
        drop(self.stdin.take());
        let deadline = Instant::now() + STOP_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(true) => break,
                Ok(false) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    self.kill();
                    break;
                }
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.child.kill();
    }
}

impl Stepper for Worker {
    /// One synchronous step under the full watchdog; the saves the frame
    /// carried are dropped, since a bare stepper has no store.
    fn step(&mut self, input: FrameInput) -> Result<OwnedFrame, StepError> {
        self.send_step(input)?;
        let timeout = self.step_timeout;
        match self.poll_frame(timeout)? {
            Some((frame, _saves)) => Ok(frame),
            None => Err(self.watchdog_expired(timeout)),
        }
    }
}

fn tag_name(m: &Message) -> &'static str {
    match m {
        Message::Load { .. } => "Load",
        Message::Step(_) => "Step",
        Message::Stop => "Stop",
        Message::Ready { .. } => "Ready",
        Message::Frame { .. } => "Frame",
        Message::Error { .. } => "Error",
    }
}

#[cfg(windows)]
mod job {
    //! A Job Object with kill-on-close, a single-process limit and a
    //! memory cap. Closing the handle (dropping `Job`) kills the worker.

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };

    pub struct Job(HANDLE);

    impl Job {
        pub fn new_and_assign(process: super::JobTarget<'_>) -> Result<Job, String> {
            // SAFETY: plain Win32 calls with valid arguments; the handle
            // is owned by `Job` and closed exactly once in `Drop`.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Err("CreateJobObject failed".into());
                }
                let job = Job(job);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                    | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                    | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                    | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
                info.BasicLimitInformation.ActiveProcessLimit = 1;
                info.ProcessMemoryLimit = super::WORKER_MEMORY_LIMIT;
                let ok = SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    return Err("SetInformationJobObject failed".into());
                }
                if AssignProcessToJobObject(job.0, process.raw_handle() as HANDLE) == 0 {
                    return Err("AssignProcessToJobObject failed".into());
                }
                Ok(job)
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateJobObjectW and is closed once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
mod job {
    /// No job objects off Windows; the worker is still a separate process.
    pub struct Job;

    impl Job {
        pub fn new_and_assign(_: super::JobTarget<'_>) -> Result<Job, String> {
            Ok(Job)
        }
    }
}
