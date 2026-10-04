//! Two endpoints in one process on loopback: the happy path, every
//! refusal the protocol defines, and the receiver's lifecycle. The wire
//! limits are tested without a socket in `wire.rs`; the raw sender below
//! speaks the wire by hand so a test can send what the real sender never
//! would.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use iroh::endpoint::{ConnectionError, ReadError, ReadExactError};
use iroh::SecretKey;
use iroh_tickets::endpoint::EndpointTicket;
use kuula_core::{Snapshot, SnapshotLimits};
use sha2::{Digest, Sha256};

use super::wire::{self, close, DeployCode, Restart};
use super::*;
use crate::proto::DEPLOY_ALPN;
use crate::tests::{loopback, WAIT};
use crate::{Code, Net};

mod stop;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A scratch tree with a data directory for each side and a carts
/// directory for the receiver, removed on drop.
struct Env {
    root: PathBuf,
    carts: PathBuf,
    rx: DeployStore,
    tx: DeployStore,
}

impl Env {
    fn new() -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("kuula-deploy-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let carts = root.join("carts");
        fs::create_dir_all(&carts).unwrap();
        Env {
            rx: DeployStore::new(&root.join("rx")),
            tx: DeployStore::new(&root.join("tx")),
            carts,
            root,
        }
    }

    /// A receiver with no console that approves the sender.
    fn receiver(&self) -> DeployReceiver {
        self.tx_approved();
        self.receiver_unapproved(None)
    }

    fn tx_approved(&self) {
        self.rx.approve(&self.tx.id().unwrap()).unwrap();
    }

    fn receiver_unapproved(&self, restart: Option<mpsc::Sender<RestartRequest>>) -> DeployReceiver {
        DeployReceiver::start(ReceiverConfig {
            carts: self.carts.clone(),
            store: self.rx.clone(),
            net: loopback(),
            restart,
        })
        .unwrap()
    }

    fn push(&self, ticket: &str, package: &Package) -> Result<PushReport, DeployError> {
        push(&loopback(), self.tx.secret_key().unwrap(), ticket, package)
    }

    fn installed(&self, name: &str) -> Vec<u8> {
        fs::read(self.carts.join(format!("{name}.cart"))).unwrap()
    }

    fn staging(&self) -> Vec<PathBuf> {
        fs::read_dir(self.carts.join(STAGING_DIR))
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default()
    }

    /// Everything under the carts directory except the (empty) staging
    /// directory itself.
    fn carts_listing(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.carts)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != STAGING_DIR)
            .collect();
        names.sort();
        names
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn snapshot(files: &[(&str, &str)]) -> Snapshot {
    Snapshot::from_entries(
        files.iter().map(|(n, c)| (*n, c.as_bytes().to_vec())),
        SnapshotLimits::default(),
    )
    .unwrap()
}

fn package(name: &str, main: &str) -> Package {
    Package::from_snapshot(name, &snapshot(&[("main.lua", main)])).unwrap()
}

/// Wait for the next `Finished` event; the earlier ones are returned too.
fn finished(rx: &DeployReceiver) -> (Vec<DeployEvent>, DeployEvent) {
    let mut seen = Vec::new();
    let until = Instant::now() + Duration::from_secs(15);
    while Instant::now() < until {
        if let Some(event) = rx.next_event(Duration::from_millis(100)) {
            if matches!(event, DeployEvent::Finished { .. }) {
                return (seen, event);
            }
            seen.push(event);
        }
    }
    panic!("no Finished event; saw {seen:?}");
}

fn finished_code(event: &DeployEvent) -> DeployCode {
    match event {
        DeployEvent::Finished { code, .. } => *code,
        other => panic!("not a Finished event: {other:?}"),
    }
}

// ----- the raw sender -------------------------------------------------------

/// What a raw sender does after the offer.
#[derive(Clone, Copy)]
enum Mode {
    /// Write everything, finish, read the result.
    Full,
    /// Write `n` bytes, announce it, then wait for the result without
    /// writing more or finishing: a sender that stalls.
    StallAt(usize),
    /// Write `n` bytes and finish the stream: a sender that gives up.
    FinishAt(usize),
    /// Write `n` bytes, announce it, wait for `go`, then write the rest.
    Gate(usize),
    /// Write everything and `k` bytes more.
    Extra(usize),
}

