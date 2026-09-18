//! CLI snapshots: runs `env!("CARGO_BIN_EXE_dstore")` over `tests/golden/cli/snapshots.json` (captured from
//! the Go binary by `tools/vectorgen/cmd/clisnap`) with a clean environment (`PATH`, a temporary `HOME`,
//! `TZ=UTC`), and compares stdout, stderr and the exit status, with `{CWD}` normalised through `pwd -P`.
