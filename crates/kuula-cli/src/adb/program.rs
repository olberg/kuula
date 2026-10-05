//! The adb program: where it is looked for, how it is run with a time
//! limit, and how its output is read without waiting on whoever else holds
//! the pipe.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{Adb, Failure, Output, Target};

/// How long the rest of a finished command's output is waited for.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// The `adb` program.
pub struct Program {
    path: PathBuf,
    serial: Option<String>,
}

/// Where `adb` may be: `KUULA_ADB` when set and nothing else, otherwise
/// the search path and then the Android SDK's usual places.
fn candidates() -> Vec<PathBuf> {
    if let Some(given) = std::env::var_os("KUULA_ADB") {
        return vec![PathBuf::from(given)];
    }
    let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
    let mut all = vec![PathBuf::from("adb")];
    let mut sdk = |root: Option<std::ffi::OsString>, below: &[&str]| {
        if let Some(root) = root {
            let mut path = PathBuf::from(root);
            path.extend(below);
            all.push(path.join("platform-tools").join(exe));
        }
    };
    sdk(std::env::var_os("ANDROID_HOME"), &[]);
    sdk(std::env::var_os("ANDROID_SDK_ROOT"), &[]);
    sdk(std::env::var_os("LOCALAPPDATA"), &["Android", "Sdk"]);
    sdk(std::env::var_os("HOME"), &["Android", "Sdk"]);
    sdk(std::env::var_os("HOME"), &["Library", "Android", "sdk"]);
    all
}

impl Program {
    /// Find `adb` and aim it at `target`.
    pub fn find(target: &Target) -> Result<Program, Failure> {
        let tried = candidates();
        for path in &tried {
            let mut command = Command::new(path);
            command.arg("version");
            if run_command(command, Duration::from_secs(10)).is_ok_and(|out| out.ok) {
                start_server(path);
                return Ok(Program {
                    path: path.clone(),
                    serial: target.serial.clone(),
                });
            }
        }
        let tried: Vec<String> = tried.iter().map(|p| p.display().to_string()).collect();
        Err(Failure::new(
            "adb_unavailable",
            format!(
                "no adb program (tried {}); install the Android platform tools or set KUULA_ADB to adb's path",
                tried.join(", ")
            ),
        ))
    }
}

impl Adb for Program {
    fn run(&mut self, args: &[&str], timeout: Duration) -> std::io::Result<Output> {
        let mut command = Command::new(&self.path);
        if let Some(serial) = &self.serial {
            command.args(["-s", serial]);
        }
        command.args(args);
        run_command(command, timeout)
    }

    fn wait(&mut self, time: Duration) {
        std::thread::sleep(time);
    }
}

/// No console window of its own for a child of a server that has none.
fn no_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Wait for `child` to end, and kill it at `timeout`: `None` then.
fn wait_or_kill(child: &mut Child, timeout: Duration) -> std::io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Start adb's server before any command is run under a pipe. The server
/// is a daemon that the first adb command forks when none is running, and
/// it keeps the output that command was given: under a pipe it would hold
/// the pipe open for as long as it lives. Here it is given nothing to
/// hold. A failure is left to the commands that follow to report.
fn start_server(adb: &Path) {
    let mut command = Command::new(adb);
    command
        .arg("start-server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    no_window(&mut command);
    if let Ok(mut child) = command.spawn() {
        let _ = wait_or_kill(&mut child, Duration::from_secs(20));
    }
}

/// What a pipe has given so far, and whether it has ended.
#[derive(Default)]
struct Drained {
    bytes: Vec<u8>,
    ended: bool,
}

/// Read `pipe` on a thread of its own into what is returned, as it comes:
/// a screenshot's worth does not fill the pipe and stall the command.
fn drain(pipe: impl Read + Send + 'static) -> Arc<Mutex<Drained>> {
    let shared = Arc::new(Mutex::new(Drained::default()));
    let into = shared.clone();
    std::thread::spawn(move || {
        let mut pipe = pipe;
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let n = pipe.read(&mut chunk).unwrap_or(0);
            let mut drained = into.lock().unwrap_or_else(|e| e.into_inner());
            if n == 0 {
                drained.ended = true;
                break;
            }
            drained.bytes.extend_from_slice(&chunk[..n]);
        }
    });
    shared
}

/// Run `command` to its end, or kill it at `timeout`: `adb` waits for
/// ever on a device that has gone quiet. The time limit holds for reading
/// its output too. A pipe ends only when everything that holds it has let
/// go, and a process the command left behind (adb's server, started under
/// this very pipe by something else's doing) may never: once the command
/// itself has ended, what it wrote is waited for a moment and no longer,
/// and what has come by then is the output.
fn run_command(mut command: Command, timeout: Duration) -> std::io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut command);
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    let status = wait_or_kill(&mut child, timeout)?;

    let ended = |pipe: &Option<Arc<Mutex<Drained>>>| {
        pipe.as_ref()
            .is_none_or(|p| p.lock().unwrap_or_else(|e| e.into_inner()).ended)
    };
    let grace = Instant::now() + DRAIN_GRACE;
    while !(ended(&stdout) && ended(&stderr)) && Instant::now() < grace {
        std::thread::sleep(Duration::from_millis(5));
    }
    let taken = |pipe: Option<Arc<Mutex<Drained>>>| {
        pipe.map(|p| std::mem::take(&mut p.lock().unwrap_or_else(|e| e.into_inner()).bytes))
            .unwrap_or_default()
    };
    let (stdout, stderr) = (taken(stdout), taken(stderr));
    match status {
        Some(status) => Ok(Output {
            ok: status.success(),
            stdout,
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        }),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("no answer in {} s", timeout.as_secs()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_run_to_its_end_with_what_it_wrote() {
        // This test binary lists its tests when asked: a program that is
        // here on every machine and writes more than a line.
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--list", "--format", "terse"]);
        let out = run_command(command, Duration::from_secs(60)).unwrap();
        assert!(out.ok);
        assert!(
            out.text()
                .contains("a_command_is_run_to_its_end_with_what_it_wrote"),
            "{}",
            out.text()
        );
        // A program that is not there is an error, not a hang.
        let missing = Command::new("kuula-no-such-program-anywhere");
        assert!(run_command(missing, Duration::from_secs(5)).is_err());
    }
}
