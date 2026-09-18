//! `client/refs.go`: `RefGet`, `RefPut`, `RefDelete`, `RefList`, CAS conditions and their errors.

use amber_store_core::reference;
use dstore_wire::{
    CODE_UNKNOWN_REF, Msg, RefInfo, T_CAS_MISMATCH, T_INCOMPLETE, T_OK, T_REF, T_REF_DELETE,
    T_REF_GET, T_REF_LIST, T_REF_PUT, T_REFS,
};

use crate::{Cluster, Ctx, Error};

/// `client.Ref`.
pub struct Ref {
    pub name: String,
    pub record: Vec<u8>,
    pub version: Vec<u8>,
    pub reference: reference::Reference,
}

/// `client.Cond`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cond {
    pub expected_version: Vec<u8>,
    pub versioned: bool,
    pub expected_old: Vec<u8>,
    pub keyed: bool,
    pub force: bool,
}

impl Cond {
    /// `cond.apply`.
    pub(crate) fn apply(&self, m: &mut Msg) {
        m.force = self.force;
        if self.versioned {
            m.has_expected = true;
            m.expected_version = self.expected_version.clone();
        }
        if self.keyed {
            m.has_expected = true;
            m.expected_old = self.expected_old.clone();
        }
    }
}

/// `*client.CASMismatch`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.message())]
pub struct CasMismatch {
    pub current: Vec<u8>,
    pub record: Vec<u8>,
    pub version: Vec<u8>,
    pub has_current: bool,
}

impl CasMismatch {
    /// "cas mismatch: reference is absent" | "cas mismatch: current key <%x>".
    pub fn message(&self) -> String {
        if !self.has_current {
            return "cas mismatch: reference is absent".to_owned();
        }
        format!(
            "cas mismatch: current key {}",
            dstore_gocompat::fmt::hex_lower(&self.current)
        )
    }

    fn from_reply(resp: Msg) -> CasMismatch {
        CasMismatch {
            current: resp.current,
            record: resp.record,
            version: resp.version,
            has_current: resp.has_current,
        }
    }
}

/// `*client.Incomplete`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("incomplete: {shortfall} keys short")]
pub struct Incomplete {
    pub sample: Vec<[u8; 32]>,
    pub shortfall: i64,
}

/// `refErr`: `unknown-ref` becomes `client: unknown reference`.
fn ref_err(err: Error) -> Error {
    if err.is_code(CODE_UNKNOWN_REF) {
        Error::UnknownRef
    } else {
        err
    }
}

impl Cluster {
    /// `RefGet`: through any node; `reference.Decode` errors are returned as they are.
    pub async fn ref_get(&self, ctx: &Ctx, name: &str) -> Result<Ref, Error> {
        let mut m = Msg {
            typ: T_REF_GET,
            name: name.to_owned(),
            ..Msg::default()
        };
        let resp = self.any_node(ctx, &mut m).await.map_err(ref_err)?;
        if resp.typ != T_REF {
            return Err(Error::UnexpectedReply(resp.typ));
        }
        let r = reference::Reference::decode(&resp.record).map_err(Error::Reference)?;
        Ok(Ref {
            name: name.to_owned(),
            record: resp.record,
            version: resp.version,
            reference: r,
        })
    }

    /// `RefPut`: writes a reference record subject to cond; returns the new version.
    pub async fn ref_put(&self, ctx: &Ctx, record: &[u8], cond: &Cond) -> Result<Vec<u8>, Error> {
        let mut m = Msg {
            typ: T_REF_PUT,
            record: record.to_vec(),
            ..Msg::default()
        };
        cond.apply(&mut m);
        let resp = self.any_node(ctx, &mut m).await.map_err(ref_err)?;
        match resp.typ {
            T_OK => Ok(resp.version),
            T_CAS_MISMATCH => Err(Error::CasMismatch(CasMismatch::from_reply(resp))),
            T_INCOMPLETE => {
                // `sample, _ := wire.Keys32(resp.Keys)`: nil when any key is not 32 bytes.
                let sample = dstore_wire::keys32(&resp.keys).unwrap_or_default();
                Err(Error::Incomplete(Incomplete {
                    sample,
                    shortfall: resp.shortfall,
                }))
            }
            other => Err(Error::UnexpectedReply(other)),
        }
    }

