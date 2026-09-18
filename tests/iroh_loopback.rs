//! Real Rust iroh endpoints on 127.0.0.1: ports of `transport/iroh_test.go` (verification §4.7). These bind
//! UDP sockets, so they run in CI but never inside the Nix sandbox.
//!
//! - `loopback_exchange` ports `TestIrohLoopback`.
//! - `dial_returns_handshaken_connections_and_dead_addresses_fail` ports `TestIrohDialWaitsForHandshake`.
//!   Rust `connect` returns only completed handshakes, so the 0-RTT resumption loop is a few plain dials.
//! - `path_rtt_is_unknown_until_sampled` ports `TestIrohPathRTTIsUnknownUntilSampled` (the 333 ms sentinel).
//! - `discover_by_id_over_mdns` ports `TestIrohDiscoverByID`. Announcing is node-side and not implemented,
//!   so a go-iroh-shaped announcement is multicast by hand; it skips when no mDNS listener can be opened.
//! - `discovery_without_answers` covers discoverDial when nothing answers; it needs no multicast loopback.
//! - The other tests cover the dial-phase error texts, FIN and STOP_SENDING, connection close, the pool over
//!   iroh, and bind options.
//! - `live_go_node_view_call` (ignored) dials a running Go dstore node.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dstore_gocompat::ctx::Ctx;
use dstore_gocompat::slog::{Level, Logger, TextHandler};
use dstore_gocompat::time::FixedZone;
use dstore_transport::{Conn, Endpoint, NodeId, PathInfo, Pool, TransportError};
use dstore_transport_iroh::{
    IrohConfig, IrohEndpoint, bind_iroh, generate_secret_key, ifaces, mdns,
};
use dstore_wire::{ALPN_CLIENT, ALPN_CLUSTER, Msg, T_PING, T_PONG, T_VIEW, T_VIEW_REPLY};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TEST_TIMEOUT: Duration = Duration::from_secs(60);

fn quiet_logger() -> Logger {
    Logger::new(Arc::new(TextHandler::new(
        Box::new(std::io::sink()),
        Level::ERROR,
        Arc::new(FixedZone(0)),
    )))
}

/// A loopback-only endpoint: one IPv4 socket on 127.0.0.1, advertising `ip:127.0.0.1:<port>`.
fn config(alpns: &[&str]) -> IrohConfig {
    IrohConfig {
        secret_key: generate_secret_key(),
        alpns: alpns.iter().map(|a| a.to_string()).collect(),
        relay: None,
        advertise: None,
        bind_addr: Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
        loopback: true,
        direct_timeout: None,
        discover: false,
        announce: false,
        logger: Some(quiet_logger()),
    }
}

async fn bind(cfg: IrohConfig) -> Arc<IrohEndpoint> {
    bind_iroh(&Ctx::background(), cfg)
        .await
        .unwrap_or_else(|e| panic!("bind: {e}"))
}

async fn within<F: std::future::Future<Output = ()>>(f: F) {
    if tokio::time::timeout(TEST_TIMEOUT, f).await.is_err() {
        panic!("test did not finish within {TEST_TIMEOUT:?}");
    }
}

fn ping_msg(epoch: u64) -> Msg {
    Msg {
        typ: T_PING,
        epoch,
        ..Default::default()
    }
}

/// Accepts connections and answers every stream's frame with TPong epoch+1, as the Go tests' servers do.
fn serve_pongs(server: Arc<IrohEndpoint>, ctx: Ctx) {
    tokio::spawn(async move {
        while let Ok(c) = server.accept(&ctx).await {
            let ctx = ctx.clone();
            tokio::spawn(async move {
                while let Ok(mut s) = c.accept_stream(&ctx).await {
                    if let Ok(m) = dstore_wire::read_msg(&mut *s.recv).await {
                        let pong = Msg {
                            typ: T_PONG,
                            epoch: m.epoch + 1,
                            ..Default::default()
                        };
                        let _ = dstore_wire::write_msg(&mut *s.send, &pong).await;
                    }
                    s.close_stream();
                }
            });
        }
    });
}

/// One request/reply on a new stream: write, FIN, read one frame, close the stream.
async fn ping(ctx: &Ctx, conn: &dyn Conn, epoch: u64) -> Msg {
    let mut s = conn
        .open_stream(ctx)
        .await
        .unwrap_or_else(|e| panic!("open_stream: {e}"));
    dstore_wire::write_msg(&mut *s.send, &ping_msg(epoch))
        .await
        .unwrap_or_else(|e| panic!("write_msg: {e}"));
    s.close_write();
    let m = dstore_wire::read_msg(&mut *s.recv)
        .await
        .unwrap_or_else(|e| panic!("read_msg: {e}"));
    s.close_stream();
    m
}