struct Plan {
    name: Vec<u8>,
    declared: u32,
    digest: [u8; 32],
    data: Vec<u8>,
    mode: Mode,
}

impl Plan {
    /// An honest offer of `data`.
    fn honest(name: &str, data: Vec<u8>, mode: Mode) -> Plan {
        Plan {
            name: name.as_bytes().to_vec(),
            declared: data.len() as u32,
            digest: Sha256::digest(&data).into(),
            data,
            mode,
        }
    }

    fn offer(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&wire::MAGIC);
        out.extend_from_slice(&wire::VERSION.to_be_bytes());
        out.push(self.name.len() as u8);
        out.extend_from_slice(&self.name);
        out.extend_from_slice(&self.declared.to_be_bytes());
        out.extend_from_slice(&self.digest);
        out
    }
}

/// What a raw sender saw.
#[derive(Debug)]
struct Seen {
    /// The answer byte; `None` when the connection ended first.
    answer: Option<u8>,
    result: Option<(DeployCode, Restart, String)>,
    /// The application close code the receiver ended with, if any.
    closed: Option<u32>,
}

struct Raw {
    halfway: mpsc::Receiver<()>,
    go: tokio::sync::mpsc::UnboundedSender<()>,
    join: std::thread::JoinHandle<Seen>,
}

impl Raw {
    /// Run the plan on a thread of its own, which keeps the sender's
    /// endpoint turning while the test does other things.
    fn spawn(ticket: &str, secret: SecretKey, plan: Plan) -> Raw {
        let ticket = ticket.to_string();
        let (halfway_tx, halfway) = mpsc::channel();
        let (go, mut go_rx) = tokio::sync::mpsc::unbounded_channel();
        let join = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(raw_run(&ticket, secret, plan, halfway_tx, &mut go_rx))
        });
        Raw { halfway, go, join }
    }

    fn wait_halfway(&self) {
        self.halfway
            .recv_timeout(WAIT)
            .expect("the raw sender never got halfway");
    }

    fn finish(self) -> Seen {
        self.join.join().unwrap()
    }
}

fn app_close(e: &ConnectionError) -> Option<u32> {
    match e {
        ConnectionError::ApplicationClosed(c) => Some(u64::from(c.error_code) as u32),
        _ => None,
    }
}

fn read_close(e: &ReadExactError) -> Option<u32> {
    match e {
        ReadExactError::ReadError(ReadError::ConnectionLost(c)) => app_close(c),
        _ => None,
    }
}

async fn raw_run(
    ticket: &str,
    secret: SecretKey,
    plan: Plan,
    halfway: mpsc::Sender<()>,
    go: &mut tokio::sync::mpsc::UnboundedReceiver<()>,
) -> Seen {
    let mut seen = Seen {
        answer: None,
        result: None,
        closed: None,
    };
    let ep = crate::bind_with(&loopback(), DEPLOY_ALPN, Some(secret))
        .await
        .unwrap();
    let ticket: EndpointTicket = ticket.parse().unwrap();
    let conn = ep
        .connect(ticket.endpoint_addr().clone(), DEPLOY_ALPN)
        .await
        .unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send.write_all(&plan.offer()).await.unwrap();
    let mut answer = [0u8; 1];
    match recv.read_exact(&mut answer).await {
        Ok(()) => seen.answer = Some(answer[0]),
        Err(e) => {
            seen.closed = read_close(&e);
            return seen;
        }
    }
    if answer[0] != 0 {
        leave(&ep, &conn).await;
        return seen;
    }
    let all = &plan.data;
    let write = |n: usize| all[..n.min(all.len())].to_vec();
    match plan.mode {
        Mode::Full => {
            send.write_all(all).await.unwrap();
            send.finish().unwrap();
        }
        Mode::Extra(k) => {
            send.write_all(all).await.unwrap();
            send.write_all(&vec![7u8; k]).await.unwrap();
            send.finish().unwrap();
        }
        Mode::FinishAt(n) => {
            send.write_all(&write(n)).await.unwrap();
            send.finish().unwrap();
        }
        Mode::StallAt(n) => {
            send.write_all(&write(n)).await.unwrap();
            let _ = halfway.send(());
        }
        Mode::Gate(n) => {
            send.write_all(&write(n)).await.unwrap();
            let _ = halfway.send(());
            let _ = go.recv().await;
            send.write_all(&all[n.min(all.len())..]).await.unwrap();
            send.finish().unwrap();
        }
    }
    let mut header = [0u8; wire::RESULT_HEADER];
    match recv.read_exact(&mut header).await {
        Ok(()) => {
            let (code, restart, n) = wire::result_header(&header).unwrap();
            let mut body = vec![0u8; n];
            recv.read_exact(&mut body).await.unwrap();
            seen.result = Some((code, restart, wire::result_detail(&body).unwrap()));
        }
        Err(e) => seen.closed = read_close(&e),
    }
    leave(&ep, &conn).await;
    seen
}