    /// `RefDelete`: deletes a reference subject to cond.
    pub async fn ref_delete(&self, ctx: &Ctx, name: &str, cond: &Cond) -> Result<(), Error> {
        let mut m = Msg {
            typ: T_REF_DELETE,
            name: name.to_owned(),
            ..Msg::default()
        };
        cond.apply(&mut m);
        let resp = self.any_node(ctx, &mut m).await.map_err(ref_err)?;
        match resp.typ {
            T_OK => Ok(()),
            T_CAS_MISMATCH => Err(Error::CasMismatch(CasMismatch::from_reply(resp))),
            other => Err(Error::UnexpectedReply(other)),
        }
    }

    /// `RefList`: every page, each through `any_node`. Errors are not mapped through refErr.
    pub async fn ref_list(&self, ctx: &Ctx, prefix: &[u8]) -> Result<Vec<RefInfo>, Error> {
        let mut out = Vec::new();
        let mut after = Vec::new();
        loop {
            let mut m = Msg {
                typ: T_REF_LIST,
                prefix: prefix.to_vec(),
                after: std::mem::take(&mut after),
                ..Msg::default()
            };
            let resp = self.any_node(ctx, &mut m).await?;
            if resp.typ != T_REFS {
                return Err(Error::UnexpectedReply(resp.typ));
            }
            let page_empty = resp.refs.is_empty();
            out.extend(resp.refs);
            if resp.next.is_empty() || page_empty {
                break;
            }
            after = resp.next;
        }
        Ok(out)
    }
}

/// Go `reference.ValidateName` over raw argv bytes: empty, then > 1024 bytes, then "must be valid UTF-8",
/// then core-rs `validate_name`.
pub fn validate_name_bytes(name: &[u8]) -> Result<(), String> {
    if name.is_empty() {
        return Err(reference::NameError::Empty.to_string());
    }
    if name.len() > reference::MAX_NAME_LEN {
        return Err(reference::NameError::TooLong.to_string());
    }
    let s = std::str::from_utf8(name).map_err(|_| reference::NameError::NotUtf8.to_string())?;
    reference::validate_name(s).map_err(|e| e.to_string())
}

