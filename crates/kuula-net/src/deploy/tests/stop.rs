//! A transfer the receiver gives up on while the sender is still writing.
//! The receiver stops the stream, so that a sender blocked on flow
//! control gets an error at once and reads the result; the sender, for
//! its part, takes a stopped write as "read the result now". Each half is
//! tested against a peer written by hand.

use iroh::endpoint::WriteError;

use super::*;

fn block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

#[test]
fn a_sender_writing_past_its_offer_is_stopped_and_reads_the_result() {
    let env = Env::new();
    let rx = env.receiver();
    let pkg = package("game", "x = 1");
    let plan = Plan::honest("game", pkg.bytes().to_vec(), Mode::Full);
    let ticket = rx.ticket().to_string();
    let secret = env.tx.secret_key().unwrap();
    let result = std::thread::spawn(move || {
        block_on(async move {
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
            recv.read_exact(&mut answer).await.unwrap();
            assert_eq!(answer[0], 0);
            send.write_all(&plan.data).await.unwrap();
            // And on, past the offer and past any window, until the
            // receiver says stop. Without the stop these writes block on
            // flow control until the receiver closes the connection.
            let more = vec![7u8; wire::CHUNK];
            loop {
                match tokio::time::timeout(WAIT, send.write_all(&more)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(WriteError::Stopped(_))) => break,
                    other => panic!("the writes ended with {other:?}, not with a stop"),
                }
            }
            let mut header = [0u8; wire::RESULT_HEADER];
            recv.read_exact(&mut header).await.unwrap();
            let (code, restart, n) = wire::result_header(&header).unwrap();
            let mut body = vec![0u8; n];
            recv.read_exact(&mut body).await.unwrap();
            leave(&ep, &conn).await;
            (code, restart, wire::result_detail(&body).unwrap())
        })
    })
    .join()
    .unwrap();
    assert_eq!(result.0, DeployCode::Offer, "{result:?}");
    let (_, event) = finished(&rx);
    assert_eq!(finished_code(&event), DeployCode::Offer, "{event:?}");
    assert!(env.carts_listing().is_empty());
    assert!(env.staging().is_empty());
}

#[test]
fn a_push_the_receiver_stops_part_way_reports_the_receivers_result() {
    // Several MiB that do not compress: more than a stream's flow-control
    // window, so the sender is blocked in a write when the stop comes.
    let mut state: u64 = 0x0fed_cba9_8765_4321;
    let mut body = String::from("-- ");
    while body.len() < 4 * 1024 * 1024 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        body.push_str(&format!("{state:016x}"));
    }
    let pkg = Package::from_snapshot("big", &snapshot(&[("main.lua", &body)])).unwrap();
    assert!(pkg.bytes().len() > 2 * 1024 * 1024, "{}", pkg.bytes().len());

    // A receiver by hand: it accepts the offer, takes one read of the
    // package and then fails as one whose disk is full would.
    let env = Env::new();
    let net = Net::build(&loopback(), DEPLOY_ALPN, Some(env.rx.secret_key().unwrap())).unwrap();
    let inner = &net.inner;
    let addr = inner
        .block_on(crate::direct_addr(&inner.endpoint, &inner.relay))
        .unwrap()
        .unwrap();
    let ticket = EndpointTicket::new(addr).to_string();
    let endpoint = inner.endpoint.clone();
    inner.handle.spawn(async move {
        let conn = endpoint
            .accept()
            .await
            .unwrap()
            .accept()
            .unwrap()
            .await
            .unwrap();
        let (mut send, mut recv) = conn.accept_bi().await.unwrap();
        let mut prefix = [0u8; wire::OFFER_PREFIX];
        recv.read_exact(&mut prefix).await.unwrap();
        let n = wire::offer_prefix(&prefix).unwrap();
        let mut rest = vec![0u8; n + wire::OFFER_TAIL];
        recv.read_exact(&mut rest).await.unwrap();
        send.write_all(&[0]).await.unwrap();
        let mut buf = vec![0u8; wire::CHUNK];
        recv.read(&mut buf).await.unwrap();
        recv.stop(0u32.into()).unwrap();
        let result = wire::encode_result(DeployCode::Install, Restart::NotRun, "disk full");
        send.write_all(&result).await.unwrap();
        send.finish().unwrap();
        let _ = tokio::time::timeout(wire::LINGER, conn.closed()).await;
    });

    let started = Instant::now();
    let report = env.push(&ticket, &pkg).unwrap();
    assert_eq!(report.code, DeployCode::Install, "{report:?}");
    assert_eq!(report.detail, "disk full");
    assert_eq!(report.restart, Restart::NotRun);
    assert!(!report.installed());
    assert!(
        started.elapsed() < wire::STALL_DEADLINE,
        "the sender waited {:?}",
        started.elapsed()
    );
    net.shutdown();
}