/// Close the connection and let the close frame out, so the receiver
/// does not wait for it.
async fn leave(ep: &iroh::Endpoint, conn: &iroh::endpoint::Connection) {
    conn.close(close::BYE.into(), close::reason(close::BYE));
    let _ = tokio::time::timeout(Duration::from_secs(1), ep.close()).await;
}

/// Connect and report the close code the receiver ended with, without
/// ever opening a stream.
fn close_code_of(ticket: &str, secret: SecretKey) -> Option<u32> {
    let ticket = ticket.to_string();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let ep = crate::bind_with(&loopback(), DEPLOY_ALPN, Some(secret))
                .await
                .unwrap();
            let ticket: EndpointTicket = ticket.parse().unwrap();
            let conn = ep
                .connect(ticket.endpoint_addr().clone(), DEPLOY_ALPN)
                .await
                .unwrap();
            tokio::time::timeout(WAIT, conn.closed())
                .await
                .ok()
                .and_then(|e| app_close(&e))
        })
    })
    .join()
    .unwrap()
}

/// Run an honest-or-not plan to the end.
fn raw(env: &Env, ticket: &str, plan: Plan) -> Seen {
    Raw::spawn(ticket, env.tx.secret_key().unwrap(), plan).finish()
}

// ----- tests ------------------------------------------------------------------

#[test]
fn an_approved_push_installs_and_the_file_equals_the_package() {
    let env = Env::new();
    let rx = env.receiver();
    let pkg = package("hello", "function _draw() cls() end");
    let report = env.push(rx.ticket(), &pkg).unwrap();
    assert_eq!(report.code, DeployCode::Ok, "{report:?}");
    assert_eq!(report.restart, Restart::NotRun);
    assert_eq!(report.bytes, pkg.bytes().len() as u64);
    assert_eq!(report.digest, pkg.digest_hex());
    assert_eq!(
        report.stages(),
        Stages {
            transfer: "ok",
            validation: "ok",
            install: "ok"
        }
    );
    assert_eq!(env.installed("hello"), pkg.bytes());
    let (before, done) = finished(&rx);
    assert!(
        matches!(&before[..], [DeployEvent::Offered { name, bytes, .. }]
            if name == "hello" && *bytes as usize == pkg.bytes().len()),
        "{before:?}"
    );
    assert_eq!(finished_code(&done), DeployCode::Ok);
    assert!(env.staging().is_empty(), "{:?}", env.staging());

    // A newer version replaces it, atomically, and the file again
    // equals the package.
    let newer = package("hello", "function _draw() cls(1) end");
    assert_ne!(newer.bytes(), pkg.bytes());
    assert_eq!(env.push(rx.ticket(), &newer).unwrap().code, DeployCode::Ok);
    assert_eq!(env.installed("hello"), newer.bytes());
    assert_eq!(env.carts_listing(), ["hello.cart"]);
    assert!(env.staging().is_empty());
}

