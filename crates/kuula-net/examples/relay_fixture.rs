//! Bounded local/LAN relay for adapter tests; no TLS, never deploy publicly.
fn main() {
    let bind: std::net::SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:3340".into())
        .parse()
        .expect("bind address");
    let seconds: u64 = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "120".into())
        .parse()
        .expect("seconds");
    assert!((1..=600).contains(&seconds));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut cfg = iroh_relay::server::ServerConfig::default();
            cfg.relay = Some(iroh_relay::server::RelayConfig::new(bind));
            let server = iroh_relay::server::Server::spawn(cfg).await.unwrap();
            println!("relay: http://{}", server.http_addr().unwrap());
            tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
            server.shutdown().await.unwrap();
        });
}
