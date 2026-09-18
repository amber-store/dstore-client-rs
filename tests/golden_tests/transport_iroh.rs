//! Golden tests of `dstore-transport-iroh` endpoint code (owner transport-iroh): `transport/ids.json`
//! (endpoint-id validation and identity strings). The relay maps of `transport/relay_urls.json`
//! (`default_map`, `custom_url_map`) are checked against the private map builder by the crate's unit tests.

use dstore_testkit::golden;
use dstore_transport::TransportError;
use dstore_transport_iroh::iroh::{PublicKey, SecretKey};
use dstore_transport_iroh::mdns;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct IdsFile {
    endpoint_ids: Vec<EndpointIdCase>,
    identities: Vec<IdentityCase>,
}

#[derive(Debug, Deserialize)]
struct EndpointIdCase {
    name: String,
    bytes: String,
    valid: bool,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdentityCase {
    name: String,
    seed: String,
    id: String,
    string: String,
    short: String,
    z32: String,
    mdns_label: String,
    no_candidates_error: String,
}

fn array32(what: &str, hex: &str) -> [u8; 32] {
    match <[u8; 32]>::try_from(golden::hex(hex).as_slice()) {
        Ok(b) => b,
        Err(_) => panic!("{what}: {hex:?} is not 32 bytes"),
    }
}

// go-iroh irohkey.NewEndpointID: `Endpoint::dial` checks ids with `iroh::PublicKey::from_bytes`, and a
// rejected id is `data is not a valid public key`.
#[test]
fn endpoint_id_validation_matches_go() {
    let f: IdsFile = golden::load_json("transport/ids.json");
    assert!(!f.endpoint_ids.is_empty());
    let mut failures = Vec::new();
    for c in &f.endpoint_ids {
        let b = array32(&c.name, &c.bytes);
        if c.valid == c.error.is_some() {
            failures.push(format!(
                "{}: vector has valid={} and error={:?}",
                c.name, c.valid, c.error
            ));
        }
        let got = PublicKey::from_bytes(&b)
            .map(|_| ())
            .map_err(|_| TransportError::InvalidKey.to_string());
        let want = match &c.error {
            None => Ok(()),
            Some(e) => Err(e.clone()),
        };
        if got != want {
            failures.push(format!("{}: got {got:?}, want {want:?}", c.name));
        }
        if dstore_ticket::is_valid_public_key(&b) != c.valid {
            failures.push(format!("{}: ticket::is_valid_public_key disagrees", c.name));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// go-iroh irohkey.NewSecretKey(seed) and the strings derived from the id.
#[test]
fn identity_strings_match_go() {
    let f: IdsFile = golden::load_json("transport/ids.json");
    assert!(!f.identities.is_empty());
    let mut failures = Vec::new();
    for c in &f.identities {
        let seed = array32(&c.name, &c.seed);
        let public = SecretKey::from_bytes(&seed).public();
        let checks = [
            ("id", dstore_gocompat::hex::encode(public.as_bytes()), &c.id),
            ("string", public.to_string(), &c.string),
            ("short", public.fmt_short().to_string(), &c.short),
            ("z32", public.to_z32(), &c.z32),
            (
                "mdns_label",
                mdns::endpoint_label(public.as_bytes()),
                &c.mdns_label,
            ),
            (
                "no_candidates_error",
                TransportError::NoCandidates(public.fmt_short().to_string()).to_string(),
                &c.no_candidates_error,
            ),
        ];
        for (field, got, want) in checks {
            if &got != want {
                failures.push(format!("{} {field}: got {got:?}, want {want:?}", c.name));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