#[test]
fn a_large_package_streams_through_staging_and_arrives_intact() {
    // Several MiB of pseudo-random hex in one Lua comment: far more than
    // one 64 KiB read, and it does not compress to nothing.
    let mut state: u64 = 0x1234_5678_9abc_def0;
    let mut body = String::from("-- ");
    while body.len() < 8 * 1024 * 1024 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        body.push_str(&format!("{state:016x}"));
    }
    let env = Env::new();
    let rx = env.receiver();
    let pkg = Package::from_snapshot("big", &snapshot(&[("main.lua", &body)])).unwrap();
    assert!(
        pkg.bytes().len() > 2 * wire::CHUNK * 8,
        "{}",
        pkg.bytes().len()
    );
    let report = env.push(rx.ticket(), &pkg).unwrap();
    assert_eq!(report.code, DeployCode::Ok, "{report:?}");
    assert_eq!(report.bytes, pkg.bytes().len() as u64);
    assert_eq!(env.installed("big"), pkg.bytes());
    assert!(env.staging().is_empty());
}

#[test]
fn a_packed_zip_is_sent_as_it_is() {
    let env = Env::new();
    let rx = env.receiver();
    let zip = env.root.join("mine.zip");
    fs::write(
        &zip,
        kuula_core::zipsource::pack(&snapshot(&[
            ("main.lua", "x = 1"),
            ("cart.toml", "[cart]\ntitle = \"T\"\n"),
        ]))
        .unwrap(),
    )
    .unwrap();
    let pkg = Package::from_zip_file("mine", &zip).unwrap();
    assert_eq!(pkg.bytes(), fs::read(&zip).unwrap());
    assert_eq!(env.push(rx.ticket(), &pkg).unwrap().code, DeployCode::Ok);
    assert_eq!(env.installed("mine"), pkg.bytes());
}

#[test]
fn an_unapproved_endpoint_is_closed_with_code_5_and_nothing_is_written() {
    let env = Env::new();
    let rx = env.receiver_unapproved(None);
    let id = env.tx.id().unwrap();
    let pkg = package("hello", "x = 1");

    // The close comes before any stream: the raw peer opens none.
    assert_eq!(
        close_code_of(rx.ticket(), env.tx.secret_key().unwrap()),
        Some(close::UNPAIRED)
    );
    let e = env.push(rx.ticket(), &pkg).unwrap_err();
    assert_eq!(e.code(), "deploy_unpaired", "{e}");
    assert!(matches!(e, DeployError::Refused(_)));
    // The receiver says who was refused, so a person can approve it.
    let event = rx.next_event(WAIT).expect("an Unpaired event");
    assert_eq!(event, DeployEvent::Unpaired { id: id.clone() });
    assert!(env.carts_listing().is_empty(), "{:?}", env.carts_listing());
    assert!(env.staging().is_empty());

    // Approving while the receiver runs takes effect for the next
    // connection; revoking takes it away again.
    env.tx_approved();
    assert_eq!(env.push(rx.ticket(), &pkg).unwrap().code, DeployCode::Ok);
    env.rx.revoke(&id).unwrap();
    assert_eq!(
        env.push(rx.ticket(), &pkg).unwrap_err().code(),
        "deploy_unpaired"
    );
    assert_eq!(env.installed("hello"), pkg.bytes());
}

#[test]
fn a_receiver_whose_ticket_names_another_id_is_not_trusted() {
    // The sender authenticates the receiver by the id in the ticket: a
    // ticket with the right address and the wrong id fails the handshake.
    let env = Env::new();
    let rx = env.receiver();
    let ticket: EndpointTicket = rx.ticket().parse().unwrap();
    let wrong = SecretKey::from_bytes(&[42; 32]).public();
    let forged = iroh::EndpointAddr::from(wrong).with_addrs(ticket.endpoint_addr().addrs.clone());
    let forged = EndpointTicket::new(forged).to_string();
    let e = env.push(&forged, &package("hello", "x = 1")).unwrap_err();
    assert!(matches!(e, DeployError::Net(_)), "{e}");
    assert!(env.carts_listing().is_empty());
}

