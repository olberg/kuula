//! `adb` as a deploy transport: a cart pushed to the Kuula app on an
//! Android device, started there, and the app's word on how it fared read
//! back from the device's log. The target is `adb` (the one device `adb`
//! sees) or `adb:<serial>`, given where a receiver's ticket goes: `kuula
//! deploy push <cart> --to adb` and the MCP `deploy` tool.
//!
//! Nothing here is networking of Kuula's own, so it is in every build. The
//! app listens to nothing: `adb push` puts `<name>.cart` in the directory
//! the app lists a person's own carts from, and `am start --es cart
//! <name>.cart` starts the app on it. That the computer may do so is
//! `adb`'s to say (the device's owner allowed debugging from it), where
//! the Iroh receiver has its approved list. The cart runs like any cart
//! there, with the saves and permissions of its installed name.
//!
//! The app says `deploy: started|faulted|not_run <name>` in its log
//! (`kuula_host_common::devlog`), which is read with `logcat`; everything
//! read from the device is untrusted text.
//!
//! Here is the deploy itself, written against [`Adb`] so that tests put a
//! script in the program's place. `program` is the adb program: finding
//! it, running it with a time limit, reading its output. `cli` is the
//! command line.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use kuula_host_common::devlog::{self, Outcome};

mod cli;
mod program;

pub use cli::push_cli;
#[cfg(not(feature = "net"))]
pub use cli::OfflineDeploy;
use program::Program;

/// The app's package, and the activity `am start` names.
pub const PACKAGE: &str = "io.github.olberg.kuula";
const ACTIVITY: &str = "io.github.olberg.kuula/android.app.NativeActivity";

/// Where the app lists a person's own carts from, as `adb` reaches it.
const CARTS: &str = "/sdcard/Android/data/io.github.olberg.kuula/files/carts";

/// The tag the app logs under.
const LOG_TAG: &str = "kuula:V";

/// The longest cart name, as for a receiver: `[a-z0-9_-]`.
const MAX_NAME: usize = 32;

/// How often, and how many times, the log is read for the app's answer:
/// 12 s in all, the app's start and half a second of the cart included.
const POLL: Duration = Duration::from_millis(300);
const POLLS: u32 = 40;

/// Lines of the app's log a report carries, the latest, and the most
/// bytes of one.
const LOG_LINES: usize = 60;
const LOG_LINE_BYTES: usize = 300;

/// The largest screenshot taken back.
const MAX_SCREENSHOT: usize = 8 << 20;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The device a deploy goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// `adb -s <serial>`; `None` is the one device `adb` sees.
    pub serial: Option<String>,
}

/// `to` as an adb target: `adb` or `adb:<serial>`. `None` when it is
/// something else (a receiver's ticket), `Err` when it starts as one and
/// the serial cannot be one.
pub fn target(to: &str) -> Option<Result<Target, String>> {
    if to == "adb" {
        return Some(Ok(Target { serial: None }));
    }
    let serial = to.strip_prefix("adb:")?;
    let plain = !serial.is_empty()
        && serial.len() <= 128
        && !serial.starts_with('-')
        && serial.chars().all(|c| c.is_ascii_graphic());
    Some(if plain {
        Ok(Target {
            serial: Some(serial.to_string()),
        })
    } else {
        Err(
            "an adb target is `adb` or `adb:<serial>`, the serial as `adb devices` prints it"
                .into(),
        )
    })
}

/// Why a deploy did not happen, with its stable code: `adb_unavailable`
/// (no `adb` program), `adb_no_device`, `adb_no_app` (Kuula is not
/// installed there), `adb_failed` (a command failed), `deploy_offer` (the
/// name), `deploy_invalid` (the cart).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub detail: String,
}

impl Failure {
    fn new(code: &'static str, detail: impl Into<String>) -> Failure {
        Failure {
            code,
            detail: detail.into(),
        }
    }
}

/// What a deploy did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub name: String,
    pub bytes: u64,
    /// SHA-256 of the package, lower-case hex.
    pub digest: String,
    /// `started`, `faulted`, `not_run` or `timeout`.
    pub restart: &'static str,
    /// The fault, or why the app gave no answer; the device's text.
    pub detail: String,
    /// The latest lines of the app's log since it started.
    pub log: Vec<String>,
    /// The device's screen as a PNG, when asked for and Kuula was in front.
    pub screenshot: Option<Vec<u8>>,
    /// Why there is no screenshot though one was asked for.
    pub no_screenshot: Option<String>,
}

