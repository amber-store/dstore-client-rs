//! `worktree/merge.go` (blocking).

use std::collections::HashMap;

use crate::{Change, Kind, equivalent, is_dir};

/// `worktree.Conflict`.
#[derive(Clone, Debug)]
pub struct Conflict {
    pub path: Vec<u8>,
    pub local: Change,
    pub incoming: Change,
}

/// `worktree.Merge`: (changes to apply, conflicts), both in incoming order.
pub fn merge(local: &[Change], incoming: &[Change]) -> (Vec<Change>, Vec<Conflict>) {
    // byPath: the last local change at a path wins, as Go's map assignment.
    let mut by_path: HashMap<&[u8], &Change> = HashMap::with_capacity(local.len());
    let mut paths: Vec<&[u8]> = Vec::with_capacity(local.len());
    for c in local {
        by_path.insert(&c.path, c);
        paths.push(&c.path);
    }
    // sort.Strings: equal paths are indistinguishable, so any sort gives Go's order.
    paths.sort_unstable();

    // goneAbove: a local change that deleted or retyped an ancestor directory of p, nearest first.
    let gone_above = |p: &[u8]| -> Option<&Change> {
        let mut i = p.iter().rposition(|&b| b == b'/');
        while let Some(idx) = i
            && idx > 0
        {
            let ancestor = p.get(..idx).unwrap_or_default();
            if let Some(l) = by_path.get(ancestor)
                && (l.kind == Kind::Deleted
                    || (l.kind == Kind::TypeChanged && is_dir(l.old.as_deref())))
            {
                return Some(l);
            }
            i = ancestor.iter().rposition(|&b| b == b'/');
        }
        None
    };
    // firstBelow: a local change strictly below the directory p.
    let first_below = |p: &[u8]| -> Option<&Change> {
        let mut dir = p.to_vec();
        dir.push(b'/');
        let i = paths.partition_point(|x| *x < dir.as_slice());
        match paths.get(i) {
            Some(q) if q.starts_with(&dir) => by_path.get(q).copied(),
            _ => None,
        }
    };

    let mut apply = Vec::new();
    let mut conflicts = Vec::new();
    let conflict = |l: &Change, inc: &Change| Conflict {
        path: inc.path.clone(),
        local: l.clone(),
        incoming: inc.clone(),
    };
    for inc in incoming {
        if let Some(&l) = by_path.get(inc.path.as_slice()) {
            if l.kind == Kind::MetaChanged {
                apply.push(inc.clone());
            } else if l.kind == Kind::Deleted && inc.kind == Kind::Deleted {
                // already gone
            } else if l.kind == Kind::Deleted || inc.kind == Kind::Deleted {
                conflicts.push(conflict(l, inc));
            } else if matches!((l.new.as_deref(), inc.new.as_deref()), (Some(a), Some(b)) if equivalent(a, b))
            {
                apply.push(inc.clone());
            } else {
                // Go dereferences both New entries here; a change list without them (never produced by
                // Scan or DiffTrees) counts as a conflict instead of a crash.
                conflicts.push(conflict(l, inc));
            }
            continue;
        }
        if let Some(l) = gone_above(&inc.path) {
            if inc.kind != Kind::Deleted {
                conflicts.push(conflict(l, inc));
            }
            continue;
        }
        if inc.kind == Kind::TypeChanged
            && is_dir(inc.old.as_deref())
            && let Some(l) = first_below(&inc.path)
        {
            conflicts.push(conflict(l, inc));
            continue;
        }
        apply.push(inc.clone());
    }
    (apply, conflicts)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amber_store_core::fstree::Entry;

    use super::*;
    use crate::sys::{S_IFDIR, S_IFREG};

    fn file_entry(name: &str, ck: u8, mode: u64) -> Arc<Entry> {
        let mut k = vec![0u8; 32];
        k[1] = ck;
        Arc::new(Entry {
            name: name.as_bytes().to_vec(),
            mode: S_IFREG | mode,
            content_key: k,
            ..Default::default()
        })
    }

    fn dir_entry(name: &str) -> Arc<Entry> {
        let mut k = vec![0u8; 32];
        k[0] = 0x20;
        Arc::new(Entry {
            name: name.as_bytes().to_vec(),
            mode: S_IFDIR | 0o755,
            content_key: k,
            ..Default::default()
        })
    }

    fn ch(path: &str, kind: Kind, old: Option<&Arc<Entry>>, new: Option<&Arc<Entry>>) -> Change {
        Change {
            path: path.as_bytes().to_vec(),
            kind,
            old: old.cloned(),
            new: new.cloned(),
        }
    }

    fn paths(cs: &[Change]) -> Vec<String> {
        cs.iter()
            .map(|c| String::from_utf8_lossy(&c.path).into_owned())
            .collect()
    }

    fn conflict_paths(cs: &[Conflict]) -> Vec<String> {
        cs.iter()
            .map(|c| String::from_utf8_lossy(&c.path).into_owned())
            .collect()
    }

    // Port of TestMerge, all 13 rows.
    #[test]
    fn merge_table() {
        use Kind::*;
        let base = file_entry("f", 1, 0o644);
        let v2 = file_entry("f", 2, 0o644);
        let v3 = file_entry("f", 3, 0o644);
        let d = dir_entry("d");
        let b = Some(&base);
        /// (name, local, incoming, apply paths, conflict paths).
        type Row<'a> = (
            &'a str,
            Vec<Change>,
            Vec<Change>,
            Vec<&'a str>,
            Vec<&'a str>,
        );
        let cases: Vec<Row> = vec![
            (
                "remote only",
                vec![],
                vec![ch("a", Modified, b, Some(&v2))],
                vec!["a"],
                vec![],
            ),
            (
                "local only",
                vec![ch("a", Modified, b, Some(&v2))],
                vec![],
                vec![],
                vec![],
            ),
            (
                "both differ",
                vec![ch("a", Modified, b, Some(&v2))],
                vec![ch("a", Modified, b, Some(&v3))],
                vec![],
                vec!["a"],
            ),
            (
                "same edit twice",
                vec![ch("a", Modified, b, Some(&v2))],
                vec![ch("a", Modified, b, Some(&v2))],
                vec!["a"],
                vec![],
            ),
            (
                "local touch",
                vec![ch("a", MetaChanged, b, b)],
                vec![ch("a", Modified, b, Some(&v2))],
                vec!["a"],
                vec![],
            ),
            (
                "both delete",
                vec![ch("a", Deleted, b, None)],
                vec![ch("a", Deleted, b, None)],
                vec![],
                vec![],
            ),
            (
                "local delete, remote edit",
                vec![ch("a", Deleted, b, None)],
                vec![ch("a", Modified, b, Some(&v2))],
                vec![],
                vec!["a"],
            ),
            (
                "local edit, remote delete",
                vec![ch("a", Modified, b, Some(&v2))],
                vec![ch("a", Deleted, b, None)],
                vec![],
                vec!["a"],
            ),
            (
                "remote add under locally deleted dir",
                vec![
                    ch("d", Deleted, Some(&d), None),
                    ch("d/x", Deleted, b, None),
                ],
                vec![ch("d/y", Added, None, Some(&v2))],
                vec![],
                vec!["d/y"],
            ),
            (
                "remote delete under locally deleted dir",
                vec![
                    ch("d", Deleted, Some(&d), None),
                    ch("d/x", Deleted, b, None),
                ],
                vec![ch("d/x", Deleted, b, None)],
                vec![],
                vec![],
            ),
            (
                "remote add under locally retyped dir",
                vec![
                    ch("d", TypeChanged, Some(&d), Some(&v2)),
                    ch("d/x", Deleted, b, None),
                ],
                vec![ch("d/y", Added, None, Some(&v2))],
                vec![],
                vec!["d/y"],
            ),
            (
                "remote retypes dir with local edits below",
                vec![ch("d/x", Modified, b, Some(&v2))],
                vec![
                    ch("d", TypeChanged, Some(&d), Some(&v3)),
                    ch("d/x", Deleted, b, None),
                ],
                vec![],
                vec!["d", "d/x"],
            ),
            (
                "remote deletes dir, local adds below",
                vec![ch("d/new", Added, None, Some(&v2))],
                vec![
                    ch("d", Deleted, Some(&d), None),
                    ch("d/x", Deleted, b, None),
                ],
                vec!["d", "d/x"],
                vec![],
            ),
        ];
        assert_eq!(cases.len(), 13);
        for (name, local, incoming, want_apply, want_conflicts) in cases {
            let (apply, conflicts) = merge(&local, &incoming);
            assert_eq!(paths(&apply), want_apply, "{name}: apply");
            assert_eq!(
                conflict_paths(&conflicts),
                want_conflicts,
                "{name}: conflicts"
            );
        }
    }

    #[test]
    fn conflict_local_is_the_ancestor_or_first_below() {
        use Kind::*;
        let base = file_entry("f", 1, 0o644);
        let v2 = file_entry("f", 2, 0o644);
        let d = dir_entry("d");
        let local = vec![
            ch("d", Deleted, Some(&d), None),
            ch("e/z", Modified, Some(&base), Some(&v2)),
        ];
        let incoming = vec![
            ch("d/a/b", Added, None, Some(&v2)),
            ch("e", TypeChanged, Some(&d), Some(&v2)),
        ];
        let (apply, conflicts) = merge(&local, &incoming);
        assert!(apply.is_empty());
        assert_eq!(conflicts[0].local.path, b"d");
        assert_eq!(conflicts[1].local.path, b"e/z");
    }
}
