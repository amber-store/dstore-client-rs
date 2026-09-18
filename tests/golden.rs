//! Golden vectors generated from the Go implementation (`tools/vectorgen`, conventions in VECTORS.md),
//! checked against the public APIs. One module per owner group of PORTING.md §6, so owners working in
//! parallel never edit the same file; each module's doc comment names the vector files it reads. A missing
//! vector file fails its test (`dstore_testkit::golden::load_json` panics).

#[path = "golden_tests/cli.rs"]
mod cli;
#[path = "golden_tests/cli_progress.rs"]
mod cli_progress;
#[path = "golden_tests/client.rs"]
mod client;
#[path = "golden_tests/client_transfer.rs"]
mod client_transfer;
#[path = "golden_tests/codec.rs"]
mod codec;
#[path = "golden_tests/gocompat_io.rs"]
mod gocompat_io;
#[path = "golden_tests/gocompat_numtime.rs"]
mod gocompat_numtime;
#[path = "golden_tests/gocompat_slog.rs"]
mod gocompat_slog;
#[path = "golden_tests/gocompat_text.rs"]
mod gocompat_text;
#[path = "golden_tests/ticket.rs"]
mod ticket;
#[path = "golden_tests/transport.rs"]
mod transport;
#[path = "golden_tests/transport_iroh.rs"]
mod transport_iroh;
#[path = "golden_tests/transport_mdns.rs"]
mod transport_mdns;
#[path = "golden_tests/udiff.rs"]
mod udiff;
#[path = "golden_tests/view.rs"]
mod view;
#[path = "golden_tests/wire.rs"]
mod wire;
#[path = "golden_tests/wire_pack.rs"]
mod wire_pack;
#[path = "golden_tests/worktree.rs"]
mod worktree;