/// What a run of `adb` gave.
#[derive(Debug, Clone, Default)]
pub struct Output {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Output {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// What `adb` said went wrong, without its own `error: ` in front.
    fn complaint(&self) -> String {
        let said = if self.stderr.trim().is_empty() {
            self.text()
        } else {
            self.stderr.clone()
        };
        let said = said.trim();
        said.strip_prefix("error: ")
            .or(said.strip_prefix("adb: "))
            .unwrap_or(said)
            .to_string()
    }
}

/// `adb`, as the deploy uses it: tests put a script in its place.
pub trait Adb {
    /// Run `adb <args>` against the target device.
    fn run(&mut self, args: &[&str], timeout: Duration) -> std::io::Result<Output>;

    /// Let `time` pass between two looks at the log.
    fn wait(&mut self, time: Duration);
}

/// A cart name as a receiver takes it: 1 to 32 bytes of `[a-z0-9_-]`,
/// which is also a name a device's shell reads as one word.
fn check_name(name: &str) -> Result<(), Failure> {
    let len = name.len();
    if !(1..=MAX_NAME).contains(&len) {
        return Err(Failure::new(
            "deploy_offer",
            format!("cart name of {len} bytes, must be 1 to {MAX_NAME}"),
        ));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(Failure::new(
            "deploy_offer",
            "cart name may only use a-z, 0-9, '_' and '-'",
        ));
    }
    Ok(())
}

/// One step: run it, and a failure to run at all is `adb_failed`.
fn step(adb: &mut dyn Adb, what: &str, args: &[&str], seconds: u64) -> Result<Output, Failure> {
    adb.run(args, Duration::from_secs(seconds))
        .map_err(|e| Failure::new("adb_failed", format!("{what}: {e}")))
}

/// The lines of the app's log, as `logcat -v raw` gives them.
fn log_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim_end)
        // `--------- beginning of main` and its like are logcat's own.
        .filter(|l| !l.is_empty() && !l.starts_with("---------"))
        .map(|l| {
            let mut end = l.len().min(LOG_LINE_BYTES);
            while !l.is_char_boundary(end) {
                end -= 1;
            }
            l[..end].to_string()
        })
        .collect()
}

