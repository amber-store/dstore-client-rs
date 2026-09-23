//! `client/commit.go`: a commit stands for its tree.

use amber_store_core::commit::{self, Commit};
use amber_store_core::key::{Key, Type};

/// Why [`tree_of`] failed. `Display` is Go's text.
#[derive(Debug, thiserror::Error)]
pub enum TreeOfError<E: std::error::Error + 'static> {
    /// Go `fmt.Errorf("reading commit %s: %w", k, err)`: the getter failed.
    #[error("reading commit {key}: {source}")]
    Read {
        key: Key,
        #[source]
        source: E,
    },
    /// Go `fmt.Errorf("commit %s: %w", k, err)`: the object is not a valid commit.
    #[error("commit {key}: {source}")]
    Decode {
        key: Key,
        #[source]
        source: commit::Error,
    },
    /// Go `fmt.Errorf("commit %s: %w", k, err)` over `commit.Footprint`: the commit's footprint fits no
    /// length field.
    #[error("commit {key}: {source}")]
    Footprint {
        key: Key,
        #[source]
        source: commit::Error,
    },
    /// The key's length field is not the commit's footprint: in practice a commit keyed by core v0.0.9's
    /// rule, its own bytes alone (dstore v0.1.10).
    #[error(
        "commit {key}: length field {length} is not the commit's footprint {want} (its own {own} bytes plus its trees); a commit keyed by an older rule has to be created again"
    )]
    Length {
        key: Key,
        /// The key's length field.
        length: u64,
        /// The footprint the key has to carry.
        want: u64,
        /// The commit's own serialized length.
        own: usize,
    },
}

/// `client.TreeOf`: the directory root that `k` stands for: a Commit's recorded tree, or `k` itself for any
/// other key. A reference naming a commit is a branch; everything that reads files through a reference goes
/// via this.
///
/// The commit is held to core's key rule, as core's own readers hold it: its length field is its
/// footprint, its own bytes plus its trees. One keyed by core v0.0.9's rule is refused here, before a
/// working copy builds history on a parent that no reference could ever name. A conflicted commit stands
/// for its first side.
///
/// The type comes from the key's nibble, so a reserved type stands for itself, as Go's `k.Type() !=
/// key.Commit` decides (core-rs `Key::type_` would panic).
pub fn tree_of<G, E>(k: Key, mut get: G) -> Result<Key, TreeOfError<E>>
where
    G: FnMut(Key) -> Result<Vec<u8>, E>,
    E: std::error::Error + 'static,
{
    if Type::from_u8(k.0[0] >> 4) != Some(Type::Commit) {
        return Ok(k);
    }
    let data = get(k).map_err(|source| TreeOfError::Read { key: k, source })?;
    let c = Commit::decode(&data).map_err(|source| TreeOfError::Decode { key: k, source })?;
    let want = commit::footprint(data.len() as u64, &c.trees())
        .map_err(|source| TreeOfError::Footprint { key: k, source })?;
    if k.length() != want {
        return Err(TreeOfError::Length {
            key: k,
            length: k.length(),
            want,
            own: data.len(),
        });
    }
    Ok(c.tree)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use amber_store_core::commit::Identity;
    use amber_store_core::fstree;

    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("absent")]
    struct Absent;

    // Port of TestTreeOf.
    #[test]
    fn tree_of_resolves_commits() {
        let dir = fstree::encode_dir_leaf(&[]).expect("empty tree");
        let id = Identity {
            name: "tester".into(),
            when: 1,
            ..Identity::default()
        };
        let plain = Commit {
            tree: dir.key,
            parents: Vec::new(),
            author: id.clone(),
            committer: id,
            message: String::new(),
            signature: Vec::new(),
            public_key: Vec::new(),
            change_id: Vec::new(),
            conflict_terms: Vec::new(),
            conflict_labels: Vec::new(),
        };
        let (ck, raw) = plain.object().expect("commit");
        let mut objs: HashMap<Key, Vec<u8>> = HashMap::from([(ck, raw.clone())]);

        let get = |objs: &HashMap<Key, Vec<u8>>, k: Key| objs.get(&k).cloned().ok_or(Absent);
        // A tree stands for itself, and the getter is not asked.
        let got = tree_of(dir.key, |_| -> Result<Vec<u8>, Absent> {
            panic!("a tree is not read")
        });
        assert_eq!(got.expect("tree"), dir.key);
        // A commit stands for its tree.
        assert_eq!(tree_of(ck, |k| get(&objs, k)).expect("commit"), dir.key);

        // A conflicted commit stands for its first side, and its terms count towards the footprint. The
        // terms are other directories than the tree, so that the first side is told from any other (Go's
        // TestTreeOf records the same tree three times, and would pass with the last side as well).
        let blob = fstree::encode_blob(b"x");
        let side = |name: &[u8]| {
            fstree::encode_dir_leaf(&[fstree::Entry {
                name: name.to_vec(),
                mode: 0o100644,
                content_key: blob.key.0.to_vec(),
                ..fstree::Entry::default()
            }])
            .expect("dir leaf")
            .key
        };
        let (removed, added) = (side(b"removed"), side(b"added"));
        assert!(removed != dir.key && added != dir.key && removed != added);
        let (xk, xraw) = Commit {
            conflict_terms: vec![removed, added],
            ..plain
        }
        .object()
        .expect("conflicted commit");
        assert_eq!(
            xk.length(),
            xraw.len() as u64 + dir.key.length() + removed.length() + added.length()
        );
        objs.insert(xk, xraw);
        assert_eq!(tree_of(xk, |k| get(&objs, k)).expect("conflicted"), dir.key);

        // The same bytes under a key of core v0.0.9's rule, the commit's own length: refused, and the
        // message says what to do.
        let old = Key::new(Type::Commit, raw.len() as u64, &raw);
        assert_ne!(
            old, ck,
            "the footprint must differ from the commit's own length"
        );
        objs.insert(old, raw.clone());
        let err = tree_of(old, |k| get(&objs, k)).expect_err("the older rule");
        assert_eq!(
            err.to_string(),
            format!(
                "commit {old}: length field {} is not the commit's footprint {} (its own {} bytes plus its trees); a commit keyed by an older rule has to be created again",
                raw.len(),
                ck.length(),
                raw.len()
            )
        );

        // A commit key over bytes that are no commit.
        objs.insert(ck, b"not a commit".to_vec());
        let err = tree_of(ck, |k| get(&objs, k)).expect_err("garbage");
        assert_eq!(
            err.to_string(),
            format!("commit {ck}: decoding commit: unexpected EOF")
        );

        // An absent commit must fail.
        objs.remove(&ck);
        let err = tree_of(ck, |k| get(&objs, k)).expect_err("absent");
        assert_eq!(err.to_string(), format!("reading commit {ck}: absent"));
    }

    /// A reserved type nibble stands for itself instead of panicking.
    #[test]
    fn tree_of_keeps_reserved_types() {
        let mut k = Key([0u8; 32]);
        k.0[0] = 0xf1;
        let got = tree_of(k, |_| -> Result<Vec<u8>, Absent> { Err(Absent) });
        assert_eq!(got.expect("reserved"), k);
    }
}