#[test]
fn refusals_leave_the_installed_cart_byte_identical() {
    let env = Env::new();
    let rx = env.receiver();
    let v1 = package("game", "x = 1");
    assert_eq!(env.push(rx.ticket(), &v1).unwrap().code, DeployCode::Ok);
    finished(&rx);
    let v2 = package("game", "x = 2");

    let check = |what: &str, plan: Plan, answer: Option<u8>, code: DeployCode| {
        let seen = raw(&env, rx.ticket(), plan);
        let (_, event) = finished(&rx);
        assert_eq!(seen.answer, answer, "{what}: {seen:?}");
        assert_eq!(finished_code(&event), code, "{what}: {event:?}");
        if answer == Some(0) {
            let (got, _, detail) = seen.result.clone().expect("a result frame");
            assert_eq!(got, code, "{what}: {detail}");
        }
        assert_eq!(env.installed("game"), v1.bytes(), "{what}");
        assert_eq!(env.carts_listing(), ["game.cart"], "{what}");
        assert!(env.staging().is_empty(), "{what}: {:?}", env.staging());
        seen
    };

    // The digest does not match the bytes.
    let mut wrong = Plan::honest("game", v2.bytes().to_vec(), Mode::Full);
    wrong.digest = [0; 32];
    let seen = check("digest", wrong, Some(0), DeployCode::Digest);
    assert!(seen.result.unwrap().2.contains("received"));

    // An offer over the limit is refused by its answer, before any data.
    let mut big = Plan::honest("game", v2.bytes().to_vec(), Mode::Full);
    big.declared = wire::MAX_PACKAGE + 1;
    check("too large", big, Some(2), DeployCode::TooLarge);
    let mut huge = Plan::honest("game", v2.bytes().to_vec(), Mode::Full);
    huge.declared = u32::MAX;
    check("huge", huge, Some(2), DeployCode::TooLarge);

    // Bad names, a bad length and a bad magic are offer faults.
    for bad in ["Game", "a b", "../game", "g\u{e4}me"] {
        let plan = Plan::honest(bad, v2.bytes().to_vec(), Mode::Full);
        check(bad, plan, Some(1), DeployCode::Offer);
    }
    let mut zero = Plan::honest("game", v2.bytes().to_vec(), Mode::Full);
    zero.declared = 0;
    check("zero length", zero, Some(1), DeployCode::Offer);
    let mut long_name = Plan::honest("game", v2.bytes().to_vec(), Mode::Full);
    long_name.name = vec![b'a'; 33];
    check("long name", long_name, Some(1), DeployCode::Offer);

    // Not a zip.
    let garbage = Plan::honest("game", b"this is not a zip file".to_vec(), Mode::Full);
    let seen = check("not a zip", garbage, Some(0), DeployCode::Invalid);
    assert!(seen.result.unwrap().2.len() > 3);

    // A zip without main.lua, and one whose manifest does not parse.
    let no_main =
        kuula_core::zipsource::pack(&snapshot(&[("cart.toml", "title = \"x\"\n")])).unwrap();
    let seen = check(
        "no main.lua",
        Plan::honest("game", no_main, Mode::Full),
        Some(0),
        DeployCode::Invalid,
    );
    assert!(seen.result.unwrap().2.contains("main.lua"));
    let bad_toml = kuula_core::zipsource::pack(&snapshot(&[
        ("main.lua", "x = 1"),
        ("cart.toml", "title = ["),
    ]))
    .unwrap();
    let seen = check(
        "bad manifest",
        Plan::honest("game", bad_toml, Mode::Full),
        Some(0),
        DeployCode::Invalid,
    );
    assert!(seen.result.unwrap().2.contains("manifest_error"));

    // More bytes than offered.
    check(
        "extra data",
        Plan::honest("game", v2.bytes().to_vec(), Mode::Extra(10)),
        Some(0),
        DeployCode::Offer,
    );

    // After all of that the receiver still installs.
    assert_eq!(env.push(rx.ticket(), &v2).unwrap().code, DeployCode::Ok);
    assert_eq!(env.installed("game"), v2.bytes());
}

