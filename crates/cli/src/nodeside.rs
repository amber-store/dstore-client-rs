//! Node-side commands (PORTING.md §2.2): identical definitions and pre-store validation, then fixed
//! errors.

use dstore_gocli::CliError;
use dstore_gocompat::os::read_file;
use dstore_gocompat::path::join;
use dstore_ticket::Ticket;

/// `<cmd> is a node-side command and dstore-client-rs does not implement the dstore node; use the Go
/// dstore binary (github.com/amber-store/dstore v0.1.10)`, where `<cmd>` is `serve`, `cluster init` or
/// `node join`.
pub fn node_side_error(cmd: &str) -> CliError {
    CliError::Msg(format!(
        "{cmd} is a node-side command and dstore-client-rs does not implement the dstore node; use the Go dstore binary (github.com/amber-store/dstore v0.1.10)"
    ))
}

pub const STORE_TICKET_UNSUPPORTED: &str = "deriving a ticket from --store needs the node's Pebble meta store, which dstore-client-rs does not implement; pass --ticket or $DSTORE_TICKET, or use the Go dstore binary";

pub const CATALOG_RESTORE_UNSUPPORTED: &str = "catalog restore writes through the node's paxos acceptor, which dstore-client-rs does not implement; use the Go dstore binary";

/// §2.2 B: `localTicket` (`main.go:394-405`) up to `node.OpenOffline`'s first step (`node/node.go:776-780`),
/// `os.ReadFile(filepath.Join(dir, "identity"))`, then `STORE_TICKET_UNSUPPORTED`.
///
/// A failed read renders Go's `*fs.PathError`: op `open` when the open fails
/// (`node: no identity in nostore: open nostore/identity: no such file or directory`) and `read` when the
/// read fails (`node: no identity in dirident: read dirident/identity: is a directory`). PORTING.md §2.2
/// B.1 always writes `open`; the Go snapshots decide (port-notes/impl-cli-app.md).
pub fn local_ticket(dir: &[u8]) -> Result<Ticket, CliError> {
    let identity = join(&[dir, &b"identity"[..]]);
    if let Err(e) = read_file(&identity) {
        return Err(CliError::Msg(format!(
            "node: no identity in {}: {e}",
            String::from_utf8_lossy(dir)
        )));
    }
    Err(CliError::Msg(STORE_TICKET_UNSUPPORTED.to_string()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn text(e: CliError) -> String {
        match e {
            CliError::Msg(m) => m,
            CliError::Exit { msg, code } => panic!("unexpected exit error {code}: {msg}"),
        }
    }

    /// A scratch directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir = std::env::temp_dir()
                .join(format!("dstore-cli-nodeside-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create scratch dir");
            Scratch(dir)
        }

        fn bytes(&self) -> Vec<u8> {
            dstore_gocompat::path::from_path(&self.0)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn node_side_texts() {
        assert_eq!(
            text(node_side_error("cluster init")),
            "cluster init is a node-side command and dstore-client-rs does not implement the dstore node; use the Go dstore binary (github.com/amber-store/dstore v0.1.10)"
        );
    }

    #[test]
    fn local_ticket_missing_identity() {
        let scratch = Scratch::new("missing");
        let dir = scratch.bytes();
        let mut nostore = dir.clone();
        nostore.extend_from_slice(b"/nostore");
        let got = text(local_ticket(&nostore).expect_err("no identity"));
        let d = String::from_utf8_lossy(&nostore);
        assert_eq!(
            got,
            format!("node: no identity in {d}: open {d}/identity: no such file or directory")
        );
    }

    /// `filepath.Join` cleans the path; the message keeps the directory as given.
    #[test]
    fn local_ticket_cleans_the_identity_path() {
        let scratch = Scratch::new("clean");
        let mut dir = scratch.bytes();
        dir.extend_from_slice(b"/./nostore/");
        let got = text(local_ticket(&dir).expect_err("no identity"));
        let given = String::from_utf8_lossy(&dir);
        let cleaned = format!("{}/nostore/identity", scratch.0.display());
        assert_eq!(
            got,
            format!("node: no identity in {given}: open {cleaned}: no such file or directory")
        );
    }

    #[test]
    fn local_ticket_identity_is_a_directory() {
        let scratch = Scratch::new("dirident");
        std::fs::create_dir(scratch.0.join("identity")).expect("mkdir identity");
        let dir = scratch.bytes();
        let got = text(local_ticket(&dir).expect_err("directory"));
        let d = String::from_utf8_lossy(&dir);
        assert_eq!(
            got,
            format!("node: no identity in {d}: read {d}/identity: is a directory")
        );
    }

    #[test]
    fn local_ticket_with_identity_is_unsupported() {
        let scratch = Scratch::new("node");
        std::fs::write(
            scratch.0.join("identity"),
            "ed8f01dbe4140b1d89a9817b8a6f46b301dd50d0a6e8eb9ccfc9ab66fb64a712\n",
        )
        .expect("write identity");
        let got = text(local_ticket(&scratch.bytes()).expect_err("unsupported"));
        assert_eq!(got, STORE_TICKET_UNSUPPORTED);
    }
}
