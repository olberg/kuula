//! Deploy receivers and senders started and stopped in a process of their
//! own, so the thread count is this test's: a receiver, a push and the
//! drop of both leave no thread behind, and staging is empty after each
//! shutdown.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use kuula_core::{Snapshot, SnapshotLimits};
use kuula_net::deploy::{push, DeployCode, DeployReceiver, DeployStore, Package, ReceiverConfig};
use kuula_net::NetConfig;

fn loopback() -> NetConfig {
    NetConfig {
        enabled: true,
        relay: Default::default(),
        bind: Some("127.0.0.1:0".parse().unwrap()),
        cancel: None,
    }
}

/// Threads of this process.
#[cfg(windows)]
fn thread_count() -> usize {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    let pid = std::process::id();
    let mut count = 0;
    // SAFETY: plain Win32 calls with a correctly sized, zeroed entry.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        if Thread32First(snap, &mut entry) != 0 {
            loop {
                if entry.th32OwnerProcessID == pid {
                    count += 1;
                }
                if Thread32Next(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    count
}

#[cfg(not(windows))]
fn thread_count() -> usize {
    std::fs::read_dir("/proc/self/task")
        .map(|d| d.count())
        .unwrap_or(0)
}

fn package() -> Package {
    let snap = Snapshot::from_entries(
        [("main.lua", b"function _draw() cls() end".to_vec())],
        SnapshotLimits::default(),
    )
    .unwrap();
    Package::from_snapshot("cycle", &snap).unwrap()
}

#[test]
fn receiver_and_sender_cycles_leave_no_thread_behind() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("kuula-deploy-life-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let carts = root.join("carts");
    let rx_store = DeployStore::new(&root.join("rx"));
    let tx_store = DeployStore::new(&root.join("tx"));
    rx_store.approve(&tx_store.id().unwrap()).unwrap();
    let package = package();

    let cycle = |traffic: bool| {
        let rx = DeployReceiver::start(ReceiverConfig {
            carts: carts.clone(),
            store: rx_store.clone(),
            net: loopback(),
            restart: None,
        })
        .unwrap();
        if traffic {
            let report = push(
                &loopback(),
                tx_store.secret_key().unwrap(),
                rx.ticket(),
                &package,
            )
            .unwrap();
            assert_eq!(report.code, DeployCode::Ok, "{report:?}");
        }
        drop(rx);
        let staging = std::fs::read_dir(carts.join(".staging")).unwrap().count();
        assert_eq!(staging, 0, "staging is empty after shutdown");
    };

    // Warm up: the first endpoint and the first push pay for lazy setup.
    cycle(true);
    let before = thread_count();
    let t = Instant::now();
    for i in 0..10 {
        cycle(i % 2 == 0);
    }
    let elapsed = t.elapsed();
    let after = thread_count();
    assert!(
        after <= before,
        "threads before {before}, after {after}: something outlived its receiver or sender"
    );
    assert!(
        elapsed < Duration::from_secs(120),
        "10 cycles took {elapsed:?}"
    );
    eprintln!("10 deploy cycles: {elapsed:?}, threads {before} -> {after}");
    let _ = std::fs::remove_dir_all(&root);
}
