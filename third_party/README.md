# Third-party code

## `iroh-1.2.0`: Rust iroh 1.2.0 with two changes

**What it is.** The crates.io package `iroh` 1.2.0 (upstream `https://github.com/n0-computer/iroh`), unpacked as
cargo unpacks it. The index checksum is `b2f8d1cfffc83efe39a1031aab423ce09cb8048071baba550508931e9a81ce46`.
Two things differ from the package:

- `.cargo-ok` is left out;
- `src/socket/remote_map/remote_state.rs` carries patch 1 below;
- `src/socket/transports/relay/actor.rs` carries patch 2 below.

**Licence.** The package's licence is `MIT OR Apache-2.0`. The package ships only `LICENSE-BSD3`, which covers
the code derived from tailscale. It is redistributed here under Apache-2.0. The Apache License 2.0 text was
added as `iroh-1.2.0/LICENSE-APACHE`; the package itself has none.

**How it is used.** The root `Cargo.toml` has `[patch.crates-io] iroh = { path = "third_party/iroh-1.2.0" }`
and `exclude = ["third_party"]`. The crate is therefore a dependency, not a workspace member: `cargo fmt`,
`cargo clippy --workspace` and `cargo test --workspace` leave it alone. `flake.nix` adds `./third_party` to the
package source. The version stays 1.2.0 (PORTING.md §0, §5.12). The features (`tls-ring`,
`fast-apple-datapath`) and every other iroh crate are unchanged.

**Patch 1: no NAT traversal round while a direct path is selected.**

```diff
@@ fn handle_msg_add_connection
-        self.trigger_holepunching();
+        // dstore-client-rs patch: select first, so that a connection made over a direct path
+        // does not start a NAT traversal round (go-iroh's rule: a selected direct path has
+        // nothing to upgrade toward).
         self.select_path();
+        self.trigger_holepunching();
         tx.send(path_state_receiver).ok();
@@ fn trigger_holepunching
             trace!("not holepunching: no connections");
             return;
         }
+        // dstore-client-rs patch: no NAT traversal while a direct path is selected.
+        if self
+            .state
+            .selected_path
+            .as_ref()
+            .is_some_and(|p| p.is_ip())
+        {
+            trace!("not holepunching: a direct path is selected");
+            return;
+        }
```

**Why.** On every new connection Rust iroh 1.2.0 starts a QUIC NAT traversal round. It sends REACH_OUT frames
with the host's interface addresses, and the node probes each one. A go-iroh v0.2.0 node has two bugs that this
round can trigger. Both close the connection:

- **Key updates.** Its key-update bookkeeping is not per path, so a PATH_ACK on path 0 after its first key
  update ends in `KEY_UPDATE_ERROR`.
- **Retired connection IDs.** It reuses a connection ID it has already retired, and noq answers with a
  stateless reset.

In the live interop suite about 1 Rust connection in 180 was killed this way. A request in flight on such a
connection fails at once, or waits out the 60 s idle timeout (interop D9, C1, B10).

A go-iroh client makes no such round while a direct path is selected ("Direct selected: nothing to upgrade
toward"). With the patch, neither does Rust. Connections made over a relay still holepunch, as they do in
go-iroh. Iroh 1.2.0's public API has no switch for this:
- the NAT traversal candidates are every interface address of a `0.0.0.0` bind;
- the transport config refuses fewer than 8 NAT traversal addresses and fewer than 9 multipath paths;
- `addr_filter` only filters what address lookup publishes;
- the noq `EndpointConfig` is built internally;
- `Connection` has no NAT traversal controls.

`port-notes/impl-interop-fixes.md` (interop-fix-1 and the completeness section) has the diagnosis, the traces
and the suite runs with and without the patch.

**Covered by.** `tests/iroh_loopback.rs` `live_go_node_stays_direct_behind_a_slow_path` and
`live_go_node_pool_stays_direct_behind_a_slow_path` (interop E3): the Go node is dialled through a UDP relay
that slows the dialled path. The connection must stay on that path, and every request must be answered.

**Checking the copy.**
`diff -r ~/.cargo/registry/src/index.crates.io-*/iroh-1.2.0 third_party/iroh-1.2.0` shows exactly the two patches,
`LICENSE-APACHE` and `.cargo-ok`.

**Patch 2: a bootstrap home relay at bind.**

```diff
@@ impl RelayActor, fn run
         let mut datagram_send_fut = std::pin::pin!(MaybeFuture::None);
 
+        // dstore-client-rs patch: a bootstrap home relay, the first of the relay map, so relay
+        // connectivity starts before the first net_report finishes (go-iroh's rule). A net_report
+        // that prefers another relay replaces it.
+        if self.config.my_relay.get().is_none()
+            && let Some(url) = self.config.relay_map.urls::<Vec<_>>().into_iter().next()
+        {
+            self.config
+                .my_relay
+                .set(url.clone(), RelayConnectionState::Connecting);
+            self.set_home_relay(url).await;
+        }
+
         loop {
```

**Why.** Stock 1.2.0 has no home relay until the first net_report names a preferred one, so
`Endpoint::online()` waits for that report. `BindIroh` waits for `online()` whenever relays are on, which
covers every CLI command. The report ends early only after at least one IPv6 QAD probe has finished,
successfully or not. A host that has an IPv6 address but no IPv6 route therefore waits the full 3 s
`PROBES_TIMEOUT` before every dial. A Tailscale ULA (`fd7a:…`) is enough to cause this. go-iroh `Bind` sets
the first relay of its map as the home relay before any net_report (`iroh/endpoint.go:635-645`), and
`applyNetReport` swaps in the preferred relay later. Stock iroh already does that swap in
`on_network_change`. Against the live cluster, `dstore refs` went from 4.05 s to 1.78 s, and the time to
`connected` went from 3.2 s to 0.60 s, where Go takes 0.62 s.

One difference remains: Rust's `RelayMap` is sorted by URL, so the bootstrap relay for the Go default map is
`aps1-1`, where go-iroh picks `use1-1`. Only the relay URL a client lists before its first net_report differs.

**Covered by.** `crates/transport-iroh/src/endpoint.rs` `bind_names_a_bootstrap_home_relay`: with a relay that
never answers, a home relay is set straight after bind. Stock iroh leaves it unset.

**Removing it.** Once go-iroh fixes both bugs, or iroh gains a switch, patch 1 can go. Patch 2 can go once iroh
names a home relay before its first net_report. With both gone, delete the directory, the
`[patch.crates-io]` entry and the `exclude` line, and run `cargo update -p iroh --offline`. Then remove
`./third_party` from `flake.nix`, change the two E3 tests back to expecting the switch to a direct path, and
invert `bind_names_a_bootstrap_home_relay`.
