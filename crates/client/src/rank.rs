//! `client/rank.go` and the ranking part of `client.go:297-326`.

use std::time::Duration;

use dstore_transport::PathInfo;

use crate::NodeId;

/// `rttClass`: <5ms 0, <25ms 1, <100ms 2, else 3 (strict `<`).
pub(crate) fn rtt_class(rtt: Duration) -> i64 {
    if rtt < Duration::from_millis(5) {
        0
    } else if rtt < Duration::from_millis(25) {
        1
    } else if rtt < Duration::from_millis(100) {
        2
    } else {
        3
    }
}

/// `rankOwners`: a stable sort by (penalty, relayed, RTT class, input position). An owner without a live
/// connection counts as direct and near (class 0).
pub(crate) fn rank_owners(
    ids: &[NodeId],
    penalty: impl Fn(NodeId) -> i64,
    path: impl Fn(NodeId) -> Option<PathInfo>,
) -> Vec<NodeId> {
    let mut scored: Vec<(i64, i64, i64, usize, NodeId)> = ids
        .iter()
        .enumerate()
        .map(|(pos, &id)| {
            let pen = penalty(id);
            let (relay, class) = match path(id) {
                Some(p) => (i64::from(!p.direct), rtt_class(p.rtt)),
                None => (0, 0),
            };
            (pen, relay, class, pos, id)
        })
        .collect();
    scored.sort_by_key(|&(pen, relay, class, pos, _)| (pen, relay, class, pos));
    scored.into_iter().map(|(_, _, _, _, id)| id).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use dstore_testkit::golden::{self, decimal_i64};
    use serde::Deserialize;

    use super::*;

    fn owner_ids(n: usize) -> Vec<NodeId> {
        (0..n)
            .map(|i| {
                let mut id = [0u8; 32];
                id[0] = i as u8 + 1;
                NodeId(id)
            })
            .collect()
    }

    fn no_penalty(_: NodeId) -> i64 {
        0
    }

    fn paths_of(m: HashMap<NodeId, PathInfo>) -> impl Fn(NodeId) -> Option<PathInfo> {
        move |id| m.get(&id).copied()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn rank_owners_keeps_rank_among_unmeasured_owners() {
        let ids = owner_ids(3);
        let got = rank_owners(&ids, no_penalty, paths_of(HashMap::new()));
        assert_eq!(got, ids);
    }

    #[test]
    fn rank_owners_does_not_demote_unmeasured_owners_behind_a_measured_one() {
        let ids = owner_ids(3);
        let paths = HashMap::from([(
            ids[2],
            PathInfo {
                direct: true,
                rtt: ms(1),
            },
        )]);
        let got = rank_owners(&ids, no_penalty, paths_of(paths));
        assert_eq!(got, ids);
    }

    #[test]
    fn rank_owners_prefers_direct_over_relayed() {
        let ids = owner_ids(2);
        let paths = HashMap::from([
            (
                ids[0],
                PathInfo {
                    direct: false,
                    rtt: ms(1),
                },
            ),
            (
                ids[1],
                PathInfo {
                    direct: true,
                    rtt: ms(2),
                },
            ),
        ]);
        let got = rank_owners(&ids, no_penalty, paths_of(paths));
        assert_eq!(got[0], ids[1]);
    }

    #[test]
    fn rank_owners_prefers_unmeasured_over_relayed() {
        let ids = owner_ids(2);
        let paths = HashMap::from([(
            ids[0],
            PathInfo {
                direct: false,
                rtt: ms(1),
            },
        )]);
        let got = rank_owners(&ids, no_penalty, paths_of(paths));
        assert_eq!(got[0], ids[1]);
    }

    #[test]
    fn rank_owners_ties_near_round_trips_by_rank() {
        let ids = owner_ids(2);
        let paths = HashMap::from([
            (
                ids[0],
                PathInfo {
                    direct: true,
                    rtt: Duration::from_micros(900),
                },
            ),
            (
                ids[1],
                PathInfo {
                    direct: true,
                    rtt: Duration::from_micros(400),
                },
            ),
        ]);
        let got = rank_owners(&ids, no_penalty, paths_of(paths));
        assert_eq!(got, ids);
    }

    #[test]
    fn rank_owners_prefers_a_much_nearer_owner() {
        let ids = owner_ids(2);
        let paths = HashMap::from([
            (
                ids[0],
                PathInfo {
                    direct: true,
                    rtt: ms(60),
                },
            ),
            (
                ids[1],
                PathInfo {
                    direct: true,
                    rtt: ms(2),
                },
            ),
        ]);
        let got = rank_owners(&ids, no_penalty, paths_of(paths));
        assert_eq!(got[0], ids[1]);
    }

    #[test]
    fn rank_owners_puts_penalised_owners_last() {
        let ids = owner_ids(3);
        let first = ids[0];
        let pen = move |id: NodeId| if id == first { 2 } else { 0 };
        let got = rank_owners(&ids, pen, paths_of(HashMap::new()));
        assert_eq!(got, vec![ids[1], ids[2], ids[0]]);
    }

    #[derive(Deserialize)]
    struct RttClassCase {
        #[serde(deserialize_with = "decimal_i64")]
        rtt_ns: i64,
        class: i64,
    }

    #[derive(Deserialize)]
    struct PathJson {
        direct: bool,
        #[serde(deserialize_with = "decimal_i64")]
        rtt_ns: i64,
    }

    #[derive(Deserialize)]
    struct OwnerJson {
        id: String,
        penalty: i64,
        path: Option<PathJson>,
    }

    #[derive(Deserialize)]
    struct RankCase {
        name: String,
        owners: Vec<OwnerJson>,
        want: Vec<usize>,
    }

    #[derive(Deserialize)]
    struct RankFile {
        rtt_class: Vec<RttClassCase>,
        rank_owners: Vec<RankCase>,
    }

    fn nanos(ns: i64) -> Duration {
        match u64::try_from(ns) {
            Ok(n) => Duration::from_nanos(n),
            Err(_) => panic!("negative rtt {ns} in a vector"),
        }
    }

    fn node_id(hex: &str) -> NodeId {
        NodeId::from_slice_lossy(&golden::hex(hex))
    }

    #[test]
    fn golden_rtt_class() {
        let f: RankFile = golden::load_json("client/rank.json");
        assert!(!f.rtt_class.is_empty());
        for c in &f.rtt_class {
            assert_eq!(rtt_class(nanos(c.rtt_ns)), c.class, "rtt {} ns", c.rtt_ns);
        }
    }

    #[test]
    fn golden_rank_owners() {
        let f: RankFile = golden::load_json("client/rank.json");
        assert!(f.rank_owners.len() >= 7);
        for c in &f.rank_owners {
            let ids: Vec<NodeId> = c.owners.iter().map(|o| node_id(&o.id)).collect();
            let penalties: HashMap<NodeId, i64> = c
                .owners
                .iter()
                .map(|o| (node_id(&o.id), o.penalty))
                .collect();
            let paths: HashMap<NodeId, PathInfo> = c
                .owners
                .iter()
                .filter_map(|o| {
                    o.path.as_ref().map(|p| {
                        (
                            node_id(&o.id),
                            PathInfo {
                                direct: p.direct,
                                rtt: nanos(p.rtt_ns),
                            },
                        )
                    })
                })
                .collect();
            let got = rank_owners(
                &ids,
                |id| penalties.get(&id).copied().unwrap_or_default(),
                paths_of(paths),
            );
            let want: Vec<NodeId> = c.want.iter().map(|&i| ids[i]).collect();
            assert_eq!(got, want, "{}", c.name);
        }
    }
}
