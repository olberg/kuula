//! `kuula sandbox-probe <read-path> <write-path> <host:port>`: the
//! hostile native executable, built into the
//! binary so the tests need no scratch crate. It tries to read a file,
//! create a file and open a TCP connection, and prints one line per
//! attempt: `read ok`, `read denied` or `read other: <error>`, and the
//! same for `write` and `connect`. Always exits 0; the verdict is in the
//! lines. Run plainly it is the positive control; run through
//! `kuula sandbox-exec` it must be denied all three.
//!
//! Under an LPAC Winsock cannot even initialise (`WSAStartup` fails
//! with `WSASYSCALLFAILURE`), which Rust's std treats as a panic; the
//! probe catches that and reports it as `connect denied`.

use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

/// `WSAEACCES`: the socket layer's access-denied.
const WSAEACCES: i32 = 10013;
/// `ERROR_ACCESS_DENIED`.
const ACCESS_DENIED: i32 = 5;

fn verdict(kind: &str, result: std::io::Result<()>) -> String {
    match result {
        Ok(()) => format!("{kind} ok"),
        Err(e) if matches!(e.raw_os_error(), Some(ACCESS_DENIED) | Some(WSAEACCES)) => {
            format!("{kind} denied")
        }
        Err(e) => format!("{kind} other: {e}"),
    }
}

pub fn main(read: &Path, write: &Path, endpoint: &str) -> u8 {
    println!("{}", verdict("read", std::fs::read(read).map(|_| ())));
    let created = std::fs::File::create_new(write).map(|_| ());
    if created.is_ok() {
        let _ = std::fs::remove_file(write);
    }
    println!("{}", verdict("write", created));
    std::panic::set_hook(Box::new(|_| {}));
    let connect = std::panic::catch_unwind(|| {
        endpoint
            .to_socket_addrs()
            .and_then(|mut addrs| {
                addrs.next().ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "no address")
                })
            })
            .and_then(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(3)))
            .map(|_| ())
    });
    match connect {
        Ok(result) => println!("{}", verdict("connect", result)),
        Err(_) => println!("connect denied: winsock did not initialise"),
    }
    0
}
