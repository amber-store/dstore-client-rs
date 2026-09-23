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
}

/// `client.TreeOf`: the directory root that `k` stands for: a Commit's recorded tree, or `k` itself for any
/// other key. A reference naming a commit is a branch; everything that reads files through a reference goes
/// via this.
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
        let (ck, raw) = Commit {
            tree: dir.key,
            parents: Vec::new(),
            author: id.clone(),
            committer: id,
            message: String::new(),
            signature: Vec::new(),
            public_key: Vec::new(),
        }
        .object()
        .expect("commit");
        let mut objs: HashMap<Key, Vec<u8>> = HashMap::from([(ck, raw)]);

        let get = |objs: &HashMap<Key, Vec<u8>>, k: Key| objs.get(&k).cloned().ok_or(Absent);
        // A tree stands for itself, and the getter is not asked.
        let got = tree_of(dir.key, |_| -> Result<Vec<u8>, Absent> {
            panic!("a tree is not read")
        });
        assert_eq!(got.expect("tree"), dir.key);
        // A commit stands for its tree.
        assert_eq!(tree_of(ck, |k| get(&objs, k)).expect("commit"), dir.key);

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
