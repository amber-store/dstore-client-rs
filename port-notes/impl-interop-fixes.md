# Interop fixes (layer L6)

Fixes and findings from the live interop suite (`interop/check.sh`) against a 3-node Go dstore v0.1.9
cluster. One section per fixer agent; each entry says what failed, the root cause, what changed, and how it
is covered.

## interop-fix-1

### D9.b / C1.mk2 (~60 s stall) and B10 (quick failure) before "negotiation failed at a primary"

**Status: root cause found; the fix needs a patched iroh, which touches files ci-nix-docs owns (below). Not
applied.**

**Root cause: two go-iroh v0.2.0 server bugs in its multipath / QUIC NAT traversal (QNT) code, exercised by
the NAT traversal round Rust iroh 1.2.0 starts on every connection.** On each new connection the Rust client
sends REACH_OUT frames with its interface addresses (on the test Mac 10.x, a Tailscale 100.x and 192.168.x).
The node answers by probing each one with PATH_CHALLENGE packets on path 0. The client validates direct paths
to the node's `[::]:<port>` socket at those addresses (multipath paths 1..3), selects the dialled path, and
abandons the others within ~2 ms. In that first few ms the node sees a burst of ~70-100 packets. Then either:

1. **KEY_UPDATE_ERROR.** The node starts its first 1-RTT key update after 100 packets (quic-go
   `FirstKeyUpdateInterval`, `internal/qng/internal/handshake/updatable_aead.go`). Its first phase-1 packets
   can be PATH_ACKs on the client's paths 3 and 4 (packet numbers 1 and 2 of those paths). The next PATH_ACK
   for path 0 acknowledges path-0 packets 70-76, which were sent in phase 0. `SetLargestAcked` compares 76 with
   `firstSentWithCurrentKey = 1`, which is a packet number from another path's space, and the node closes the
   connection: `KEY_UPDATE_ERROR: received ACK for key phase 1, but peer didn't update keys`. The key-phase
   bookkeeping (`firstSentWithCurrentKey`, `firstRcvdWithCurrentKey`, `largestAcked`) is not per path. A client
   that receives the CONNECTION_CLOSE fails its call at once (B10's quick failure).
2. **Connection ID reused after retirement.** For its probes the node rotates through the client's path-0 CIDs
   and retires them (RETIRE_CONNECTION_ID seq 2 and 4 at 4.16 ms). It then sends a PATH_RESPONSE with seq 4
   (5.15 ms). noq has dropped the retired CID from its routing table, so it answers with a stateless reset
   (`noq_proto::endpoint: sending stateless reset`). The node closes the connection without a CONNECTION_CLOSE.
   The client's connection is still open, and a request in flight on it waits for the 60 s idle timeout.
   That is D9's 12 progress lines followed by the WARN, and C1.mk2.

Why only Rust: a go-iroh client also punches on connect (`iroh/endpoint.go` `NATTraversalRemoteAddrsReady` →
`TriggerHolepunchConn`), but it configures no stateless reset key, so it drops the reused CID instead of
resetting the connection. It also exchanges far fewer packets in the QNT phase. noq adds 1200-byte probe
retries, off-path responses and MTU probes, which is what brings the node's 100th packet into the
multi-path window.

Evidence, from two full traced suite runs (per-process iroh/noq traces from a scratch tracing wrapper around
`dstore_cli::main_entry`; every Go process with `QLOGDIR`), counting node-side connections with 20-byte initial
DCIDs (noq's; a few go-iroh ones match too):

| client iroh | connections | with REACH_OUT/PATH_ABANDON | killed by the node |
|---|---|---|---|
| 1.2.0 as pinned | 544 | 290 | 3: 2 `key_update_error` (`gc why`, `catalog restore`), 1 `stateless_reset` (`node repair`) |
| 1.2.0 + the patch below | 514 | 23 (E3's test binary, built unpatched, and Go clients) | 0 |

Both runs passed 52/0/4: the three kills hit connections without a request in flight. The earlier complete run
that failed D9 and C1.mk2 hit them with one.

**Proposed fix: iroh 1.2.0, `src/socket/remote_map/remote_state.rs`, no NAT traversal while a direct path is
selected** (the rule of go-iroh's upgrade tick: "Direct selected: nothing to upgrade toward"). With no REACH_OUT
the node never probes, no extra paths exist, and neither bug can fire. Connections made over a relay still
holepunch; `check_connections` and the address-change triggers keep working for them.

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

Validated: a traced direct connection logs "not holepunching: a direct path is selected" and sends no REACH_OUT.
The node sees only path 0, and a key update on that path works (150 pings, normal close). The full suite passes
with no connection killed (table).

There is no lever in iroh 1.2.0's public API:
- the QNT local candidates are every interface address of a `0.0.0.0` bind (`socket.rs`
  `collect_local_addresses`);
- `QuicTransportConfigBuilder` refuses `max_remote_nat_traversal_addresses` below 8 and multipath paths below
  9;
- `Builder::addr_filter` only filters address lookup publishing;
- the noq `EndpointConfig`, and with it stateless resets, is built internally (`socket.rs:1008`);
- `Connection` exposes no QNT controls.

**For the completeness agent / orchestrator (ci-nix-docs files; a decision on the iroh pin, PORTING.md §0 and
§5.12):**
- Vendor iroh 1.2.0 with the patch, e.g. `third_party/iroh-1.2.0/`. Its licence is MIT/Apache-2.0; the crates.io
  `Cargo.toml` works unchanged as a path dependency.
- Add `[patch.crates-io] iroh = { path = "third_party/iroh-1.2.0" }` to the root `Cargo.toml`, regenerate
  `Cargo.lock`, and add `./third_party` to the flake's `lib.fileset.unions`.
- Note it in README and PORTING §5.12.
- Alternatives: report both bugs to go-iroh (per-path key-update bookkeeping, CID reuse after
  RETIRE_CONNECTION_ID) and to iroh (a switch for holepunching on direct paths).
- Until then D9 / C1.mk2 / B10 stay intermittent: about 1 in 180 Rust connections is killed.

Ruled out on the way, each with a deterministic experiment against a live node:
- **Path switch.** iroh abandoning path 0 for a faster NAT traversal path is harmless: go-iroh keeps sending
  on path 0, and noq keeps accepting an abandoned path until the peer abandons it too, which go-iroh never
  does. A delaying UDP relay forced the switch on every connection: 11 of 11 runs, every request answered.
- **Key update without extra paths.** Fine.
- **Lost CONNECTION_CLOSE and reused client ports.** The CLI's closes all arrive. The lost closes seen in the
  node logs came only from test processes that drop their runtime right after `close_bounded`, plus H1/H2/H4
  processes that die by panic or SIGPIPE.
- **Backoff.** A ranking penalty only.

A drafted "sticky direct" path selector was withdrawn, since the switch is not the cause.

Coverage added:
- `tests/iroh_loopback.rs` `live_go_node_answers_after_nat_traversal` and
  `live_go_node_pool_answers_after_nat_traversal` (ignored; check E3). The node is dialled through a UDP relay
  that delays the dialled path, so NAT traversal moves the connection to a direct path, on one connection and
  on the CLI pool's 4. Every request must be answered, and the path must have moved.
- With the patch applied these tests would stay on the dialled path; flip their last assertion to
  `rtt >= DIRECT_RTT` then.
- The go-iroh kill itself is too timing-dependent for a unit test: a 200-client stress run reached the node's
  key update in only 4 connections. The suite census above is the check.

### B12: SIGINT during `store push` ends with "transport: peer recently unreachable"

Harness expectation, not a product bug (fixed in `interop/check.sh` by the harness agent). Verified against Go:
- `transport.Pool.Get` (`transport/transport.go`) stores `p.failed[k]` for every failed dial, cancelled ones
  included, and returns `transport: peer recently unreachable` for 2 s while the peer has no live connection;
- `askPrimaries` does not re-ask once `ctx.Err() != nil`;
- Rust's `Pool::get` does the same.

B12 passed in both suite runs.

### Harness

- `interop/check.sh` E3 now runs every `live_go_node_*` test. When mDNS is unavailable (B13) it skips only
  `live_go_node_view_call` (it dials by id), instead of running just `live_go_node_ping_is_stamped`.
  E3: 4 passed in both runs.

## interop-fix-2

### H5 coverage gap: the TUI never downsampled colours (Go wins; implemented, DD-6 narrowed)

**What H5 showed.** H5 compared only the `pulled` line. The typescripts showed that Go's Bubble Tea downsamples
and Rust never did:

| mode | Go | Rust before |
|---|---|---|
| `COLORTERM=truecolor TERM=xterm-256color` | 24-bit SGR | 24-bit SGR |
| `TERM=xterm-256color` | `38;5;N;48;5;N` (256 colours) | 24-bit SGR |
| `NO_COLOR=1` | bold and faint, no colour at all (`done` plain) | 24-bit SGR, green `done` |
| `TERM=dumb` | plain frames, no SGR at all | 24-bit SGR |

**Root cause.** `crates/cli/src/progress.rs` wrote `UiModel::view()` to the terminal unchanged. The colorprofile
subset of cli.md §4.4 ("see §8") was never implemented, and DD-6's "(renderer control sequences, colour
downsampling)" was read as permission to skip it. Bubble Tea v2.0.9 `Program.Run` calls
`colorprofile.Detect(os.Stderr, os.Environ())` once, and its renderer converts each cell's style with
`Profile.Convert` (ultraviolet `ConvertStyle`: TrueColor as is; ANSI256/ANSI converted; Ascii drops the colours;
NoTTY drops every style).

**Decision.** Go's observed behaviour is the reference and the difference is visible (NO_COLOR ignored, a
coloured TUI under TERM=dumb), so it is implemented, not recorded as a deviation. DD-6 now covers only the bytes.

**Fix: `crates/cli/src/progress/colorprofile.rs` (new, `pub mod colorprofile` of `progress`).**
- **Detection.** A full port of colorprofile v0.4.3 `Detect`, not just the cli.md subset:
  - `colorProfile` and `envColorProfile`: `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE` and `TTY_FORCE` with
    `strconv.ParseBool`; `TERM` unset, empty or `dumb`; the truecolor terminal names (substring match, so `st`
    matches `st-256color`); the `tmux`/`screen`/`xterm` prefixes; `WT_SESSION`; `GOOGLE_CLOUD_SHELL`;
    `COLORTERM` truecolor/24bit/yes/true (lowercased); `*256color`; `*direct`.
  - For a terminal that is not dumb, the maximum with the terminfo profile and the tmux profile:
    - Terminfo: xo/terminfo `Load` → `Open` → `Decode`. The dirs are `$TERMINFO`, `~/.terminfo`, `$TERMINFO_DIRS`,
      `/etc/terminfo`, `/lib/terminfo` and `/usr/share/terminfo`. Each dir is tried as `x/name`, then as `78/name`.
      Every `Decode` validity check is kept (4096-byte limit, magic, cap counts, lengths, the extended header), and
      the search stops at the first file found, valid or not. The profile is `TrueColor` when an extended boolean
      is named `Tc` or `RGB`, else `ANSI`.
    - tmux: `$TMUX` set, then `tmux info` (`TrueColor` for a line with `Tc` or `RGB` and `true`, else ANSI256).
  - `run_transfer` detects once, in `spawn_blocking` (it may read files and spawn tmux). The inputs are
    `stderr.is_terminal()` and the process environment.
- **Conversion.** `Profile.Convert`, and x/ansi v0.11.8 `Convert256`/`Convert16` with the `ansi256To16` table:
  - the go-colorful v1.4.1 HSLuv distance (`LuvLChWhiteRef(hSLuvD65)`, `maxChromaForLH`, `getBounds`);
  - Go's `math.Pow(x, 3)` as `x*(x*x)`.
- **Writing.** Each line of a frame is rewritten after the width cut, the way colorprofile's `Writer` does it:
  - NoTTY: `ansi.Strip`, so no escape sequence at all (the cut's reset included);
  - ANSI and ANSI256: `handleSgr` over the SGR parameters, with `ReadStyleColor` for every colour type
    (`;` and `:` forms, types 1-6);
  - Ascii: the colour parameters are dropped. A colour-only SGR becomes `ESC[m`, which looks the same because
    each styled run of a frame starts after a reset;
  - other sequences and text are left as they are.
  - `UiModel::view()` is unchanged and stays byte-identical.

**FMA.** Rust must round as Go on arm64 does, and this matters here. The gc compiler fuses `c*255 - 35` in
`to6Cube`, and the `r+g+b` of the grey average, into FMADD/FNMSUB on arm64:
- Without the fusion, 194216 of the 2^24 colours differ from Go on arm64, e.g. (0,0,155): Go 18, unfused 19.
- These are exactly the colours where Go on amd64 differs from Go on arm64. Every one of them has a channel of
  115, 155, 195 or 235.
- With `mul_add` in those two places, all 2^24 colours match Go darwin/arm64, for both `Convert256` and
  `Convert16`. This was a one-off probe: a scratch Go dump against a temporary Rust test, both deleted.
- The other FMA sites (the XYZ matrix, `xyz_to_uv`, `getBounds`, `atan2`+360, the distance sum) follow the arm64
  rules too. None of them changes a result.
- Go on amd64 gives the unfused index for those colours. That is a difference between Go builds, and it is
  recorded in DD-6 ("as Go computes them on arm64").

**Harness (`interop/lib.sh`, `interop/check.sh`, `interop/README.md`).**
- **`strip_tty` bug.** It hid Go's TERM=dumb frames ("frames go 0"). Its CSI intermediate class `[ -\/]` is
  a range from space to backslash, so after a final byte such as the `J` of `ESC [ J` it also ate the next
  letter: `ESC[Jpull trees/rs` became `ull trees/rs`. Under NO_COLOR an `ESC[1m` followed, so the bug stayed
  hidden. It now uses the ECMA-48 classes (`[0-?]*[ -/]*[@-~]`, `#` as the sed delimiter). The Go dumb transcript
  now shows its 2 frames.
- **H5 assertions.**
  - Both implementations must draw frames in every mode. The old "Bubble Tea draws nothing for TERM=dumb" was
    wrong.
  - New `h5_classes` flags which kinds of styling a transcript uses: 24-bit colours (`[34]8;2;`), 256 colours
    (`[34]8;5;`), the green `ESC[32mdone`, and the bold `ESC[1mpull `. The go and rs flags must be equal. A
    mismatch is a FAIL.

**Tests.**
- Unit tests in `progress/colorprofile.rs`:
  - colorprofile's `env_test.go` cases (off Windows), and the truecolor terminal names;
  - `detect` with injected terminfo and tmux: non-terminal, `TTY_FORCE`, `NO_COLOR`/dumb early returns,
    tmux raise, `CLICOLOR_FORCE`;
  - `tmux info` parsing;
  - terminfo `Decode` over synthetic legacy and 32-bit entries, and invalid ones (magic, truncation, 4096
    bytes, a bad extended header);
  - `Open` over letter and hex directories;
  - `Convert256`/`Convert16` values, including the arm64 boundary (0,0,155) → 18;
  - `Writer` outputs per profile from a Go probe, and SGR forms.
- Unit tests in `progress.rs`:
  - `frame_bytes_downsample_each_line_after_the_cut`;
  - `run_tui_downsamples_frames_to_the_profile` (NoTTY / Ascii / ANSI256).
- Golden: new sections of `tests/golden/cli/text.json` from `tools/vectorgen/family_cli.go`, read by
  `tests/golden_tests/cli_progress.rs`, with the schema in `tools/vectorgen/docs/vectorgen-cli.md`:
  - `color_profile` (90 cases): `colorprofile.Env` for a terminal, `Detect` over a buffer for a non-terminal;
  - `convert256` (669 cases): bar colours at widths 20/60/80, the empty run, cube-level neighbours, an xorshift
    sample;
  - `downsample` (56 cases): `colorprofile.Writer` for every profile.
  - The generator keeps only colours without a 115/155/195/235 channel (`cliArchNeutral`). Full bars go only
    through the non-converting profiles. Generated on darwin/arm64 and with `GOARCH=amd64` (Rosetta), the files
    are identical, so the CI `vectors` job (ubuntu, amd64) reproduces them. The existing sections and
    `cli/size.json` are unchanged.
  - `tools/vectorgen/go.mod`: colorprofile and x/ansi moved to the direct requirements; `go vet` and `gofmt` are
    clean.

**Remaining differences (DD-6):**
- The bytes: uv writes cell diffs and cursor moves; Rust writes whole lines per frame.
- The terminfo home directory is `$HOME` (Go: `user.Current().HomeDir`).
- colorprofile's `Writer` panics on an invalid 38/48/58 colour under ANSI/ANSI256 (`MakeColor(nil)`); Rust drops
  that attribute. Bubble Tea itself does not use `Writer`, and the frames never contain one.

**For the completeness agent (VECTORS.md belongs to ci-nix-docs):**
- Add the rows `color_profile`, `convert256` and `downsample` to VECTORS.md's `cli/text.json` table (copy them
  from `tools/vectorgen/docs/vectorgen-cli.md`).
- Change "colour downsampling" in its "Not captured" list: the policy is now captured by those vectors, and the
  live check is interop H5.

**Re-run** (`interop/check.sh H4 H5`, which pulls in B1): 3 passed, 0 failed. The styling flags
(24-bit / 256 / green done / bold) were the same for go and rs in every mode:

| mode | flags |
|---|---|
| truecolor | 1011 |
| 256 | 0111 |
| NO_COLOR | 0001 |
| dumb | 0000 |

Frames: go 2, rs 3 in every mode, TERM=dumb included. With the old binary the 256, NO_COLOR and dumb flags would
differ. Gates:
- `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked -D warnings` and `cargo check`: clean.
- `cargo test --workspace --all-targets --locked --no-fail-fast`: 27 binaries, 985 passed, 0 failed, 4 ignored.
- tools/vectorgen: `go vet ./...` and `go test ./...` pass.

A first workspace run saw 2 `cli_snapshots` failures (`fixture_builder_builds_every_fixture`, `wc_cases`).
That test and its vectors belong to ci-nix-docs and were being edited at the time. They passed on the re-run,
and nothing in this fix touches them.

## interop-fix-3

### E3: `live_go_node_pool_answers_after_nat_traversal` failed about 1 run in 5

**Status: fixed in the test, so the result no longer depends on the host's interfaces. Not a port bug: a Go
client fails the same way on the route involved. No product code changed.**

**What failed.** E3 runs the four `live_go_node_*` tests at once. The pool test failed in round 0: a call
returned `dial ip:127.0.0.1:<relay port>: context deadline exceeded`, while the pool's first connection was
already on a direct path. Reproduced here, with the unfixed binary running all four tests at once against a lone
Go v0.1.9 node: 6 failures in 30 runs, all this signature.

**Root cause: NAT traversal can select a route that cannot carry the node's handshake datagrams, on a test
host with several interfaces.**
1. The pool test's client was the CLI's endpoint (`cli_client`, bound on every interface). Its NAT traversal
   candidates were the host's interface addresses:
   - 192.168.1.223 (en0, MTU 1500);
   - 10.241.247.228 (feth3832, MTU 2800);
   - 100.79.132.116 (utun6, Tailscale, MTU 1280).

   The node probes each address. The client validates a direct path through each one, and the selector moves
   to one of them, because each is more than 5 ms better than the slow relay.
2. iroh 1.2.0 sends every datagram for a remote that has a selected path only to that path
   (`remote_state.rs` `handle_msg_send_datagram`), including the Initials of later connections. So pool
   connections 2-4 run their handshakes over the selected path.
3. The host routes its own Tailscale address through utun6: `route -n get 100.79.132.116` gives interface
   utun6, flags `HOST,LOCAL`, MTU 1280. A scratch DF probe sent from a udp4 socket and from a dual-stack socket
   (the node's shape):
   - to 100.79.132.116, every size up to 1252 bytes of UDP payload arrives; 1253 and larger fail at once with
     `sendto: message too long`;
   - to 10.241.247.228 and 127.0.0.1, every size up to 1452 arrives.
   Without DF every size arrives, fragmented.
4. go-iroh v0.2.0's QUIC (qng) causes the loss:
   - it pads its Initial and Handshake datagrams to `InitialPacketSize = 1280` (`internal/protocol/params.go`);
   - it sets DF (`sys_conn_df_darwin.go`: `IPV6_DONTFRAG` on dual-stack sockets from Darwin 24; this host is
     25.6);
   - it treats an EMSGSIZE send as sent (`send_queue.go:54` and `:107`), so nothing is logged;
   - its path probes are padded only to 1200 bytes (`packet_packer.go`), so the path validates.

   Every handshake reply over this route is therefore dropped in the node's kernel. This is what the diagnosis's
   qlog shows: 14 Initial, 3 Handshake and 1 1-RTT packets "sent" by the node, and no Handshake packet from the
   client, which never received them.
5. The dial ends at the 2 s direct phase (`DIRECT_TIMEOUT`). The test has no relay and no discovery, so that
   error is the call's result. Go's `Pool.Get` does the same: it returns an extra dial's error even while a live
   connection exists.

Concurrency only changes which path the selector ends up on; it is not the cause.

**Deterministic check, independent of NAT traversal.** A scratch UDP forwarder on 127.0.0.1 relays to the node
at a chosen host address, so the node's replies take that address's route. A fake store dir holds the node's
identity and the forwarder's port, and the Go ticket is rewritten to the forwarder's address.

| route to the node | Rust client: `live_go_node_ping_is_stamped`, loopback-bound | Go v0.1.9 client: `cluster status --no-relay --no-discovery` |
|---|---|---|
| 100.79.132.116 (utun6, MTU 1280) | 0 of 5 pass: `dial ip:127.0.0.1:<fwd>: context deadline exceeded` | exit 1 after 2 s: `client: no bootstrap node answered: dial ip:127.0.0.1:<fwd>: iroh: connect to <id>: context deadline exceeded` |
| 10.241.247.228 (feth3832, MTU 2800) | 5 of 5 pass | exit 0 |

So a Go node cannot answer a handshake over a route narrower than 1308 bytes (IPv4), for a Go client or a Rust
one. The Rust port behaves as Go does.

**Product exposure (same in Go, nothing to fix here).** A CLI whose peer's selected path is such a route gets
the 2 s direct-phase failure on the pool's later dials. An example is a node reached over a tailnet after NAT
traversal moved the first connection off a relay. The causes are go-iroh's 1280-byte handshake datagrams (an
upstream fix would be a 1252-byte `InitialPacketSize` for IPv4, or reacting to EMSGSIZE) and iroh 1.2.0 routing
new connections only to the selected path (the iroh#4280 area). Neither is in this repository. The CLI against
the interop cluster is not exposed: it dials 127.0.0.1, and the selector moves only to a path at least 5 ms
better. interop-fix-1's proposed iroh patch (no NAT traversal while a direct path is selected) would remove the
test's route to this state too, but not the exposure over a relay.

**Fix, `tests/iroh_loopback.rs`.**
- `live_go_node_pool_answers_after_nat_traversal` binds its client to 127.0.0.1 (`config(&[])`). Its only NAT
  traversal candidate is then `127.0.0.1:<port>`, so every datagram the node sends it goes to 127.0.0.1 over
  lo0, whichever path is selected. The test still dials through the slow relay, still requires every call of
  every round to be answered, and still requires the first connection to end on a direct path. Its doc comment
  says why it does not use the CLI endpoint.
- `live_go_node_answers_after_nat_traversal` keeps the CLI's endpoint: the D9 shape, with REACH_OUT carrying
  every interface address. It opens no connection after the switch, so the utun route cannot fail it this way.
- Its pings no longer panic inside `ping`. `ping_within` returns the step that failed (`open_stream: …`,
  `write_msg: …`, `read_msg: …`, `no reply within 5 s`), and the assertion prints it with the connection's
  path.

**Harness, `interop/check.sh` E3 and `interop/README.md`.**
- The comment names both NAT traversal tests and says why the pool one is loopback-bound; the README describes
  E3 the same way.
- A failure now keeps each panic's message. Rust prints `panicked at <file:line:col>:` and the message on the
  lines after it, and the old `grep 'panicked|FAILED|error' | head -n 15` dropped those lines. That is why the
  second mode's message was lost. E3 now keeps 3 lines after each `panicked at`, plus the `FAILED` and `error`
  lines, up to 30.
- The info lines include the two `nat traversal` lines, which show the paths the tests ended on.
- The four tests still run at once, with cargo's default threads. After the fix the pool test does not depend
  on which path is selected, and running together is the case it has to survive. `--test-threads=1` would only
  have made the utun choice less likely on this host, not impossible.
- `check.sh` was replaced by an atomic rename of an edited copy, so a suite already reading it would be
  unaffected.

**The second, rarer mode.** In the diagnosis, `live_go_node_answers_after_nat_traversal` panicked twice in
about 59 concurrent runs at `ping`'s `open_stream`. An `open_stream` failure on an established connection means
the connection was already closed. The likely cause is interop-fix-1's go-iroh kill (`KEY_UPDATE_ERROR`, or a
stateless reset after CID reuse), which the CLI endpoint's NAT traversal round exposes: about 1 connection in
180. That stays open until interop-fix-1's vendored iroh patch is decided. The next occurrence will report its
error text in E3's failure lines.

**Results.**
- Against a lone Go v0.1.9 node, with all four tests at once:
  - unfixed binary: 6 of 30 runs failed, every one the pool test's round-0 dial;
  - fixed binary: 40 of 40 passed, 28 to 46 rounds of 4 calls each. The second mode did not appear.
  - Most of the fixed runs had `cargo test --workspace` loading the host at the same time.
- `interop/check.sh E3` (its own 3-node cluster): **PASS**, `4 passed; 0 failed`. The info lines show both NAT
  traversal tests ending on direct paths.
- Gates:
  - `cargo fmt --all --check`, `cargo check --workspace --all-targets --locked` and
    `cargo clippy --workspace --all-targets --locked -- -D warnings`: clean.
  - `cargo test --workspace --all-targets --locked --no-fail-fast`: 27 binaries, 985 passed, 0 failed,
    4 ignored (the live tests).
- Observation, cause not established: with the loopback-bound client, the pool test's final path RTT is about
  11 ms in most runs (0.33 to 27 ms over 41 runs, the E3 re-run 10.9 ms). The CLI endpoint gave 0.4 to 1.7 ms,
  and the single-connection test 0.3 to 2.6 ms. This is not load: the E3 re-run, made on an idle host, shows it
  too. The value is below `DIRECT_RTT` (40 ms) in every run, and the relay path is at least 50 ms. If the final
  assertion ever flakes, look here first.

## completeness (L6 final)

The deferred items of the three rounds, as the completeness agent settled them.

### interop-fix-1: the iroh patch is applied (vendored iroh 1.2.0)

**Decision.** Go's observed behaviour is the reference: a go-iroh client never starts a NAT traversal round
on a direct connection, and it never loses about 1 connection in 180 to the node's bugs. So the patch is
applied rather than recorded as a deviation. The user's pin, iroh 1.2.0, stays.

- **`third_party/iroh-1.2.0`.** The crates.io package with the patch above, verbatim. `LICENSE-APACHE` was
  added, because the package ships only `LICENSE-BSD3` and the licence is `MIT OR Apache-2.0`.
  `third_party/README.md` records:
  - where the copy comes from, and its checksum;
  - the diff;
  - why the patch exists, and how to remove it.

  `diff -r` against the registry copy shows exactly the patch, `LICENSE-APACHE` and `.cargo-ok`.
- **Manifests.**
  - Root `Cargo.toml`: `[patch.crates-io] iroh = { path = "third_party/iroh-1.2.0" }`, and
    `[workspace] exclude = ["third_party"]`, so the copy is not a member that fmt, clippy or the tests would
    cover.
  - `Cargo.lock`: the `iroh` entry loses its registry `source` and `checksum`. Nothing else changes, apart from
    ci-nix-docs' two root dev-dependency lines.
  - `flake.nix`: `./third_party` is in the source fileset.
- **Docs.** PORTING.md §0, §3.1, §5.12, §8 and §10, the README section "Rust iroh 1.2.0 and the interop
  evidence", `interop/README.md` (E3) and the E3 comment in `interop/check.sh`.
- **Tests.** The two D9 tests are renamed `live_go_node_stays_direct_behind_a_slow_path` and
  `live_go_node_pool_stays_direct_behind_a_slow_path`, and their last assertion is flipped to
  `rtt >= DIRECT_RTT`, as this round proposed: the connection must stay on the dialled path while every
  request is answered. Their output lines still start with `nat traversal`, which `check.sh`'s E3 info lines
  grep for. The historical sections above keep the old names.
- **Evidence.** The census of killed connections needs the scratch tracing wrapper and was not repeated. The
  full suite run below is the check.

### interop-fix-2: VECTORS.md

- **The table.** The `color_profile`, `convert256` and `downsample` rows are in the `cli/text.json` table,
  copied from `tools/vectorgen/docs/vectorgen-cli.md`.
- **"Not captured".** Instead of "colour downsampling", it now lists the TUI's styling on a real terminal. It
  says that the policy itself is captured by those vectors, and that interop H5 compares the styling live.

### interop-fix-3

The second, rarer mode (`open_stream` failing on an established connection from the CLI endpoint) is
interop-fix-1's kill. The patch removes it: no NAT traversal round runs on a direct connection.

### Other L6 hand-offs closed here

- **`tools/vectorgen/docs/vectorgen-cli.md`.** The `symlink` row has the optional `mode` field, and the wc1
  paragraph follows the table, both as in VECTORS.md.
- **`tests/fake_cluster_transfer.rs`.** The module doc no longer claims that the root package lacks
  `futures`.
- **The CI interop job and the README's harness paragraph, checked against the real `check.sh`.**
  - The job's inputs match: `DSTORE_GO_REPO` at tag v0.1.9, and `DSTORE_RS_BIN` with `examples/holdlock`
    next to it. E3's `cargo test` goes to `$REPO/target`, which the job built.
  - The README claimed a default `DSTORE_RS_BIN`. It now describes the real lookup order and passes
    `DSTORE_RS_BIN` in its example.
- **`crates/transport-iroh/src/endpoint.rs` mDNS wiring.** All three rules were already implemented:
  - a failed `MdnsResolver::start` registers `MdnsResolver::without_listener`;
  - answers with an empty address set are skipped (go-iroh `lookupAddr`'s `found.IsEmpty()`; a relay URL
    counts as an address);
  - the endpoint's close stops the listener.

  They had no endpoint-level tests. `registered_mdns` and `Discovery::close` are now small seams, covered by
  `a_failed_mdns_start_registers_a_resolver_without_listener`, `discovery_skips_answers_without_addresses`
  and `closing_discovery_stops_the_mdns_listener`. They run without sockets, over the `#[cfg(test)]` helpers
  `MdnsResolver::with_cache` and `is_listening` in `mdns.rs`.
- **`tests/cli_wc.rs` `a_locked_working_copy_is_refused`.** 30 runs of the built test binary, 30 passed, so
  no cause to fix.

### Gates and the suite run (macOS arm64, nix develop: rustc 1.95.0, go 1.26.5)

- **Full live suite on the patched build.** `interop/check.sh` with `INTEROP_HEAVY=1 INTEROP_CHAOS=1
  INTEROP_MDNS=auto`, against `/Users/dragan/amber-store/dstore` at v0.1.9: **55 passed, 0 failed, 1
  skipped (H8)**, exit 0. D9, C1, B10 and B13 pass.
- **E3.** 4 passed. The info lines show that both D9 tests stayed on the dialled path:
  - `nat traversal (none expected): 26 pings answered, path PathInfo { direct: true, rtt: 54.431008ms }`;
  - `nat traversal through the pool (none expected): 31 rounds of 4 calls answered, path Some(PathInfo { direct: true, rtt: 54.355577ms })`.
- **Cargo gates**, with the dev shell and `--locked`:
  - `cargo fmt --all --check`: clean;
  - `cargo check --workspace --all-targets`: clean;
  - `cargo clippy --workspace --all-targets -- -D warnings`: clean. The vendored iroh builds without
    warnings;
  - `TZ=UTC cargo test --workspace --all-targets --no-fail-fast`: 27 binaries, **988 passed, 0 failed, 4
    ignored**. The ignored ones are the live Go tests, each with its reason.
- **tools/vectorgen** (`GOPROXY=off`, a scratch `GOCACHE`, deleted afterwards):
  - `go vet ./...` and `go test ./...`: ok;
  - every family regenerated twice: identical, and equal to `tests/golden`;
  - `gotables`, `goerrno` and `clisnap` regenerated: no diff against `tables.rs`, `errno_tables.rs` and
    `cli/snapshots.json`.
- **`nix flake check -L`** (aarch64-darwin): exit 0. It built `checks.fmt`, `checks.clippy`, `checks.dstore`
  and `checks.tests`, the last over the vendored iroh: 24 test binaries, 976 passed, 0 failed. The first
  attempt failed at evaluation, because
  `./third_party` was still untracked and a git flake sees only tracked files. It passed once the tree was
  staged for the commit. The check outputs were then deleted from the store.