/// The same for `reference.ValidateUser` ("user must …").
pub fn validate_user_bytes(user: &[u8]) -> Result<(), String> {
    if user.is_empty() {
        return Err(reference::UserError::Empty.to_string());
    }
    if user.len() > reference::MAX_USER_LEN {
        return Err(reference::UserError::TooLong.to_string());
    }
    let s = std::str::from_utf8(user).map_err(|_| reference::UserError::NotUtf8.to_string())?;
    reference::validate_user(s).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use dstore_testkit::golden::hex;
    use dstore_wire::{CODE_BAD_REQUEST, T_ERR, err_msg};

    use super::*;
    use crate::cluster::test_node::{self, Fail, TestNet};

    /// client-core §3.2: the reference record `trees/a` → key 01..20, user alice.
    const RECORD: &str = "a4006774726565732f610158200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f200265616c696365031b17979cfe3d85cd15";

    fn frame(m: &Msg) -> String {
        hex_of(&dstore_wire::encode_frame(m).expect("frame"))
    }

    fn hex_of(b: &[u8]) -> String {
        dstore_gocompat::hex::encode(b)
    }

    /// A stamped request as `call` sends it, over the test view (cluster id 00..0f, incarnation 1,
    /// epoch 7).
    fn stamped(mut m: Msg) -> Msg {
        m.cluster_id = (0u8..16).collect();
        m.incarnation = 1;
        m.epoch = 7;
        m
    }

    fn ref_put_msg(cond: &Cond) -> Msg {
        let mut m = Msg {
            typ: T_REF_PUT,
            record: hex(RECORD),
            ..Msg::default()
        };
        cond.apply(&mut m);
        stamped(m)
    }

    // client-core §3.2 (verified hex).
    #[test]
    fn cond_apply_frames() {
        let versioned_nil = Cond {
            versioned: true,
            ..Cond::default()
        };
        assert_eq!(
            frame(&ref_put_msg(&versioned_nil)),
            format!(
                "0000005da60018250150000102030405060708090a0b0c0d0e0f0201030707583e{RECORD}11f5"
            )
        );
        let versioned = Cond {
            versioned: true,
            expected_version: vec![1, 2, 3],
            ..Cond::default()
        };
        assert_eq!(
            frame(&ref_put_msg(&versioned)),
            format!(
                "00000062a70018250150000102030405060708090a0b0c0d0e0f0201030707583e{RECORD}0e4301020311f5"
            )
        );
        let keyed = Cond {
            keyed: true,
            expected_old: (0x40u8..0x60).collect(),
            ..Cond::default()
        };
        assert_eq!(
            frame(&ref_put_msg(&keyed)),
            format!(
                "00000080a70018250150000102030405060708090a0b0c0d0e0f0201030707583e{RECORD}0f5820404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f11f5"
            )
        );
        let force = Cond {
            force: true,
            ..Cond::default()
        };
        assert_eq!(
            frame(&ref_put_msg(&force)),
            format!(
                "0000005da60018250150000102030405060708090a0b0c0d0e0f0201030707583e{RECORD}10f5"
            )
        );
        let mut del = Msg {
            typ: T_REF_DELETE,
            name: "trees/a".into(),
            ..Msg::default()
        };
        force.apply(&mut del);
        assert_eq!(
            frame(&stamped(del)),
            "00000025a60018260150000102030405060708090a0b0c0d0e0f02010307066774726565732f6110f5"
        );
        let mut del = Msg {
            typ: T_REF_DELETE,
            name: "trees/a".into(),
            ..Msg::default()
        };
        versioned.apply(&mut del);
        assert_eq!(
            frame(&stamped(del)),
            "0000002aa70018260150000102030405060708090a0b0c0d0e0f02010307066774726565732f610e4301020311f5"
        );
    }

    #[test]
    fn cas_mismatch_and_incomplete_texts() {
        let absent = CasMismatch {
            current: Vec::new(),
            record: Vec::new(),
            version: Vec::new(),
            has_current: false,
        };
        assert_eq!(absent.to_string(), "cas mismatch: reference is absent");
        let nil_current = CasMismatch {
            has_current: true,
            ..absent.clone()
        };
        assert_eq!(nil_current.to_string(), "cas mismatch: current key ");
        let current = CasMismatch {
            current: vec![1, 2, 3],
            ..nil_current
        };
        assert_eq!(current.to_string(), "cas mismatch: current key 010203");
        let inc = Incomplete {
            sample: Vec::new(),
            shortfall: 3,
        };
        assert_eq!(inc.to_string(), "incomplete: 3 keys short");
    }

    #[test]
    fn validate_bytes_order() {
        assert_eq!(
            validate_name_bytes(b""),
            Err("reference name must not be empty".to_owned())
        );
        let long = vec![0xffu8; 1025];
        assert_eq!(
            validate_name_bytes(&long),
            Err("reference name exceeds 1024 bytes".to_owned())
        );
        assert_eq!(
            validate_name_bytes(b"a\xffb"),
            Err("reference name must be valid UTF-8".to_owned())
        );
        assert_eq!(
            validate_name_bytes(b"a@b"),
            Err("reference name must not contain '@'".to_owned())
        );
        assert_eq!(
            validate_name_bytes(b"a\x01"),
            Err("reference name must not contain control characters".to_owned())
        );
        assert_eq!(validate_name_bytes(&[b'a'; 1024]), Ok(()));
        assert_eq!(validate_name_bytes("trees/é".as_bytes()), Ok(()));

        assert_eq!(
            validate_user_bytes(b""),
            Err("user must not be empty".to_owned())
        );
        assert_eq!(
            validate_user_bytes(&vec![0x01u8; 1025]),
            Err("user exceeds 1024 bytes".to_owned())
        );
        assert_eq!(
            validate_user_bytes(b"\xc3"),
            Err("user must be valid UTF-8".to_owned())
        );
        assert_eq!(
            validate_user_bytes(b"a\x7f"),
            Err("user must not contain control characters".to_owned())
        );
        assert_eq!(validate_user_bytes(b"alice@example.com"), Ok(()));
    }

    fn ok_reply(version: &[u8]) -> Msg {
        Msg {
            typ: T_OK,
            incarnation: 1,
            epoch: 7,
            version: version.to_vec(),
            ..Msg::default()
        }
    }

    #[tokio::test]
    async fn ref_get_outcomes() {
        let replies = Arc::new(std::sync::Mutex::new(vec![
            Msg {
                typ: T_REF,
                incarnation: 1,
                epoch: 7,
                record: hex(RECORD),
                version: (0x30u8..0x40).collect(),
                ..Msg::default()
            },
            err_msg(CODE_UNKNOWN_REF, "no such reference"),
            ok_reply(&[]),
            Msg {
                typ: T_REF,
                record: vec![0x80],
                ..Msg::default()
            },
        ]));
        let tn = TestNet::start(1, test_node::scripted(replies)).await;
        let c = tn.dial().await;
        let ctx = Ctx::background();

        let r = c.ref_get(&ctx, "trees/a").await.expect("ref get");
        assert_eq!(r.name, "trees/a");
        assert_eq!(r.version, (0x30u8..0x40).collect::<Vec<u8>>());
        assert_eq!(r.reference.name, "trees/a");
        assert_eq!(r.reference.user, "alice");
        assert_eq!(r.reference.created_at, 1_700_000_000_123_456_789);

        let err = c.ref_get(&ctx, "trees/a").await.fail("unknown ref");
        assert!(err.is_unknown_ref());
        assert_eq!(err.to_string(), "client: unknown reference");

        let err = c.ref_get(&ctx, "trees/a").await.fail("unexpected");
        assert_eq!(err.to_string(), "client: unexpected reply 53");

        let err = c.ref_get(&ctx, "trees/a").await.fail("decode");
        assert!(matches!(err, Error::Reference(_)), "{err:?}");

        // The request: stamped TRefGet trees/a (client-core §3.2).
        let reqs = tn.requests();
        assert_eq!(
            frame(&reqs[1].1),
            "00000023a50018240150000102030405060708090a0b0c0d0e0f02010307066774726565732f61"
        );
    }

    #[tokio::test]
    async fn ref_put_outcomes() {
        let cas = Msg {
            typ: T_CAS_MISMATCH,
            incarnation: 1,
            epoch: 7,
            record: hex(RECORD),
            version: (0x30u8..0x40).collect(),
            current: (1u8..=32).collect(),
            has_current: true,
            ..Msg::default()
        };
        let incomplete = Msg {
            typ: T_INCOMPLETE,
            keys: vec![(1u8..=32).collect()],
            shortfall: 5,
            ..Msg::default()
        };
        let bad_sample = Msg {
            typ: T_INCOMPLETE,
            keys: vec![vec![1; 32], vec![2; 31]],
            shortfall: 9,
            ..Msg::default()
        };
        let replies = Arc::new(std::sync::Mutex::new(vec![
            ok_reply(&[0x30, 0x31]),
            cas,
            incomplete,
            bad_sample,
            Msg {
                typ: T_REFS,
                ..Msg::default()
            },
            err_msg(CODE_UNKNOWN_REF, "no such reference"),
            err_msg(CODE_BAD_REQUEST, "record key"),
        ]));
        let tn = TestNet::start(1, test_node::scripted(replies)).await;
        let c = tn.dial().await;
        let ctx = Ctx::background();
        let rec = hex(RECORD);
        let cond = Cond {
            force: true,
            ..Cond::default()
        };

        assert_eq!(
            c.ref_put(&ctx, &rec, &cond).await.expect("ok"),
            [0x30, 0x31]
        );

        let err = c.ref_put(&ctx, &rec, &cond).await.fail("cas");
        let cm = err.cas_mismatch().expect("cas mismatch");
        assert!(cm.has_current);
        assert_eq!(cm.record, rec);
        assert_eq!(
            err.to_string(),
            "cas mismatch: current key 0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20"
        );

        let err = c.ref_put(&ctx, &rec, &cond).await.fail("incomplete");
        let inc = err.incomplete().expect("incomplete");
        assert_eq!(inc.shortfall, 5);
        assert_eq!(inc.sample.len(), 1);
        assert_eq!(err.to_string(), "incomplete: 5 keys short");

        let err = c.ref_put(&ctx, &rec, &cond).await.fail("bad sample");
        assert_eq!(err.incomplete().map(|i| i.sample.len()), Some(0));

        let err = c.ref_put(&ctx, &rec, &cond).await.fail("refs");
        assert_eq!(err.to_string(), "client: unexpected reply 56");
        let err = c.ref_put(&ctx, &rec, &cond).await.fail("unknown");
        assert!(matches!(err, Error::UnknownRef));
        let err = c.ref_put(&ctx, &rec, &cond).await.fail("bad request");
        assert_eq!(err.to_string(), "remote: bad-request: record key");
        assert!(err.is_code(CODE_BAD_REQUEST));
    }

    #[tokio::test]
    async fn ref_delete_outcomes() {
        let replies = Arc::new(std::sync::Mutex::new(vec![
            ok_reply(&[]),
            Msg {
                typ: T_CAS_MISMATCH,
                ..Msg::default()
            },
            Msg {
                typ: T_REF,
                ..Msg::default()
            },
            err_msg(CODE_UNKNOWN_REF, "no such reference"),
        ]));
        let tn = TestNet::start(1, test_node::scripted(replies)).await;
        let c = tn.dial().await;
        let ctx = Ctx::background();
        let cond = Cond {
            force: true,
            ..Cond::default()
        };
        c.ref_delete(&ctx, "trees/a", &cond).await.expect("ok");
        let err = c.ref_delete(&ctx, "trees/a", &cond).await.fail("cas");
        assert_eq!(err.to_string(), "cas mismatch: reference is absent");
        let err = c.ref_delete(&ctx, "trees/a", &cond).await.fail("ref");
        assert_eq!(err.to_string(), "client: unexpected reply 52");
        let err = c.ref_delete(&ctx, "trees/a", &cond).await.fail("unknown");
        assert!(err.is_unknown_ref());
    }

    fn info(name: &str) -> RefInfo {
        RefInfo {
            name: name.to_owned(),
            key: Some(vec![1; 32]),
            version: Some(vec![2; 8]),
            created_at: 5,
            user: String::new(),
        }
    }

    fn refs(names: &[&str], next: &str) -> Msg {
        Msg {
            typ: T_REFS,
            incarnation: 1,
            epoch: 7,
            refs: names.iter().map(|n| info(n)).collect(),
            next: next.as_bytes().to_vec(),
            ..Msg::default()
        }
    }

    #[tokio::test]
    async fn ref_list_paging() {
        let replies = Arc::new(std::sync::Mutex::new(vec![
            // Listing 1: two pages, the second ends without Next.
            refs(&["trees/a", "trees/b"], "trees/b"),
            refs(&["trees/c"], ""),
            // Listing 2: a page with Next but no refs ends the listing.
            refs(&["x"], "x"),
            refs(&[], "y"),
            // Listing 3: unknown-ref is not mapped; unexpected type.
            err_msg(CODE_UNKNOWN_REF, "no such reference"),
            ok_reply(&[]),
        ]));
        let tn = TestNet::start(1, test_node::scripted(replies)).await;
        let c = tn.dial().await;
        let ctx = Ctx::background();

        let got = c.ref_list(&ctx, b"trees/").await.expect("listing 1");
        let names: Vec<&str> = got.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["trees/a", "trees/b", "trees/c"]);

        let got = c.ref_list(&ctx, b"").await.expect("listing 2");
        assert_eq!(got.len(), 1);

        let err = c.ref_list(&ctx, b"").await.fail("remote");
        assert!(!err.is_unknown_ref());
        assert_eq!(err.to_string(), "remote: unknown-ref: no such reference");
        let err = c.ref_list(&ctx, b"").await.fail("unexpected");
        assert_eq!(err.to_string(), "client: unexpected reply 53");

        // client-core §3.2 request frames: the first page has no After, the next carries Next.
        let reqs: Vec<Msg> = tn.requests().into_iter().map(|(_, m)| m).collect();
        assert_eq!(
            frame(&reqs[1]),
            "00000022a50018270150000102030405060708090a0b0c0d0e0f02010307124674726565732f"
        );
        assert_eq!(
            frame(&reqs[2]),
            "0000002ba60018270150000102030405060708090a0b0c0d0e0f02010307124674726565732f134774726565732f62"
        );
        assert_eq!(
            frame(&reqs[3]),
            "0000001aa40018270150000102030405060708090a0b0c0d0e0f02010307"
        );
        assert_eq!(reqs[4].after, b"x");
        assert!(
            reqs.iter()
                .skip(1)
                .all(|m| m.typ == T_REF_LIST || m.typ == T_ERR)
        );
    }
}
