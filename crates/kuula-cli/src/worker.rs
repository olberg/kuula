//! The worker side: `kuula worker` reads framed messages on stdin, runs
//! the console and writes frames to stdout. It never touches the file
//! system; the cart arrives as a snapshot in the `Load` message, the save
//! slots with it, and every save the cart makes travels back to the
//! broker in the next `Frame`, which persists it.

use std::cell::RefCell;
use std::io::{self, BufReader, BufWriter};
use std::rc::Rc;

use kuula_core::{Console, MemoryStore, SaveError, SaveStore};
use kuula_lua::LuaGuest;

use crate::ipc::{self, Message, ReadError};

/// Exit code when the broker spoke garbage.
pub const EXIT_PROTOCOL: u8 = 3;

/// Test hook: when set, the worker exits on its first `Step` so the
/// broker's dead-worker path can be exercised end to end.
pub const CRASH_HOOK: &str = "KUULA_TEST_WORKER_CRASH";

/// Test hook: when set, the worker never answers its first `Step`, so
/// the broker's watchdog path can be exercised end to end.
pub const HANG_HOOK: &str = "KUULA_TEST_WORKER_HANG";

/// The slots written since the last frame was sent, last write per slot.
type Writes = Rc<RefCell<Vec<(u8, Vec<u8>)>>>;

/// A memory store that notes each write for the next `Frame`.
struct RecordingStore {
    inner: MemoryStore,
    writes: Writes,
}

impl SaveStore for RecordingStore {
    fn read(&mut self, slot: u8) -> Result<Option<Vec<u8>>, SaveError> {
        self.inner.read(slot)
    }

    fn write(&mut self, slot: u8, bytes: &[u8]) -> Result<(), SaveError> {
        self.inner.write(slot, bytes)?;
        let mut writes = self.writes.borrow_mut();
        writes.retain(|(s, _)| *s != slot);
        writes.push((slot, bytes.to_vec()));
        Ok(())
    }
}

pub fn main() -> u8 {
    let mut stdin = BufReader::new(io::stdin().lock());
    let mut stdout = BufWriter::new(io::stdout().lock());
    let mut console: Option<Console> = None;
    let writes: Writes = Rc::new(RefCell::new(Vec::new()));
    loop {
        let msg = match ipc::read_frame(&mut stdin) {
            Ok(Some(m)) => m,
            Ok(None) => return 0,
            Err(ReadError::Io(e)) => {
                eprintln!("worker: pipe: {e}");
                return EXIT_PROTOCOL;
            }
            Err(ReadError::Proto(e)) => {
                eprintln!("worker: protocol: {e}");
                return EXIT_PROTOCOL;
            }
        };
        let reply = match msg {
            Message::Load {
                snapshot,
                saves,
                net,
            } => match MemoryStore::from_slots(&saves) {
                Ok(store) => {
                    let mut c = LuaGuest::console(Rc::new(snapshot));
                    c.set_save_store(Box::new(RecordingStore {
                        inner: store,
                        writes: writes.clone(),
                    }));
                    c.set_net_env(net);
                    let out = c.output();
                    let ready = Message::Ready {
                        width: out.width,
                        height: out.height,
                    };
                    console = Some(c);
                    ready
                }
                Err(e) => Message::Error {
                    code: "worker_error".into(),
                    message: format!("initial saves refused: {e}"),
                },
            },
            Message::Step { input, events } => match console.as_mut() {
                _ if std::env::var_os(CRASH_HOOK).is_some() => std::process::exit(9),
                _ if std::env::var_os(HANG_HOOK).is_some() => loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                },
                Some(c) => {
                    // The events were bounded by the decoder; the cart's
                    // commands come back with the frame, and the room
                    // its inbox has left tells the broker how many
                    // events the next step may carry.
                    let frame = kuula_host_headless::step_console(c, input, events);
                    let commands = c.take_net_commands();
                    Message::Frame {
                        frame,
                        saves: std::mem::take(&mut *writes.borrow_mut()),
                        commands,
                        net_room: c.net_room() as u32,
                    }
                }
                None => Message::Error {
                    code: "worker_error".into(),
                    message: "Step before Load".into(),
                },
            },
            Message::Stop => return 0,
            other => {
                eprintln!("worker: unexpected message from broker");
                let _ = other;
                return EXIT_PROTOCOL;
            }
        };
        if let Err(e) = ipc::write_frame(&mut stdout, &reply) {
            eprintln!("worker: pipe: {e}");
            return EXIT_PROTOCOL;
        }
    }
}
