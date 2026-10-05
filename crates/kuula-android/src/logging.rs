//! Logging to Android's log under the tag "kuula": `adb logcat -s kuula`.
//! A cart's `print` lines and faults are logged by the loop; what a library
//! prints to stdout or stderr (the audio report, a panic's message) is
//! carried over by a pipe, so none of it goes where nobody can read it.

use std::io::{BufRead, BufReader};
use std::os::fd::FromRawFd;

use log::LevelFilter;

const TAG: &str = "kuula";

pub fn init() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(LevelFilter::Info)
            .with_tag(TAG),
    );
    std::panic::set_hook(Box::new(|info| {
        log::error!("panic: {info}");
    }));
    redirect_stdio();
}

/// Point file descriptors 1 and 2 at a pipe and log what comes out of it,
/// a line at a time. Failing to is not fatal: the log lacks those lines.
fn redirect_stdio() {
    let mut fds = [0i32; 2];
    // SAFETY: plain libc calls on descriptors this function creates; the
    // read end is handed to one File, which owns and closes it.
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            log::warn!("stdout and stderr stay where they are: no pipe");
            return;
        }
        libc::dup2(fds[1], 1);
        libc::dup2(fds[1], 2);
        libc::close(fds[1]);
    }
    let read = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let spawned = std::thread::Builder::new()
        .name("stdio-to-log".into())
        .spawn(move || {
            for line in BufReader::new(read).lines() {
                match line {
                    Ok(line) => log::info!("{line}"),
                    Err(_) => break,
                }
            }
        });
    if spawned.is_err() {
        log::warn!("stdout and stderr are not logged: no thread");
    }
}
