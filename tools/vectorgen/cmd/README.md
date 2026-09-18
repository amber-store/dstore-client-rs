# Helper programs

Programs of the vectorgen module that are not vector families (PORTING.md §3.1, §7; port-notes/verification.md
§4.2). Each lives in `cmd/<name>/` as its own `package main` and runs with `go run ./cmd/<name>`; never commit a
built binary.

| Program | Purpose |
|---|---|
| `gotables` | prints `crates/gocompat/src/tables.rs`: the go1.26.5 `strconv` isPrint/isNotPrint/isGraphic tables, `unicode.White_Space`, and every rune whose `unicode.ToLower`/`ToUpper` differs from itself |
| `goerrno` | prints `crates/gocompat/src/errno_tables.rs`: the go1.26.5 `syscall` errno texts of darwin and linux on amd64 and arm64 |
| `clisnap` | builds dstore v0.1.9 `cmd/dstore` with `go build -trimpath` into a temporary directory (deleted on exit), runs every CLI case and writes `snapshots.json` |
| `mktree` | writes a deterministic source tree for interop checks |
| `treekey` | prints the root key of a directory computed by core `ingest` without storing |
| `storecmp` | `storecmp A B ROOT`: checks that every object reachable from ROOT is present in the packstores A and B with identical record bytes (interop B2, B3, B11) |
| `holdlock` | `holdlock DIR SECONDS`: opens `DIR/.dstore/packstore` (flock), prints `locked <path>` and sleeps, for the lock-interop check D12; `examples/holdlock.rs` is the Rust twin |