#[test]
fn a_sender_that_gives_up_halfway_is_interrupted_and_the_old_cart_stays() {
    let env = Env::new();
    let rx = env.receiver();
    let v1 = package("game", "x = 1");
    env.push(rx.ticket(), &v1).unwrap();
    finished(&rx);
    let v2 = package("game", &format!("x = 2 -- {}", "pad ".repeat(2000)));

    // It finishes its stream short of the offered length.
    let half = v2.bytes().len() / 2;
    let seen = raw(
        &env,
        rx.ticket(),
        Plan::honest("game", v2.bytes().to_vec(), Mode::FinishAt(half)),
    );
    let (_, event) = finished(&rx);
    assert_eq!(finished_code(&event), DeployCode::Interrupted, "{event:?}");
    assert_eq!(seen.result.unwrap().0, DeployCode::Interrupted);
    assert_eq!(env.installed("game"), v1.bytes());
    assert!(env.staging().is_empty());
}

#[test]
fn a_sender_that_stalls_halfway_is_interrupted_after_five_seconds() {
    let env = Env::new();
    let rx = env.receiver();
    let v1 = package("game", "x = 1");
    env.push(rx.ticket(), &v1).unwrap();
    finished(&rx);
    let v2 = package("game", &format!("x = 2 -- {}", "pad ".repeat(2000)));

    let half = v2.bytes().len() / 2;
    let started = Instant::now();
    let sender = Raw::spawn(
        rx.ticket(),
        env.tx.secret_key().unwrap(),
        Plan::honest("game", v2.bytes().to_vec(), Mode::StallAt(half)),
    );
    sender.wait_halfway();
    // The partial package sits in staging, not in the carts directory.
    let began = Instant::now();
    while env.staging().is_empty() && began.elapsed() < WAIT {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(env.staging().len(), 1);
    assert_eq!(env.carts_listing(), ["game.cart"]);
    assert_eq!(env.installed("game"), v1.bytes());

    let (_, event) = finished(&rx);
    let took = started.elapsed();
    assert_eq!(finished_code(&event), DeployCode::Interrupted, "{event:?}");
    assert!(
        took >= wire::STALL_DEADLINE && took < Duration::from_secs(12),
        "{took:?}"
    );
    let seen = sender.finish();
    assert_eq!(seen.result.unwrap().0, DeployCode::Interrupted);
    assert_eq!(env.installed("game"), v1.bytes());
    assert!(env.staging().is_empty());
}

#[test]
fn an_offer_that_never_completes_times_out() {
    let env = Env::new();
    let rx = env.receiver();
    let started = Instant::now();
    // The magic and nothing more.
    let ticket = rx.ticket().to_string();
    let secret = env.tx.secret_key().unwrap();
    let seen = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let ep = crate::bind_with(&loopback(), DEPLOY_ALPN, Some(secret))
                .await
                .unwrap();
            let ticket: EndpointTicket = ticket.parse().unwrap();
            let conn = ep
                .connect(ticket.endpoint_addr().clone(), DEPLOY_ALPN)
                .await
                .unwrap();
            let (mut send, _recv) = conn.open_bi().await.unwrap();
            send.write_all(b"KUU").await.unwrap();
            // Longer than the deadline under test: the close comes when
            // the offer deadline passes, not before.
            tokio::time::timeout(wire::OFFER_DEADLINE + WAIT, conn.closed())
                .await
                .ok()
                .and_then(|e| app_close(&e))
        })
    })
    .join()
    .unwrap();
    let (_, event) = finished(&rx);
    assert_eq!(finished_code(&event), DeployCode::Offer);
    assert_eq!(seen, Some(close::TIMEOUT));
    assert!(started.elapsed() >= wire::OFFER_DEADLINE);
    assert!(env.carts_listing().is_empty());
}