/// Push `package` as `<name>.cart` to the device behind `adb`, start the
/// app on it and wait for the app's answer.
pub fn deploy_with(
    adb: &mut dyn Adb,
    name: &str,
    package: &[u8],
    screenshot: bool,
) -> Result<Report, Failure> {
    check_name(name)?;
    let file = format!("{name}.cart");

    let state = step(adb, "get-state", &["get-state"], 10)?;
    if !state.ok || state.text().trim() != "device" {
        let said = state.complaint();
        return Err(Failure::new(
            "adb_no_device",
            format!(
                "{}; connect a device with debugging allowed, or name one of several with adb:<serial>",
                if said.is_empty() { "no device" } else { &said }
            ),
        ));
    }
    let installed = step(adb, "pm path", &["shell", "pm", "path", PACKAGE], 10)?;
    if !installed.text().contains("package:") {
        return Err(Failure::new(
            "adb_no_app",
            format!("Kuula ({PACKAGE}) is not installed on the device"),
        ));
    }

    // `adb push` takes a file: the package goes through one of this
    // process's own, named apart from every other push the process makes,
    // so two at once do not write and remove each other's.
    static PUSHES: AtomicU32 = AtomicU32::new(0);
    let local = std::env::temp_dir().join(format!(
        "kuula-adb-{}-{}-{file}",
        std::process::id(),
        PUSHES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&local, package)
        .map_err(|e| Failure::new("adb_failed", format!("{}: {e}", local.display())))?;
    let remote = format!("{CARTS}/{file}");
    let pushed = step(
        adb,
        "push",
        &["push", &local.display().to_string(), &remote],
        60,
    );
    let _ = std::fs::remove_file(&local);
    let pushed = pushed?;
    if !pushed.ok {
        return Err(Failure::new(
            "adb_failed",
            format!("push: {}", pushed.complaint()),
        ));
    }

    // `-S` ends the app first, so it starts anew, lists the cart and has
    // a log of this run only; `-W` waits for the activity.
    let started = step(
        adb,
        "am start",
        &[
            "shell", "am", "start", "-S", "-W", "-n", ACTIVITY, "--es", "cart", &file,
        ],
        30,
    )?;
    if !started.ok || started.text().contains("Error") {
        return Err(Failure::new(
            "adb_failed",
            format!("am start: {}", started.complaint()),
        ));
    }
    let pid = step(adb, "pidof", &["shell", "pidof", PACKAGE], 10)?.text();
    let pid = pid.split_whitespace().next().unwrap_or("").to_string();
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Failure::new(
            "adb_failed",
            "the app is not running after am start",
        ));
    }

    let pid_arg = format!("--pid={pid}");
    let mut log = Vec::new();
    let mut outcome = None;
    for poll in 0..POLLS {
        if poll > 0 {
            adb.wait(POLL);
        }
        let read = step(
            adb,
            "logcat",
            &["logcat", "-d", "-v", "raw", &pid_arg, "-s", LOG_TAG],
            15,
        )?;
        log = log_lines(&read.text());
        outcome = log
            .iter()
            .filter_map(|l| devlog::parse(l))
            .find(|(named, _)| *named == file)
            .map(|(_, outcome)| outcome);
        if outcome.is_some() {
            break;
        }
    }
    let (restart, detail) = match outcome {
        Some(Outcome::Started) => ("started", String::new()),
        Some(Outcome::Faulted(why)) => ("faulted", why),
        Some(Outcome::NotRun(why)) => ("not_run", why),
        None => ("timeout", no_answer(adb)),
    };

    let (mut picture, mut no_picture) = (None, None);
    if screenshot {
        match take_screenshot(adb) {
            Ok(png) => picture = Some(png),
            Err(why) => no_picture = Some(why),
        }
    }
    if log.len() > LOG_LINES {
        log.drain(..log.len() - LOG_LINES);
    }
    Ok(Report {
        name: name.to_string(),
        bytes: package.len() as u64,
        digest: kuula_core::zipsource::digest_hex(package),
        restart,
        detail,
        log,
        screenshot: picture,
        no_screenshot: no_picture,
    })
}

/// Why the app said nothing: it steps no frame while the device sleeps
/// or is locked, which is the usual reason.
fn no_answer(adb: &mut dyn Adb) -> String {
    let seconds = POLL.as_millis() as u64 * POLLS as u64 / 1000;
    let power = step(adb, "dumpsys power", &["shell", "dumpsys", "power"], 10)
        .map(|out| out.text())
        .unwrap_or_default();
    let wakefulness = power
        .lines()
        .find_map(|l| l.trim().strip_prefix("mWakefulness="))
        .unwrap_or("");
    match wakefulness {
        "" | "Awake" => format!(
            "the app did not say how the cart fared in {seconds} s; is the device unlocked with Kuula in front?"
        ),
        other => format!(
            "the app did not say how the cart fared in {seconds} s; the device is {}: wake and unlock it",
            other.to_lowercase()
        ),
    }
}

/// The device's screen, only while Kuula's window has the focus: a lock
/// screen or another app is not ours to take a picture of.
fn take_screenshot(adb: &mut dyn Adb) -> Result<Vec<u8>, String> {
    let windows = step(adb, "dumpsys window", &["shell", "dumpsys", "window"], 10)
        .map_err(|f| f.detail)?
        .text();
    let in_front = windows
        .lines()
        .any(|l| l.contains("mCurrentFocus") && l.contains(PACKAGE));
    if !in_front {
        return Err("Kuula is not in front on the device".into());
    }
    let shot =
        step(adb, "screencap", &["exec-out", "screencap", "-p"], 20).map_err(|f| f.detail)?;
    if !shot.ok || !shot.stdout.starts_with(PNG_MAGIC) {
        return Err("screencap gave no picture".into());
    }
    if shot.stdout.len() > MAX_SCREENSHOT {
        return Err(format!(
            "the picture is {} bytes, more than {MAX_SCREENSHOT}",
            shot.stdout.len()
        ));
    }
    Ok(shot.stdout)
}