/// A loopback UDP address nothing listens on (Go `deadLoopbackAddr`), as a bare `ip:port`.
fn dead_loopback_addr() -> String {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").expect("udp bind");
    let addr = sock.local_addr().expect("local addr").to_string();
    drop(sock);
    addr
}

fn err_text<T>(r: Result<T, TransportError>) -> String {
    match r {
        Ok(_) => panic!("expected an error"),
        Err(e) => e.to_string(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loopback_exchange() {
    within(async {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(30));
        let server = bind(config(&[ALPN_CLIENT, ALPN_CLUSTER])).await;
        let client = bind(config(&[])).await;
        let port = server.raw().bound_sockets()[0].port();
        assert_eq!(server.addrs(), vec![format!("ip:127.0.0.1:{port}")]);
        assert_ne!(server.id(), client.id());

        let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
        let (srv, sctx, client_id) = (server.clone(), ctx.clone(), client.id());
        tokio::spawn(async move {
            let res = async {
                let c = srv.accept(&sctx).await.map_err(|e| e.to_string())?;
                if c.alpn() != ALPN_CLIENT || c.remote_id() != client_id {
                    return Err(format!(
                        "alpn {:?} remote {}",
                        c.alpn(),
                        c.remote_id().short()
                    ));
                }
                let mut s = c.accept_stream(&sctx).await.map_err(|e| e.to_string())?;
                let m = dstore_wire::read_msg(&mut *s.recv)
                    .await
                    .map_err(|e| e.to_string())?;
                let pong = Msg {
                    typ: T_PONG,
                    epoch: m.epoch + 1,
                    ..Default::default()
                };
                let r = dstore_wire::write_msg(&mut *s.send, &pong).await;
                s.close_stream();
                r.map_err(|e| e.to_string())?;
                // Keep the connection until the client has read the reply.
                c.closed().await;
                Ok(())
            }
            .await;
            let _ = tx.send(res);
        });

        let conn = client
            .dial(&ctx, server.id(), server.addrs(), ALPN_CLIENT)
            .await
            .unwrap_or_else(|e| panic!("dial: {e}"));
        assert_eq!(conn.remote_id(), server.id());
        assert_eq!(conn.alpn(), ALPN_CLIENT);
        let m = ping(&ctx, &*conn, 41).await;
        assert_eq!((m.typ, m.epoch), (T_PONG, 42));
        let p = conn.path();
        assert!(p.direct, "expected a direct path, got {p:?}");
        conn.close();
        assert!(conn.is_closed());
        match rx.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => panic!("server: {e}"),
            Err(_) => panic!("server task dropped"),
        }
        client.close().await;
        server.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dial_returns_handshaken_connections_and_dead_addresses_fail() {
    within(async {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(40));
        let server = bind(config(&[ALPN_CLIENT])).await;
        serve_pongs(server.clone(), ctx.clone());
        let mut cfg = config(&[]);
        cfg.direct_timeout = Some(Duration::from_millis(500));
        let client = bind(cfg).await;

        // Every connection dial returns has completed its handshake: a stream exchange works at once.
        for i in 0..3 {
            let conn = client
                .dial(&ctx, server.id(), server.addrs(), ALPN_CLIENT)
                .await
                .unwrap_or_else(|e| panic!("dial {i}: {e}"));
            let m = ping(&ctx, &*conn, i).await;
            assert_eq!((m.typ, m.epoch), (T_PONG, i + 1));
            conn.close();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // A dead address fails within DirectTimeout. Rust iroh remembers the paths of a remote per
        // endpoint, so this uses an endpoint that has never reached the server.
        let mut cfg = config(&[]);
        cfg.direct_timeout = Some(Duration::from_millis(500));
        let fresh = bind(cfg).await;
        let dead = dead_loopback_addr();
        let start = Instant::now();
        let text = err_text(
            fresh
                .dial(&ctx, server.id(), vec![dead.clone()], ALPN_CLIENT)
                .await,
        );
        let took = start.elapsed();
        assert!(took < Duration::from_secs(3), "dead address took {took:?}");
        assert!(
            took >= Duration::from_millis(400),
            "dead address took {took:?}"
        );
        assert_eq!(text, format!("dial ip:{dead}: context deadline exceeded"));

        // Dead and live addresses in one phase: the live one wins.
        let mut addrs = vec![dead.clone()];
        addrs.extend(server.addrs());
        let conn = fresh
            .dial(&ctx, server.id(), addrs, ALPN_CLIENT)
            .await
            .unwrap_or_else(|e| panic!("dial with a dead and a live address: {e}"));
        let m = ping(&ctx, &*conn, 9).await;
        assert_eq!((m.typ, m.epoch), (T_PONG, 10));
        conn.close();
        fresh.close().await;
        client.close().await;
        server.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn path_rtt_is_unknown_until_sampled() {
    within(async {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(30));
        let server = bind(config(&[ALPN_CLIENT])).await;
        serve_pongs(server.clone(), ctx.clone());
        let client = bind(config(&[])).await;
        let conn = client
            .dial(&ctx, server.id(), server.addrs(), ALPN_CLIENT)
            .await
            .unwrap_or_else(|e| panic!("dial: {e}"));
        let check = |when: &str, p: PathInfo| {
            assert!(p.direct, "{when}: {p:?}");
            assert!(
                p.rtt.is_zero() || p.rtt < Duration::from_millis(50),
                "{when}: a loopback connection reports an RTT of {:?}: the initial guess, not a sample",
                p.rtt
            );
        };
        check("after dial", conn.path());
        let _ = ping(&ctx, &*conn, 1).await;
        check("after one exchange", conn.path());
        conn.close();
        client.close().await;
        server.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dial_phase_errors() {
    within(async {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(40));
        let server = bind(config(&[ALPN_CLIENT])).await;
        let mut cfg = config(&[]);
        cfg.direct_timeout = Some(Duration::from_millis(300));
        let client = bind(cfg).await;
        let dead = dead_loopback_addr();

        // Go irohkey.NewEndpointID.
        let mut bad = [0u8; 32];
        bad[0] = 2;
        assert_eq!(
            err_text(client.dial(&ctx, NodeId(bad), vec![], ALPN_CLIENT).await),
            "data is not a valid public key"
        );

        // No candidate and no discovery: go-iroh Connect with a bare id.
        assert_eq!(
            err_text(client.dial(&ctx, server.id(), vec![], ALPN_CLIENT).await),
            "iroh: no reachable address for endpoint"
        );
        let unparseable = vec!["garbage".to_string(), "mem:01020304".to_string()];
        assert_eq!(
            err_text(client.dial(&ctx, server.id(), unparseable, ALPN_CLIENT).await),
            "iroh: no reachable address for endpoint"
        );
        assert_eq!(
            err_text(client.dial(&ctx, client.id(), vec![], ALPN_CLIENT).await),
            "iroh: cannot connect to self"
        );

        // Self-dial at an address.
        let own = client.addrs();
        assert_eq!(
            err_text(client.dial(&ctx, client.id(), own.clone(), ALPN_CLIENT).await),
            format!("dial {}: iroh: cannot connect to self", own[0])
        );

        // Without a relay transport a relay candidate has no dial target (go-iroh dialTargets): it fails at
        // once, whether url::Url parses it or not.
        for relay in [
            "relay:example.com",
            "relay:https://use1-1.relay.n0.iroh-canary.iroh.link./",
        ] {
            let start = Instant::now();
            assert_eq!(
                err_text(
                    client
                        .dial(&ctx, server.id(), vec![relay.to_string()], ALPN_CLIENT)
                        .await
                ),
                format!("dial {relay}: iroh: no reachable address for endpoint")
            );
            let took = start.elapsed();
            assert!(took < Duration::from_secs(1), "{relay} took {took:?}");
        }

        // Direct phase (300 ms, discarded), then relays and direct under the ctx: one line per candidate,
        // relays first. The relay lines fail at once; the direct line waits for the ctx.
        let pctx = ctx.with_timeout(Duration::from_millis(1500));
        let start = Instant::now();
        let text = err_text(
            client
                .dial(
                    &pctx,
                    server.id(),
                    vec![
                        "relay:https://euc1-1.relay.n0.iroh-canary.iroh.link./".to_string(),
                        dead.clone(),
                        "relay:example.com".to_string(),
                    ],
                    ALPN_CLIENT,
                )
                .await,
        );
        let took = start.elapsed();
        assert_eq!(
            text,
            format!(
                "dial relay:https://euc1-1.relay.n0.iroh-canary.iroh.link./: iroh: no reachable address for endpoint\n\
                 dial relay:example.com: iroh: no reachable address for endpoint\n\
                 dial ip:{dead}: context deadline exceeded"
            )
        );
        assert!(took >= Duration::from_millis(1400), "took {took:?}");

        // A cancelled ctx.
        let cctx = ctx.with_cancel();
        cctx.cancel();
        assert_eq!(
            err_text(client.dial(&cctx, server.id(), vec![dead.clone()], ALPN_CLIENT).await),
            format!("dial ip:{dead}: context canceled")
        );
        // go-iroh checks the own id before the ctx.
        assert_eq!(
            err_text(client.dial(&cctx, client.id(), own.clone(), ALPN_CLIENT).await),
            format!("dial {}: iroh: cannot connect to self", own[0])
        );

        // Accepting needs ALPNs; a closed endpoint neither accepts nor dials.
        assert_eq!(
            err_text(client.accept(&ctx).await),
            "iroh: no ALPNs configured; nothing to accept"
        );
        server.close().await;
        assert_eq!(err_text(server.accept(&ctx).await), "iroh: endpoint closed");
        assert_eq!(
            err_text(server.dial(&ctx, client.id(), client.addrs(), ALPN_CLIENT).await),
            format!("dial {}: iroh: endpoint closed", client.addrs()[0])
        );
        client.close().await;
    })
    .await;
}

/// discoverDial on an endpoint with discovery and nothing announcing: go-iroh `connectEarly`'s checks, the zero
/// id that `lookupAddr` never looks up, the 3 s mDNS lookup, and a relay candidate without a relay transport.
/// Nothing here depends on multicast coming back to local listeners.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_without_answers() {
    within(async {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(40));
        let mut cfg = config(&[]);
        cfg.discover = true;
        let client = bind(cfg).await;
        let unknown = NodeId(*generate_secret_key().public().as_bytes());

        let start = Instant::now();
        assert_eq!(
            err_text(
                client
                    .dial(&ctx, NodeId([0; 32]), vec![], ALPN_CLIENT)
                    .await
            ),
            "iroh: no reachable address for endpoint"
        );
        let took = start.elapsed();
        assert!(took < Duration::from_secs(1), "zero id took {took:?}");

        // Relays are disabled, so only mDNS answers, and it waits out its timeout.
        let start = Instant::now();
        assert_eq!(
            err_text(client.dial(&ctx, unknown, vec![], ALPN_CLIENT).await),
            "iroh: no reachable address for endpoint"
        );
        let took = start.elapsed();
        assert!(
            took >= dstore_transport_iroh::MDNS_LOOKUP_TIMEOUT && took < Duration::from_secs(5),
            "unknown id took {took:?}"
        );

        let relay = "relay:https://use1-1.relay.n0.iroh-canary.iroh.link./";
        assert_eq!(
            err_text(
                client
                    .dial(&ctx, unknown, vec![relay.to_string()], ALPN_CLIENT)
                    .await
            ),
            format!(
                "dial {relay}: iroh: no reachable address for endpoint\n\
                 discovery: iroh: no reachable address for endpoint"
            )
        );

        let start = Instant::now();
        assert_eq!(
            err_text(client.dial(&ctx, client.id(), vec![], ALPN_CLIENT).await),
            "iroh: cannot connect to self"
        );
        client.close().await;
        assert_eq!(
            err_text(client.dial(&ctx, unknown, vec![], ALPN_CLIENT).await),
            "iroh: endpoint closed"
        );
        let took = start.elapsed();
        assert!(
            took < Duration::from_secs(4),
            "self and closed dials took {took:?}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streams_fin_stop_and_connection_close() {
    within(async {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(30));
        let server = bind(config(&[ALPN_CLIENT])).await;
        let client = bind(config(&[])).await;

        let (tx, rx) = tokio::sync::oneshot::channel::<Result<Vec<String>, String>>();
        let (srv, sctx) = (server.clone(), ctx.clone());
        tokio::spawn(async move {
            let res = async {
                let mut seen = Vec::new();
                let c = srv.accept(&sctx).await.map_err(|e| e.to_string())?;

                // FIN at a frame boundary: the frame, then EOF.
                let mut s = c.accept_stream(&sctx).await.map_err(|e| e.to_string())?;
                let m = dstore_wire::read_msg(&mut *s.recv)
                    .await
                    .map_err(|e| e.to_string())?;
                seen.push(format!("frame {} {}", m.typ, m.epoch));
                match dstore_wire::read_msg(&mut *s.recv).await {
                    Ok(m) => seen.push(format!("unexpected frame {}", m.typ)),
                    Err(e) => seen.push(format!("then {e}")),
                }
                s.send
                    .write_all(b"world")
                    .await
                    .map_err(|e| e.to_string())?;
                s.close_write();

                // STOP_SENDING from the client: writes fail.
                let mut s2 = c.accept_stream(&sctx).await.map_err(|e| e.to_string())?;
                let mut one = [0u8; 1];
                s2.recv
                    .read_exact(&mut one)
                    .await
                    .map_err(|e| e.to_string())?;
                let chunk = vec![0u8; 64 << 10];
                let write_err = loop {
                    if let Err(e) = s2.send.write_all(&chunk).await {
                        break e.to_string();
                    }
                };
                seen.push(format!("write after stop: {write_err}"));

                // CONNECTION_CLOSE from the client.
                c.closed().await;
                seen.push(format!("closed {}", c.is_closed()));
                match c.accept_stream(&sctx).await {
                    Ok(_) => seen.push("stream after close".to_string()),
                    Err(_) => seen.push("accept_stream fails".to_string()),
                }
                Ok(seen)
            }
            .await;
            let _ = tx.send(res);
        });

        let conn = client
            .dial(&ctx, server.id(), server.addrs(), ALPN_CLIENT)
            .await
            .unwrap_or_else(|e| panic!("dial: {e}"));

        let mut s = conn
            .open_stream(&ctx)
            .await
            .unwrap_or_else(|e| panic!("open_stream: {e}"));
        dstore_wire::write_msg(&mut *s.send, &ping_msg(5))
            .await
            .unwrap_or_else(|e| panic!("write_msg: {e}"));
        s.close_write();
        let mut reply = Vec::new();
        s.recv
            .read_to_end(&mut reply)
            .await
            .unwrap_or_else(|e| panic!("read_to_end: {e}"));
        assert_eq!(reply, b"world");
        s.close_stream();

        let mut s2 = conn
            .open_stream(&ctx)
            .await
            .unwrap_or_else(|e| panic!("open_stream: {e}"));
        s2.send
            .write_all(b"x")
            .await
            .unwrap_or_else(|e| panic!("write: {e}"));
        s2.cancel_read(0);

        // Give the server time to see STOP_SENDING, then close the connection.
        tokio::time::sleep(Duration::from_millis(300)).await;
        conn.close();
        assert!(conn.is_closed());
        conn.closed().await;

        let seen = match rx.await {
            Ok(Ok(seen)) => seen,
            Ok(Err(e)) => panic!("server: {e}"),
            Err(_) => panic!("server task dropped"),
        };
        assert_eq!(
            seen,
            vec![
                format!("frame {T_PING} 5"),
                "then EOF".to_string(),
                "write after stop: sending stopped by peer: error 0".to_string(),
                "closed true".to_string(),
                "accept_stream fails".to_string(),
            ]
        );
        client.close().await;
        server.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_call_over_iroh() {
    within(async {
        let ctx = Ctx::background().with_timeout(Duration::from_secs(30));
        let server = bind(config(&[ALPN_CLIENT])).await;
        serve_pongs(server.clone(), ctx.clone());
        let client = bind(config(&[])).await;
        let addrs = server.addrs();
        let pool = Pool::new(client.clone(), Arc::new(move |_| addrs.clone()), 2);
        for epoch in [7u64, 8, 9] {
            let reply = pool
                .call(&ctx, server.id(), ALPN_CLIENT, &ping_msg(epoch))
                .await
                .unwrap_or_else(|e| panic!("call: {e}"));
            assert_eq!((reply.typ, reply.epoch), (T_PONG, epoch + 1));
        }
        let p = pool
            .path(server.id(), ALPN_CLIENT)
            .unwrap_or_else(|| panic!("no live connection"));
        assert!(p.direct, "{p:?}");
        pool.close();
        assert_eq!(pool.path(server.id(), ALPN_CLIENT), None);
        client.close().await;
        server.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bind_options() {
    within(async {
        // announce is node-side.
        let mut cfg = config(&[ALPN_CLIENT]);
        cfg.announce = true;
        match bind_iroh(&Ctx::background(), cfg).await {
            Ok(_) => panic!("announce accepted"),
            Err(e) => assert_eq!(
                e.to_string(),
                "transport: bind: announce is node-side and not implemented"
            ),
        }

        // An ended ctx still binds: go-iroh Bind ignores its ctx.
        let ended = Ctx::background().with_cancel();
        ended.cancel();
        let ep = bind_iroh(&ended, config(&[]))
            .await
            .unwrap_or_else(|e| panic!("bind under a cancelled ctx: {e}"));
        let port = ep.raw().bound_sockets()[0].port();
        assert_eq!(ep.addrs(), vec![format!("ip:127.0.0.1:{port}")]);
        ep.close().await;

        // With relays, bind waits for a home relay, bounded by ctx. A URL url::Url rejects gives an empty
        // relay map (DD-13), so no home relay ever connects and nothing reaches the network.
        let undialable = || {
            Some(dstore_transport_iroh::RelayChoice::Custom(
                dstore_transport::addr::GoRelayUrl("relay.example.com".to_string()),
            ))
        };
        let mut cfg = config(&[]);
        cfg.relay = undialable();
        let start = Instant::now();
        let ep = bind_iroh(&ended, cfg)
            .await
            .unwrap_or_else(|e| panic!("bind with relays under a cancelled ctx: {e}"));
        let took = start.elapsed();
        assert!(
            took < Duration::from_secs(2),
            "online wait under an ended ctx took {took:?}"
        );
        let port = ep.raw().bound_sockets()[0].port();
        assert_eq!(ep.addrs(), vec![format!("ip:127.0.0.1:{port}")]);
        ep.close().await;
        let mut cfg = config(&[]);
        cfg.relay = undialable();
        let start = Instant::now();
        let ep = bind_iroh(
            &Ctx::background().with_timeout(Duration::from_millis(400)),
            cfg,
        )
        .await
        .unwrap_or_else(|e| panic!("bind with relays: {e}"));
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(350) && took < Duration::from_secs(3),
            "online wait bounded by a 400 ms ctx took {took:?}"
        );
        ep.close().await;

        // Default sockets (IPv4 and IPv6) with loopback: the IPv4 socket's port.
        let mut cfg = config(&[]);
        cfg.bind_addr = None;
        let ep = bind(cfg).await;
        let v4 = ep
            .raw()
            .bound_sockets()
            .into_iter()
            .find(SocketAddr::is_ipv4)
            .unwrap_or_else(|| panic!("no IPv4 socket"));
        assert_eq!(ep.addrs(), vec![format!("ip:127.0.0.1:{}", v4.port())]);
        ep.close().await;

        // Advertise wins, verbatim; an empty list advertises nothing.
        let mut cfg = config(&[]);
        cfg.advertise = Some(vec![]);
        let ep = bind(cfg).await;
        assert!(ep.addrs().is_empty());
        ep.close().await;
        let mut cfg = config(&[]);
        cfg.advertise = Some(vec![
            "127.0.0.1:5".parse().expect("addr"),
            "[::1]:6".parse().expect("addr"),
            "0.0.0.0:7".parse().expect("addr"),
            "[::ffff:10.0.0.1]:8".parse().expect("addr"),
        ]);
        let ep = bind(cfg).await;
        assert_eq!(
            ep.addrs(),
            vec![
                "ip:127.0.0.1:5",
                "ip:[::1]:6",
                "ip:0.0.0.0:7",
                "ip:[::ffff:10.0.0.1]:8"
            ]
        );
        ep.close().await;

        // Neither: every interface address with the bound port, else 127.0.0.1.
        let mut cfg = config(&[]);
        cfg.loopback = false;
        let ep = bind(cfg).await;
        let port = ep.raw().bound_sockets()[0].port();
        let mut want: Vec<String> = ifaces::interface_ips()
            .into_iter()
            .map(|ip| {
                dstore_transport::addr::GoTransportAddr::Ip {
                    ip,
                    zone: None,
                    port,
                }
                .to_string()
            })
            .collect();
        if want.is_empty() {
            want.push(format!("ip:127.0.0.1:{port}"));
        }
        assert_eq!(ep.addrs(), want);
        ep.close().await;
    })
    .await;
}

#[test]
fn interface_ips_are_dialable_unicast() {
    let ips = ifaces::interface_ips();
    for (i, ip) in ips.iter().enumerate() {
        assert!(!ip.is_loopback(), "{ip}");
        assert!(!ip.is_unspecified(), "{ip}");
        assert!(!ips[..i].contains(ip), "duplicate {ip}");
        if let std::net::IpAddr::V6(v6) = ip {
            assert!(v6.segments()[0] & 0xffc0 != 0xfe80, "link-local {ip}");
            assert!(v6.to_ipv4_mapped().is_none(), "mapped {ip}");
        }
    }
}

// ---- discovery ----

fn dns_name(out: &mut Vec<u8>, name: &str) {
    for label in name.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
}

fn dns_rr(out: &mut Vec<u8>, owner: &str, typ: u16, rdata: &[u8]) {
    dns_name(out, owner);
    out.extend_from_slice(&typ.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&120u32.to_be_bytes());
    out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    out.extend_from_slice(rdata);
}

/// A go-iroh v0.2.0 announcement (`dnsmsg.go` `buildAnnouncement`, no TXT): every record in the answer
/// section, no name compression.
fn announcement(id: &[u8; 32], ip: Ipv4Addr, port: u16) -> Vec<u8> {
    let label = mdns::endpoint_label(id);
    let service = format!("_{}._udp.local", mdns::SERVICE_NAME);
    let instance = format!("{label}.{service}");
    let host = format!("{label}.local");
    let mut p = vec![0, 0, 0x84, 0x00, 0, 0, 0, 3, 0, 0, 0, 0];
    let mut ptr = Vec::new();
    dns_name(&mut ptr, &instance);
    dns_rr(&mut p, &service, 12, &ptr);
    let mut srv = vec![0, 0, 0, 0];
    srv.extend_from_slice(&port.to_be_bytes());
    dns_name(&mut srv, &host);
    dns_rr(&mut p, &instance, 33, &srv);
    dns_rr(&mut p, &host, 1, &ip.octets());
    p
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discover_by_id_over_mdns() {
    within(async {
        // A host that cannot open an mDNS listener cannot run this test.
        let probe = match mdns::MdnsResolver::start(quiet_logger()).await {
            Ok(probe) => probe,
            Err(e) => {
                eprintln!("skipping discover_by_id_over_mdns: no mdns listener: {e}");
                return;
            }
        };

        let ctx = Ctx::background().with_timeout(Duration::from_secs(40));
        let server = bind(config(&[ALPN_CLIENT])).await;
        serve_pongs(server.clone(), ctx.clone());
        let port = server.raw().bound_sockets()[0].port();
        let packet = announcement(server.id().as_bytes(), Ipv4Addr::LOCALHOST, port);
        let parsed = mdns::parse_announcement(&packet).unwrap_or_else(|| panic!("announcement"));
        assert_eq!(parsed.id, server.id().0);
        assert_eq!(parsed.addrs, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))]);

        let sock = tokio::net::UdpSocket::bind("0.0.0.0:0")
            .await
            .expect("announcer socket");
        let announcer = tokio::spawn(async move {
            loop {
                let _ = sock.send_to(&packet, "224.0.0.251:5353").await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        // A host whose multicast does not come back to local listeners cannot run the rest. The probe is a
        // separate resolver, so a failure below is this crate's discovery wiring, not the environment.
        let heard = probe
            .resolve(&ctx, server.id().0, Duration::from_secs(2))
            .await;
        probe.close();
        if heard.is_none() {
            eprintln!(
                "skipping discover_by_id_over_mdns: multicast announcements do not reach local listeners"
            );
            announcer.abort();
            server.close().await;
            return;
        }

        let mut cfg = config(&[]);
        cfg.discover = true;
        cfg.direct_timeout = Some(Duration::from_millis(300));
        let client = bind(cfg).await;

        // By id alone.
        let conn = client
            .dial(&ctx, server.id(), vec![], ALPN_CLIENT)
            .await
            .unwrap_or_else(|e| panic!("dial by id: {e}"));
        assert_eq!(conn.remote_id(), server.id());
        let m = ping(&ctx, &*conn, 7).await;
        assert_eq!((m.typ, m.epoch), (T_PONG, 8));
        conn.close();

        // Wrong addresses fall back to discovery.
        let conn = client
            .dial(&ctx, server.id(), vec!["ip:127.0.0.1:9".to_string()], ALPN_CLIENT)
            .await
            .unwrap_or_else(|e| panic!("dial with a bad address: {e}"));
        conn.close();

        // An id nobody announces: the direct error, then discovery's after the 3 s mDNS lookup.
        let unknown = NodeId(*generate_secret_key().public().as_bytes());
        let dead = dead_loopback_addr();
        let start = Instant::now();
        let text = err_text(client.dial(&ctx, unknown, vec![dead.clone()], ALPN_CLIENT).await);
        assert_eq!(
            text,
            format!(
                "dial ip:{dead}: context deadline exceeded\ndiscovery: iroh: no reachable address for endpoint"
            )
        );
        assert!(start.elapsed() >= Duration::from_secs(3), "{:?}", start.elapsed());

        // Without discovery a dial by id alone fails.
        let plain = bind(config(&[])).await;
        assert_eq!(
            err_text(plain.dial(&ctx, server.id(), vec![], ALPN_CLIENT).await),
            "iroh: no reachable address for endpoint"
        );

        announcer.abort();
        plain.close().await;
        client.close().await;
        server.close().await;
    })
    .await;
}

// ---- live interop ----

/// Dials a running Go dstore v0.1.9 node (`dstore serve --store DIR --no-relay --loopback`) through
/// `bind_iroh` and `Endpoint::dial`, does a view call through `Pool::call`, and resolves the node by id over
/// mDNS (the node announces).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live interop: needs a running Go dstore v0.1.9 node; set DSTORE_LIVE_STORE to its store directory"]
async fn live_go_node_view_call() {
    within(async {
        let store = std::env::var("DSTORE_LIVE_STORE").expect("DSTORE_LIVE_STORE");
        let seed = std::fs::read_to_string(format!("{store}/identity")).expect("identity file");
        let seed = dstore_gocompat::hex::decode_string(seed.trim().as_bytes()).expect("hex seed");
        let seed: [u8; 32] = seed.try_into().expect("32-byte seed");
        let id = NodeId(
            *dstore_transport_iroh::iroh::SecretKey::from_bytes(&seed)
                .public()
                .as_bytes(),
        );
        let port: u16 = std::fs::read_to_string(format!("{store}/port"))
            .expect("port file")
            .trim()
            .parse()
            .expect("port number");
        let addrs = vec![format!("ip:127.0.0.1:{port}")];
        let ctx = Ctx::background().with_timeout(Duration::from_secs(40));

        // The CLI client's configuration, with relays disabled.
        let client = bind_iroh(
            &ctx,
            IrohConfig {
                secret_key: generate_secret_key(),
                alpns: vec![],
                relay: None,
                advertise: None,
                bind_addr: None,
                loopback: false,
                direct_timeout: None,
                discover: true,
                announce: false,
                logger: None,
            },
        )
        .await
        .unwrap_or_else(|e| panic!("bind: {e}"));
        eprintln!("client addrs {:?}", client.addrs());

        let start = Instant::now();
        let conn = client
            .dial(&ctx, id, addrs.clone(), ALPN_CLIENT)
            .await
            .unwrap_or_else(|e| panic!("dial Go node: {e}"));
        assert_eq!(conn.remote_id(), id);
        assert_eq!(conn.alpn(), ALPN_CLIENT);
        eprintln!("dial took {:?}, path {:?}", start.elapsed(), conn.path());
        conn.close();

        let pool = Pool::new(client.clone(), Arc::new(move |_| addrs.clone()), 1);
        let reply = pool
            .call(
                &ctx,
                id,
                ALPN_CLIENT,
                &Msg {
                    typ: T_VIEW,
                    ..Default::default()
                },
            )
            .await
            .unwrap_or_else(|e| panic!("view call: {e}"));
        assert_eq!(reply.typ, T_VIEW_REPLY);
        assert!(!reply.view.is_empty());
        eprintln!(
            "view reply: incarnation {} epoch {} view {} bytes, pool path {:?}",
            reply.incarnation,
            reply.epoch,
            reply.view.len(),
            pool.path(id, ALPN_CLIENT)
        );
        pool.close();

        // The node announces over mDNS: a dial by id alone resolves it.
        let start = Instant::now();
        match client.dial(&ctx, id, vec![], ALPN_CLIENT).await {
            Ok(conn) => {
                eprintln!(
                    "dial by id took {:?}, path {:?}",
                    start.elapsed(),
                    conn.path()
                );
                let m = ping(&ctx, &*conn, 1).await;
                eprintln!("ping by id: type {} epoch {}", m.typ, m.epoch);
                conn.close();
            }
            Err(e) => panic!("dial by id over mDNS: {e}"),
        }
        client.close_bounded(Duration::from_secs(3)).await;
    })
    .await;
}
