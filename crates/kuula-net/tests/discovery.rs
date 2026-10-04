use kuula_core::{net::identity::Identity, Snapshot, SnapshotLimits};
use kuula_net::{
    discovery::{Advertisement, Discovery},
    Net, NetConfig,
};
use std::time::{Duration, Instant};

#[test]
fn two_local_discovery_sockets_find_a_compatible_host() {
    let snap = Snapshot::from_entries(
        [(
            "main.lua",
            format!("discovery {}", std::process::id()).into_bytes(),
        )],
        SnapshotLimits::default(),
    )
    .unwrap();
    let id = Identity::new(&snap);
    let host = Net::new(&NetConfig {
        enabled: true,
        bind: Some("127.0.0.1:0".parse().unwrap()),
        ..Default::default()
    })
    .unwrap();
    let listener = host.listen().unwrap();
    let advert = Advertisement::new(&id, "LAN fixture", listener.ticket());
    let mut a = Discovery::open().unwrap();
    let mut b = Discovery::open().unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        a.poll(Some(&advert), &id, listener.ticket()).unwrap();
        if b.poll(None, &id, "")
            .unwrap()
            .iter()
            .any(|c| c.ticket == listener.ticket())
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "multicast advertisement not received"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
