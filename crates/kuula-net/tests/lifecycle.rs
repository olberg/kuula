//! Repeated start and stop in a process of its own, so the thread count
//! is this test's and not the workspace's: a hundred create-drop cycles
//! leave no thread behind and finish within a bound.

use std::time::{Duration, Instant};

use kuula_net::{Net, NetConfig};

fn loopback() -> NetConfig {
    NetConfig {
        enabled: true,
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

#[test]
fn a_hundred_create_drop_cycles_leave_no_thread_behind() {
    // Warm up once: the first endpoint pays for lazy initialisation.
    drop(Net::new(&loopback()).unwrap());
    let before = thread_count();
    let t = Instant::now();
    for _ in 0..100 {
        let net = Net::new(&loopback()).unwrap();
        let _ = net.id();
        drop(net);
    }
    let elapsed = t.elapsed();
    let after = thread_count();
    assert!(
        after <= before,
        "threads before {before}, after {after}: something outlived its Net"
    );
    assert!(
        elapsed < Duration::from_secs(60),
        "100 cycles took {elapsed:?}"
    );
    eprintln!("100 create-drop cycles: {elapsed:?}, threads {before} -> {after}");
}
