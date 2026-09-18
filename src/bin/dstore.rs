//! The `dstore` command-line interface (Go `cmd/dstore`).

fn main() {
    std::process::exit(dstore_cli::main_entry())
}