#[test]
fn a_second_sender_during_a_transfer_is_busy() {
    let env = Env::new();
    let rx = env.receiver();
    let pkg = package("game", &format!("x = 1 -- {}", "pad ".repeat(2000)));
    let half = pkg.bytes().len() / 2;
    let first = Raw::spawn(
        rx.ticket(),
        env.tx.secret_key().unwrap(),
        Plan::honest("game", pkg.bytes().to_vec(), Mode::Gate(half)),
    );
    first.wait_halfway();

    // An approved sender gets busy (close code 1), an unapproved one
    // still gets unpaired: pairing is decided first.
    let e = env
        .push(rx.ticket(), &package("other", "y = 1"))
        .unwrap_err();
    assert_eq!(e.code(), "deploy_busy", "{e}");
    assert_eq!(
        close_code_of(rx.ticket(), SecretKey::from_bytes(&[9; 32])),
        Some(close::UNPAIRED)
    );

    // The first transfer is untouched and completes.
    first.go.send(()).unwrap();
    let seen = first.finish();
    assert_eq!(seen.result.unwrap().0, DeployCode::Ok);
    assert_eq!(env.installed("game"), pkg.bytes());
    assert_eq!(env.carts_listing(), ["game.cart"]);
    let mut events = Vec::new();
    while let Some(e) = rx.next_event(Duration::from_millis(300)) {
        events.push(e);
    }
    assert!(
        events.iter().any(|e| matches!(e, DeployEvent::Busy { .. })),
        "{events:?}"
    );
    // Once it is over, the receiver takes the next one.
    assert_eq!(
        env.push(rx.ticket(), &package("other", "y = 1"))
            .unwrap()
            .code,
        DeployCode::Ok
    );
}