/// Deploy through the `adb` program found on this computer.
pub fn deploy(
    target: &Target,
    name: &str,
    package: &[u8],
    screenshot: bool,
) -> Result<Report, Failure> {
    check_name(name)?;
    let mut adb = Program::find(target)?;
    deploy_with(&mut adb, name, package, screenshot)
}

/// The MCP `deploy` tool's push to an adb target: the cart as the other
/// tools read it, packed as `kuula build` packs it.
pub fn mcp_push(
    target: Result<Target, String>,
    request: kuula_mcp::DeployRequest,
) -> Result<kuula_mcp::DeployOutcome, kuula_mcp::ToolError> {
    use kuula_mcp::ToolError;
    let target = target.map_err(|why| ToolError::new("invalid_arguments", why))?;
    if request
        .snapshot
        .get(kuula_core::console::MAIN_FILE)
        .is_none()
    {
        return Err(ToolError::new(
            "deploy_invalid",
            "not_found main.lua: a cart needs a main.lua",
        ));
    }
    let package = kuula_core::zipsource::pack(&request.snapshot).map_err(|e| {
        let at = if e.path.is_empty() {
            String::new()
        } else {
            format!(" {}", e.path)
        };
        ToolError::new("deploy_invalid", format!("{}{at}: {}", e.code, e.message))
    })?;
    let report = deploy(&target, &request.name, &package, request.screenshot)
        .map_err(|f| ToolError::new(f.code, f.detail))?;
    let mut detail = report.detail;
    if let Some(why) = &report.no_screenshot {
        if !detail.is_empty() {
            detail.push_str("; ");
        }
        detail.push_str(&format!("no screenshot: {why}"));
    }
    Ok(kuula_mcp::DeployOutcome {
        name: report.name,
        bytes: report.bytes,
        digest: report.digest,
        code: "deploy_ok".into(),
        detail,
        transfer: "ok".into(),
        validation: "ok".into(),
        install: "ok".into(),
        restart: report.restart.into(),
        log: report.log,
        screenshot: report.screenshot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted `adb`: what each command answers, and what was asked.
    struct Script {
        asked: Vec<Vec<String>>,
        state: Output,
        installed: bool,
        /// What the log holds at each look; the last stays.
        logs: Vec<&'static str>,
        looks: usize,
        power: &'static str,
        focus: &'static str,
        waited: Duration,
    }

    fn said(text: &str) -> Output {
        Output {
            ok: true,
            stdout: text.as_bytes().to_vec(),
            stderr: String::new(),
        }
    }

    impl Script {
        fn new(logs: &[&'static str]) -> Script {
            Script {
                asked: Vec::new(),
                state: said("device\n"),
                installed: true,
                logs: logs.to_vec(),
                looks: 0,
                power: "  mWakefulness=Awake\n",
                focus: "  mCurrentFocus=Window{3ae u0 io.github.olberg.kuula/android.app.NativeActivity}\n",
                waited: Duration::ZERO,
            }
        }

        fn commands(&self) -> Vec<String> {
            self.asked.iter().map(|a| a.join(" ")).collect()
        }
    }

    impl Adb for Script {
        fn run(&mut self, args: &[&str], _: Duration) -> std::io::Result<Output> {
            self.asked
                .push(args.iter().map(|a| a.to_string()).collect());
            Ok(match args {
                ["get-state"] => self.state.clone(),
                ["shell", "pm", "path", _] if self.installed => {
                    said("package:/data/app/x/base.apk\n")
                }
                ["shell", "pm", "path", _] => said(""),
                ["push", local, _] => {
                    // The package is in the file while it is pushed.
                    assert!(std::fs::read(local).is_ok_and(|b| b == b"PK-package"));
                    said("1 file pushed\n")
                }
                ["shell", "am", "start", ..] => said("Starting: Intent\nStatus: ok\n"),
                ["shell", "pidof", _] => said("4242\n"),
                ["logcat", ..] => {
                    let at = self.looks.min(self.logs.len().saturating_sub(1));
                    self.looks += 1;
                    said(self.logs.get(at).copied().unwrap_or(""))
                }
                ["shell", "dumpsys", "power"] => said(self.power),
                ["shell", "dumpsys", "window"] => said(self.focus),
                ["exec-out", "screencap", "-p"] => Output {
                    ok: true,
                    stdout: [PNG_MAGIC, b"pixels"].concat(),
                    stderr: String::new(),
                },
                other => panic!("an adb command the deploy has no business running: {other:?}"),
            })
        }

        fn wait(&mut self, time: Duration) {
            self.waited += time;
        }
    }

    const BOOT: &str =
        "--------- beginning of main\nkuula_android: kuula 0.0.3 starting\nkuula_android::app: started on hello.cart\n";

    #[test]
    fn a_target_is_adb_or_adb_and_a_serial() {
        assert_eq!(target("adb"), Some(Ok(Target { serial: None })));
        assert_eq!(
            target("adb:emulator-5554"),
            Some(Ok(Target {
                serial: Some("emulator-5554".into())
            }))
        );
        assert_eq!(
            target("adb:192.0.2.5:5555").unwrap().unwrap().serial,
            Some("192.0.2.5:5555".into())
        );
        // A ticket is not one, and neither is a word that starts alike.
        assert_eq!(target("endpointabc"), None);
        assert_eq!(target("adbx"), None);
        assert_eq!(target(""), None);
        for bad in ["adb:", "adb:-s", "adb:two words", "adb:a\nb"] {
            assert!(matches!(target(bad), Some(Err(_))), "{bad:?}");
        }
    }

    #[test]
    fn a_cart_is_pushed_started_and_reported_started() {
        let later = format!(
            "{BOOT}kuula_android::app: cart: hi\nkuula_android::app: deploy: started hello.cart\n"
        );
        let later: &'static str = later.leak();
        let mut adb = Script::new(&[BOOT, later]);
        let report = deploy_with(&mut adb, "hello", b"PK-package", false).unwrap();
        assert_eq!(report.name, "hello");
        assert_eq!(report.bytes, 10);
        assert_eq!(report.digest.len(), 64);
        assert_eq!(report.restart, "started");
        assert_eq!(report.detail, "");
        assert_eq!(report.screenshot, None);
        // The app's log without logcat's own line, the cart's print in it.
        assert_eq!(report.log.len(), 4, "{:?}", report.log);
        assert_eq!(report.log[2], "kuula_android::app: cart: hi");
        // One wait: the answer was there at the second look.
        assert_eq!(adb.waited, POLL);
        let commands = adb.commands();
        assert!(commands[2].starts_with("push "), "{commands:?}");
        assert!(
            commands[2]
                .ends_with(" /sdcard/Android/data/io.github.olberg.kuula/files/carts/hello.cart"),
            "{commands:?}"
        );
        assert_eq!(
            commands[3],
            "shell am start -S -W -n io.github.olberg.kuula/android.app.NativeActivity --es cart hello.cart"
        );
        assert_eq!(commands[5], "logcat -d -v raw --pid=4242 -s kuula:V");
        // The local copy, which the push named, is gone again.
        let local = std::path::Path::new(&adb.asked[2][1]);
        assert!(!local.exists());
        assert!(
            adb.asked[2][1].ends_with("-hello.cart"),
            "{:?}",
            adb.asked[2]
        );
    }

    #[test]
    fn a_fault_is_the_answer_and_its_text_the_detail() {
        let log: &'static str = format!(
            "{BOOT}kuula_android::app: deploy: faulted hello.cart: runtime_error main.lua:3: boom\n"
        )
        .leak();
        let mut adb = Script::new(&[log]);
        let report = deploy_with(&mut adb, "hello", b"PK-package", false).unwrap();
        assert_eq!(report.restart, "faulted");
        assert_eq!(report.detail, "runtime_error main.lua:3: boom");
        assert_eq!(adb.waited, Duration::ZERO);

        let log: &'static str =
            format!("{BOOT}kuula_android::app: deploy: not_run hello.cart: cart_read_error hello.cart: no such cart\n")
                .leak();
        let report = deploy_with(&mut Script::new(&[log]), "hello", b"PK-package", false).unwrap();
        assert_eq!(report.restart, "not_run");
        assert_eq!(report.detail, "cart_read_error hello.cart: no such cart");
    }

    #[test]
    fn no_answer_is_a_timeout_that_says_whether_the_device_sleeps() {
        // What the cart prints, and the answer about another cart, are not
        // this cart's answer.
        let log: &'static str = format!(
            "{BOOT}kuula_android::app: cart: deploy: started hello.cart\nkuula_android::app: deploy: started other.cart\n"
        )
        .leak();
        let mut adb = Script::new(&[log]);
        adb.power = "  mWakefulness=Asleep\n";
        let report = deploy_with(&mut adb, "hello", b"PK-package", true).unwrap();
        assert_eq!(report.restart, "timeout");
        assert!(report.detail.contains("12 s"), "{}", report.detail);
        assert!(report.detail.contains("asleep"), "{}", report.detail);
        assert_eq!(adb.waited, POLL * (POLLS - 1));

        let mut adb = Script::new(&[BOOT]);
        let report = deploy_with(&mut adb, "hello", b"PK-package", false).unwrap();
        assert!(report.detail.contains("unlocked"), "{}", report.detail);
    }

    #[test]
    fn a_screenshot_is_taken_only_with_kuula_in_front() {
        let log: &'static str =
            format!("{BOOT}kuula_android::app: deploy: started hello.cart\n").leak();
        let mut adb = Script::new(&[log]);
        let report = deploy_with(&mut adb, "hello", b"PK-package", true).unwrap();
        assert!(report
            .screenshot
            .is_some_and(|png| png.starts_with(PNG_MAGIC)));
        assert_eq!(report.detail, "");
        assert_eq!(report.no_screenshot, None);

        let mut adb = Script::new(&[log]);
        adb.focus = "  mCurrentFocus=Window{1 u0 NotificationShade}\n";
        let report = deploy_with(&mut adb, "hello", b"PK-package", true).unwrap();
        assert_eq!(report.screenshot, None);
        assert_eq!(report.restart, "started");
        assert_eq!(report.detail, "");
        assert_eq!(
            report.no_screenshot.as_deref(),
            Some("Kuula is not in front on the device")
        );
        assert!(!adb.commands().iter().any(|c| c.contains("screencap")));
    }

    #[test]
    fn no_device_no_app_and_a_bad_name_are_failures_with_their_codes() {
        let mut adb = Script::new(&[BOOT]);
        adb.state = Output {
            ok: false,
            stdout: Vec::new(),
            stderr: "error: no devices/emulators found\n".into(),
        };
        let failure = deploy_with(&mut adb, "hello", b"PK-package", false).unwrap_err();
        assert_eq!(failure.code, "adb_no_device");
        assert!(failure.detail.starts_with("no devices/emulators found;"));
        assert_eq!(adb.commands(), ["get-state"]);

        let mut adb = Script::new(&[BOOT]);
        adb.state = said("unauthorized\n");
        let failure = deploy_with(&mut adb, "hello", b"PK-package", false).unwrap_err();
        assert_eq!(failure.code, "adb_no_device");
        assert!(
            failure.detail.starts_with("unauthorized;"),
            "{}",
            failure.detail
        );

        let mut adb = Script::new(&[BOOT]);
        adb.installed = false;
        let failure = deploy_with(&mut adb, "hello", b"PK-package", false).unwrap_err();
        assert_eq!(failure.code, "adb_no_app");
        assert_eq!(adb.asked.len(), 2);

        for name in ["", "Hello", "two words", "a;b", "../x", &"a".repeat(33)] {
            let mut adb = Script::new(&[BOOT]);
            let failure = deploy_with(&mut adb, name, b"PK-package", false).unwrap_err();
            assert_eq!(failure.code, "deploy_offer", "{name:?}");
            assert!(adb.asked.is_empty(), "nothing is run for {name:?}");
        }
    }

    #[test]
    fn log_lines_are_bounded_and_whole_characters() {
        let long = "\u{e4}".repeat(400);
        let lines = log_lines(&format!("--------- beginning of main\n\n{long}\nshort\r\n"));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), LOG_LINE_BYTES);
        assert_eq!(lines[1], "short");
    }
}
