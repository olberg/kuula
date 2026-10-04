//! A real private relay, with IP transports disabled on both peers.
use kuula_net::{config::RelayConfig, Net, NetConfig};
use std::time::{Duration, Instant};

#[test]
fn relay_only_exchanges_data_reports_path_and_detects_relay_loss() {
    let (url_tx, url_rx) = std::sync::mpsc::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut cfg = iroh_relay::server::ServerConfig::default();
            cfg.relay = Some(iroh_relay::server::RelayConfig::new(([127, 0, 0, 1], 0)));
            let server = iroh_relay::server::Server::spawn(cfg).await.unwrap();
            url_tx
                .send(format!("http://{}", server.http_addr().unwrap()))
                .unwrap();
            let _ = stop_rx.await;
            server.shutdown().await.unwrap();
        });
    });
    let url = url_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let config = NetConfig {
        enabled: true,
        relay: RelayConfig {
            url: Some(url),
            only: true,
        },
        ..Default::default()
    };
    let host = Net::new(&config).unwrap();
    let joiner = Net::new(&config).unwrap();
    assert!(
        host.bound().is_empty(),
        "relay-only must have no IP sockets"
    );
    assert!(joiner.bound().is_empty());
    let mut listener = host.listen().unwrap();
    assert!(listener.addresses().is_empty());
    let mut b = joiner.join(listener.ticket()).unwrap();
    let mut a = listener.accept(Duration::from_secs(5)).unwrap().unwrap();
    a.send_text("private relay").unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(kuula_net::Event::Text(s)) = b.try_recv().unwrap() {
            assert_eq!(s, "private relay");
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(a.path().starts_with("relay "), "{}", a.path());
    assert!(b.path().starts_with("relay "), "{}", b.path());
    stop_tx.send(()).unwrap();
    thread.join().unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        match b.try_recv() {
            Err(_) | Ok(Some(kuula_net::Event::PeerLost(_) | kuula_net::Event::Error(_))) => break,
            _ => {}
        }
        assert!(
            Instant::now() < until,
            "relay loss must end session within idle timeout"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