#[test]
fn staging_is_emptied_at_start_and_a_shutdown_cancels_a_transfer_in_flight() {
    let env = Env::new();
    // Leftovers of a crashed receiver.
    let staging = env.carts.join(STAGING_DIR);
    fs::create_dir_all(staging.join("sub")).unwrap();
    fs::write(staging.join("incoming-0.part"), b"stale").unwrap();
    fs::write(staging.join("sub").join("x"), b"stale").unwrap();
    let rx = env.receiver();
    assert!(env.staging().is_empty(), "{:?}", env.staging());

    let v1 = package("game", "x = 1");
    env.push(rx.ticket(), &v1).unwrap();
    let v2 = package("game", &format!("x = 2 -- {}", "pad ".repeat(2000)));
    let half = v2.bytes().len() / 2;
    let sender = Raw::spawn(
        rx.ticket(),
        env.tx.secret_key().unwrap(),
        Plan::honest("game", v2.bytes().to_vec(), Mode::StallAt(half)),
    );
    sender.wait_halfway();
    let began = Instant::now();
    while env.staging().is_empty() && began.elapsed() < WAIT {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(env.staging().len(), 1, "a partial file is staged");

    let t = Instant::now();
    rx.shutdown();
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert!(env.staging().is_empty(), "{:?}", env.staging());
    assert_eq!(env.installed("game"), v1.bytes());
    let seen = sender.finish();
    match seen.result {
        Some((code, ..)) => assert_eq!(code, DeployCode::Cancelled),
        None => assert!(seen.closed.is_some(), "{seen:?}"),
    }
}

#[test]
fn the_restart_status_comes_from_the_console() {
    let env = Env::new();
    env.tx_approved();
    let (tx, requests) = mpsc::channel::<RestartRequest>();
    let rx = env.receiver_unapproved(Some(tx));
    // A console on another thread: started, then faulted with a code,
    // then silent past the deadline, then gone.
    let console = std::thread::spawn(move || {
        let first = requests.recv().unwrap();
        assert_eq!(first.name, "game");
        assert!(first.path.ends_with("game.cart"));
        assert!(
            first.path.is_file(),
            "the file is installed before the console is asked"
        );
        first.reply.send(Restart::Started, "");
        let second = requests.recv().unwrap();
        second.reply.send(Restart::Faulted, "script_error");
        let third = requests.recv().unwrap();
        std::thread::sleep(wire::RESTART_DEADLINE + Duration::from_millis(700));
        third.reply.send(Restart::Started, "too late");
        let fourth = requests.recv().unwrap();
        drop(fourth.reply);
    });
    let started = env.push(rx.ticket(), &package("game", "x = 1")).unwrap();
    assert_eq!(
        (started.code, started.restart),
        (DeployCode::Ok, Restart::Started)
    );
    let faulted = env.push(rx.ticket(), &package("game", "x = 2")).unwrap();
    assert_eq!(faulted.restart, Restart::Faulted);
    assert_eq!(faulted.detail, "script_error");
    assert_eq!(
        faulted.code,
        DeployCode::Ok,
        "an installed cart that faults is installed"
    );
    let t = Instant::now();
    let slow = env.push(rx.ticket(), &package("game", "x = 3")).unwrap();
    assert_eq!(
        (slow.code, slow.restart),
        (DeployCode::Ok, Restart::Timeout)
    );
    assert!(t.elapsed() >= wire::RESTART_DEADLINE, "{:?}", t.elapsed());
    let dropped = env.push(rx.ticket(), &package("game", "x = 4")).unwrap();
    assert_eq!(dropped.restart, Restart::Timeout);
    console.join().unwrap();
    // The console is gone: nothing is asked, `not_run`.
    let gone = env.push(rx.ticket(), &package("game", "x = 5")).unwrap();
    assert_eq!((gone.code, gone.restart), (DeployCode::Ok, Restart::NotRun));
}

#[test]
fn the_two_alpns_are_never_registered_together() {
    // A play endpoint does not speak deploy and a deploy endpoint does
    // not speak play: each side names the mismatch.
    let env = Env::new();
    let rx = env.receiver();
    let play = Net::new(&loopback()).unwrap();
    let e = play.join(rx.ticket()).unwrap_err();
    assert_eq!(e.code, Code::ProtocolMismatch, "{e}");

    let listener_net = Net::new(&loopback()).unwrap();
    let mut listener = listener_net.listen().unwrap();
    let e = env
        .push(listener.ticket(), &package("game", "x = 1"))
        .unwrap_err();
    assert_eq!(e.code(), "net_protocol_mismatch", "{e}");
    assert!(e.detail().contains("kuula/deploy/1"), "{e}");
    let e = listener.accept(WAIT).unwrap_err();
    assert_eq!(e.code, Code::ProtocolMismatch);
    assert!(env.carts_listing().is_empty());
}

#[test]
fn a_cancelled_push_returns_net_cancelled() {
    let env = Env::new();
    let rx = env.receiver();
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let config = crate::NetConfig {
        cancel: Some(flag),
        ..loopback()
    };
    let e = push(
        &config,
        env.tx.secret_key().unwrap(),
        rx.ticket(),
        &package("game", "x = 1"),
    )
    .unwrap_err();
    assert_eq!(e.code(), "net_cancelled", "{e}");
}

#[test]
fn local_checks_refuse_a_bad_package_before_anything_is_sent() {
    let env = Env::new();
    let none = Snapshot::empty();
    let e = Package::from_snapshot("game", &none).unwrap_err();
    assert_eq!(e.code, DeployCode::Invalid);
    assert!(e.detail.contains("main.lua"), "{e}");
    let ok = snapshot(&[("main.lua", "x = 1")]);
    assert_eq!(
        Package::from_snapshot("Game", &ok).unwrap_err().code,
        DeployCode::Offer
    );
    let bad_manifest = snapshot(&[("main.lua", "x = 1"), ("cart.toml", "title = [")]);
    let e = Package::from_snapshot("game", &bad_manifest).unwrap_err();
    assert_eq!(e.code, DeployCode::Invalid);
    assert!(e.detail.starts_with("manifest_error cart.toml"), "{e}");
    let junk = env.root.join("junk.zip");
    fs::write(&junk, b"not a zip").unwrap();
    assert_eq!(
        Package::from_zip_file("junk", &junk).unwrap_err().code,
        DeployCode::Invalid
    );
    assert_eq!(
        Package::from_zip_file("junk", Path::new("/nonexistent/x.zip"))
            .unwrap_err()
            .code,
        DeployCode::Invalid
    );
}
