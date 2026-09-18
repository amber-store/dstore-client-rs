# Early interop probe: Rust iroh 1.2.0 → Go dstore v0.1.9 node

Run on 2026-09-18 (macOS arm64), before any crate was implemented, to de-risk the
"latest Rust iroh" decision (PORTING.md §0 override).

Setup:

- Go `dstore` built from `/Users/dragan/amber-store/dstore` HEAD `368f2c7` (v0.1.9) with
  `CGO_ENABLED=0`.
- One node: `dstore cluster init --store n1 --replicas 3 --weight 10 --no-relay --loopback`, then
  `dstore serve --store n1 --no-relay --loopback`. It listened on `ip:127.0.0.1:60998`.
- Rust probe: `iroh = "=1.2.0"` with `default-features = false`,
  `features = ["tls-ring", "fast-apple-datapath"]`, and tokio.

Probe code path, which is what `dstore-transport-iroh` builds on:

```rust
let ep = Endpoint::builder(presets::Minimal)
    .secret_key(SecretKey::generate())
    .relay_mode(RelayMode::Disabled)
    .bind().await?;
let conn = ep.connect(EndpointAddr::from_parts(id, [TransportAddr::Ip(addr)]), b"amber-dstore/1").await?;
let (mut send, mut recv) = conn.open_bi().await?;
send.write_all(&[0, 0, 0, 4, 0xa1, 0x00, 0x18, 0x20]).await?;   // Msg{Type: TView}
send.finish()?;
// read u32be length, then the payload
conn.close(0u32.into(), b"");
ep.close().await;
```

Result:

- Connected, and `remote_id()` equalled the Go node id.
- The reply frame was 149 bytes: `a4 00 18 30 02 01 03 01 0c 58 8a …`. That is a map of 4 pairs:
  `Type = 48` (TViewReply), `Incarnation = 1`, `Epoch = 1`, and `View` holding a 138-byte string.
- The Go node logged no errors. An unstamped `view` request is accepted.

Conclusion: the QUIC handshake, TLS raw-public-key authentication, ALPN negotiation, bidirectional
streams, FIN semantics and the framing all work between noq 1.3.0 / iroh 1.2.0 and go-iroh v0.2.0
over direct UDP. Relay paths, discovery (mDNS, number0 DNS) and multi-stream throughput are not
covered yet. They belong to the L6 interop suite. The probe's binaries and store were deleted.
