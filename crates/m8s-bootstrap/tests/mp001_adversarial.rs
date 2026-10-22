//! Independent public canonical verification, NOT admission/runtime proof.
//! PRIMARY-approved capture: agor://kb/document/01a160ea-273a-7685-8f4b-5c11b428bde1.
//! Exact c2190fdd6bc4f9100ac5029621622341c3e8c8e6 private-encoder TESTCOPY capture;
//! public synthetic seed [7;32], node [0x42;32], Some("synthetic-only"), None host.
//! Patch SHA f13f76d98d510e66c75f097789944c0eee2fd1634ab181c72831dd363703f620;
//! recipe SHA 07c1460a8474972dde7b8cb9744f02adeed6491e7a7b97ee207a6f82946925f3.
//! JSON constants retain exactly one trailing newline; hashes checked below.
//! ABSURD2020..2100 synthetic lifetime is NOT production issuance authorization.
//! No signer/encoder/private clock/proof constructor or host IO is used here.
#![cfg(feature = "mp001-certificates")]
use m8s_bootstrap::{
    CertificateClaims, CertificateVerifier, EndpointId, Error, NodeCertificate, Result,
    mp001::{Mp001CertificateVerifier, Mp001TrustPin},
};
use mesh_llm_identity::{
    OwnershipStatus, SignedNodeOwnership, TrustPolicy, TrustStore,
    node_certificate::{RevokedNodeCert, RevokedNodeId, RevokedOwner, TrustedOwner},
    verify_node_ownership,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error as _,
    time::{SystemTime, UNIX_EPOCH},
};
const LONG: &str = r#"{"claim":{"version":1,"cert_id":"m8s-synthetic-public-long-window-v1","owner_id":"fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889","owner_sign_public_key":"ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c","node_endpoint_id":"4242424242424242424242424242424242424242424242424242424242424242","issued_at_unix_ms":1577836800000,"expires_at_unix_ms":4102444800000,"node_label":"synthetic-only"},"signature":"688e2126eb9a0cd91ec25dffb465f9c19b3f6f065e8445be7171377a45da0ba674db0d311464c6b05971cc5a258eed9211a6caca7f7cc1d8e4ba7127c1c33907"}
"#;
const FUTURE: &str = r#"{"claim":{"version":1,"cert_id":"m8s-synthetic-public-future-v1","owner_id":"fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889","owner_sign_public_key":"ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c","node_endpoint_id":"4242424242424242424242424242424242424242424242424242424242424242","issued_at_unix_ms":4102444800000,"expires_at_unix_ms":4102531200000,"node_label":"synthetic-only"},"signature":"3c3182a278a4c9119652c9657ff98636cbe0f3d3d216bf9c7324d5579dba4322999cca59dcf972d45120b9e34878f9d9d649b3ddaeb45c717150f35dfdec6809"}
"#;
const REMAINDER: &str = r#"{"claim":{"version":1,"cert_id":"m8s-synthetic-public-millisecond-remainder-v1","owner_id":"fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889","owner_sign_public_key":"ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c","node_endpoint_id":"4242424242424242424242424242424242424242424242424242424242424242","issued_at_unix_ms":1577836800001,"expires_at_unix_ms":4102444800000,"node_label":"synthetic-only"},"signature":"86cce9c931cf6e0356bb08cd5471681eaebdc971e6a91c6f1528ac6877a924f066b9742730bbf501bc3b09e5aa509548e040777972caf3a194c170152357450c"}
"#;
const OWNER_ID: &str = "fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889";
const KEY: &str = "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c";
const OTHER_KEY: &str = "1398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca";
const SENTINEL: &str = "synthetic-opaque-input-sentinel";
fn now_ms() -> u64 {
    let now = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("requires UTC after epoch")
            .as_millis(),
    )
    .unwrap();
    assert!(
        (1_577_836_800_000..4_102_444_800_000).contains(&now),
        "fixture suite requires real UTC2020-01-01 <= now <2100-01-01: refresh reviewed vectors, NEVER skip"
    );
    now
}
fn claims() -> CertificateClaims {
    CertificateClaims {
        owner: EndpointId([1; 32]),
        node: EndpointId([0x42; 32]),
        issued_at: 1_577_836_800,
        expires_at: 4_102_444_800,
    }
}
fn pin(key: &str) -> Mp001TrustPin {
    Mp001TrustPin::new(
        EndpointId([1; 32]),
        hex::decode(key).unwrap().try_into().unwrap(),
    )
    .unwrap()
}
fn verifier(trust: TrustStore) -> Mp001CertificateVerifier {
    now_ms();
    Mp001CertificateVerifier::new(pin(KEY), trust).unwrap()
}
fn verify(v: &Mp001CertificateVerifier, text: &str) -> Result<()> {
    v.verify(&NodeCertificate(text.as_bytes().to_vec()), &claims())
}
fn rejected<T>(result: Result<T>) {
    match result {
        Err(e) => {
            assert!(matches!(e, Error::Certificate));
            assert_eq!(e.to_string(), "certificate signing or verification failed");
            assert_eq!(format!("{e:?}"), "Certificate");
            assert!(e.source().is_none());
        }
        Ok(_) => panic!("invalid input accepted"),
    }
}
fn canonical_positive(text: &str, at: u64) -> SignedNodeOwnership {
    let c: SignedNodeOwnership = serde_json::from_str(text).unwrap();
    let mut t = TrustStore::default();
    t.add_trusted_owner(c.claim.owner_id.clone(), None);
    let r = verify_node_ownership(Some(&c), &[0x42; 32], &t, TrustPolicy::Allowlist, at);
    assert!(r.verified);
    assert_eq!(r.status, OwnershipStatus::Verified);
    c
}
#[test]
fn reviewed_vectors_are_real_crypto_and_current_positive_accepts_hex_byte_equivalence() {
    for (text, digest) in [
        (
            LONG,
            "5594d0ca6b4c6af64ff3f89414ca9dc0c710d45e666f95c20cc0f19a4a2d4f88",
        ),
        (
            FUTURE,
            "a3979316e53a105f5cecfbecf7924e777bb9323a52e42a8c88113c5713c3ea45",
        ),
        (
            REMAINDER,
            "cc81f9d23007237496ef3b4b91ad7cc39c865b7f20f819045eb5af1c70ef3dc5",
        ),
    ] {
        assert_eq!(hex::encode(Sha256::digest(text.as_bytes())), digest);
    }
    canonical_positive(LONG, now_ms());
    let v = verifier(TrustStore::default());
    verify(&v, LONG).unwrap();
    let mut c: SignedNodeOwnership = serde_json::from_str(LONG).unwrap();
    c.claim.owner_sign_public_key.make_ascii_uppercase();
    c.claim.node_endpoint_id.make_ascii_uppercase();
    c.signature.make_ascii_uppercase();
    let upper = serde_json::to_string(&c).unwrap();
    canonical_positive(&upper, now_ms());
    verify(&v, &upper).unwrap();
    // Node42 contains digits only; this is NOT a letter-case node-ID proof.
    assert_eq!(c.claim.owner_id, OWNER_ID);
}
#[test]
fn exact_owner_node_signer_claims_and_signature_remain_independently_bound() {
    let v = verifier(TrustStore::default());
    verify(&v, LONG).unwrap();
    for which in 0..4 {
        let mut expected = claims();
        match which {
            0 => expected.owner = EndpointId([2; 32]),
            1 => expected.node = EndpointId([3; 32]),
            2 => expected.issued_at += 1,
            _ => expected.expires_at -= 1,
        };
        rejected(v.verify(&NodeCertificate(LONG.as_bytes().to_vec()), &expected));
    }
    let other = Mp001CertificateVerifier::new(pin(OTHER_KEY), TrustStore::default()).unwrap();
    rejected(verify(&other, LONG));
    let original: Value = serde_json::from_str(LONG).unwrap();
    for (field, replacement) in [
        ("cert_id", json!(SENTINEL)),
        ("owner_id", json!("01".repeat(32))),
        ("owner_sign_public_key", json!(OTHER_KEY)),
        ("node_endpoint_id", json!("03".repeat(32))),
        ("node_label", Value::Null),
        ("hostname_hint", json!("")),
    ] {
        let mut changed = original.clone();
        changed["claim"][field] = replacement;
        rejected(verify(&v, &serde_json::to_string(&changed).unwrap()));
    }
    let signature = original["signature"].as_str().unwrap();
    let mut flipped = signature.as_bytes().to_vec();
    flipped[0] = if flipped[0] == b'0' { b'1' } else { b'0' };
    for bad in [
        String::from_utf8(flipped).unwrap(),
        "00".repeat(64),
        "zz".repeat(64),
        "00".into(),
    ] {
        let mut c = original.clone();
        c["signature"] = json!(bad);
        rejected(verify(&v, &serde_json::to_string(&c).unwrap()));
    }
    verify(&v, LONG).unwrap();
}
#[test]
fn strict_raw_json_duplicates_unknowns_and_size_cap_do_not_change_signed_semantics() {
    let v = verifier(TrustStore::default());
    verify(&v, LONG).unwrap();
    let unknown = LONG.replacen("{", &format!("{{\"{SENTINEL}\":true,"), 1);
    // Canonical decoder ignores this unknown field; strict ADAPTER must not.
    canonical_positive(&unknown, now_ms());
    rejected(verify(&v, &unknown));
    let text = LONG.trim_end();
    let cases = vec![
        LONG.replacen(
            "\"claim\":{",
            &format!("\"claim\":{{\"{SENTINEL}\":true,"),
            1,
        ),
        LONG.replacen("\"signature\":", "\"signature\":null,\"signature\":", 1),
        LONG.replacen("\"version\":1", "\"version\":1,\"version\":1", 1),
        LONG.replacen(
            "\"node_label\":\"synthetic-only\"",
            "\"node_label\":null,\"node_label\":\"synthetic-only\"",
            1,
        ),
        LONG.replacen(
            "\"version\":1",
            "\"hostname_hint\":null,\"hostname_hint\":null,\"version\":1",
            1,
        ),
        format!("{text} {{}}"),
        LONG.replace("\"version\":1", "\"version\":true"),
        LONG.replace("\"signature\":", "\"omitted_signature\":"),
        String::new(),
    ];
    for bad in cases {
        rejected(verify(&v, &bad));
    }
    rejected(v.verify(&NodeCertificate(vec![255]), &claims()));
    let mut padded = LONG.as_bytes().to_vec();
    padded.resize(131_072, b' ');
    v.verify(&NodeCertificate(padded.clone()), &claims())
        .unwrap();
    padded.push(b' ');
    rejected(v.verify(&NodeCertificate(padded), &claims()));
    // These bound violations are rejected, but unresigned mutations alone do
    // NOT prove the exact validation stage or successful signed boundary length.
    for (field, value) in [
        ("cert_id", "x".repeat(129)),
        ("owner_id", OWNER_ID.to_uppercase()),
        ("node_label", "λ".repeat(129)),
        ("hostname_hint", "x".repeat(257)),
    ] {
        let mut c: Value = serde_json::from_str(LONG).unwrap();
        c["claim"][field] = json!(value);
        rejected(verify(&v, &serde_json::to_string(&c).unwrap()));
    }
    verify(&v, LONG).unwrap();
}
#[test]
fn actual_clock_and_exact_checked_seconds_reject_future_remainder_and_overflow() {
    let at = now_ms();
    let v = verifier(TrustStore::default());
    verify(&v, LONG).unwrap();
    canonical_positive(FUTURE, at); // Existing BASE intentionally permits future issuance.
    let future = CertificateClaims {
        issued_at: 4_102_444_800,
        expires_at: 4_102_531_200,
        ..claims()
    };
    rejected(v.verify(&NodeCertificate(FUTURE.as_bytes().to_vec()), &future));
    canonical_positive(REMAINDER, at);
    rejected(verify(&v, REMAINDER));
    for (issued, expires) in [
        (-1, 4_102_444_800),
        (1_577_836_800, -1),
        (0, i64::MAX),
        (i64::MAX, i64::MAX),
        (4, 4),
        (5, 4),
    ] {
        let expected = CertificateClaims {
            issued_at: issued,
            expires_at: expires,
            ..claims()
        };
        rejected(v.verify(&NodeCertificate(LONG.as_bytes().to_vec()), &expected));
    }
    // No injected public clock or racy sleep-to-exact-expiry assertion.
}
#[test]
fn explicit_pin_is_authority_but_never_overrides_any_revocation_or_off_policy() {
    for policy in [
        TrustPolicy::Off,
        TrustPolicy::PreferOwned,
        TrustPolicy::RequireOwned,
        TrustPolicy::Allowlist,
    ] {
        let base = TrustStore {
            policy,
            ..TrustStore::default()
        };
        verify(&verifier(base.clone()), LONG).unwrap(); // Empty input allowlist is valid: pin adds authority.
        for kind in 0..3 {
            let mut t = base.clone();
            match kind {
                0 => t.revoke_owner(OWNER_ID.into(), None),
                1 => t.revoke_node_cert("m8s-synthetic-public-long-window-v1".into(), None),
                _ => t.revoke_node_id("42".repeat(32), None),
            };
            rejected(verify(&verifier(t), LONG));
        }
    }
}
#[test]
fn constructors_bound_every_snapshot_list_and_retained_metadata_without_unique_id_rule() {
    let other = "01".repeat(32);
    let mut full = TrustStore {
        trusted_owners: vec![
            TrustedOwner {
                owner_id: other.clone(),
                label: None
            };
            256
        ],
        ..TrustStore::default()
    };
    rejected(Mp001CertificateVerifier::new(pin(KEY), full.clone())); // No room to insert pin.
    full.trusted_owners[0].owner_id = OWNER_ID.into();
    verify(&verifier(full.clone()), LONG).unwrap();
    full.trusted_owners.push(TrustedOwner {
        owner_id: other.clone(),
        label: None,
    });
    rejected(Mp001CertificateVerifier::new(pin(KEY), full));
    for kind in 0..3 {
        let mut t = TrustStore::default();
        match kind {
            0 => {
                t.revoked_owners = vec![
                    RevokedOwner {
                        owner_id: other.clone(),
                        reason: None
                    };
                    256
                ]
            }
            1 => {
                t.revoked_node_certs = vec![
                    RevokedNodeCert {
                        cert_id: "unused".into(),
                        reason: None
                    };
                    256
                ]
            }
            _ => {
                t.revoked_node_ids = vec![
                    RevokedNodeId {
                        node_endpoint_id: other.clone(),
                        reason: None
                    };
                    256
                ]
            }
        };
        verify(&verifier(t.clone()), LONG).unwrap();
        match kind {
            0 => t.revoked_owners.push(t.revoked_owners[0].clone()),
            1 => t.revoked_node_certs.push(t.revoked_node_certs[0].clone()),
            _ => t.revoked_node_ids.push(t.revoked_node_ids[0].clone()),
        };
        rejected(Mp001CertificateVerifier::new(pin(KEY), t));
    }
    for kind in 0..4 {
        let mut t = TrustStore::default();
        let exact = Some("λ".repeat(128));
        match kind {
            0 => t.trusted_owners.push(TrustedOwner {
                owner_id: other.clone(),
                label: exact,
            }),
            1 => t.revoked_owners.push(RevokedOwner {
                owner_id: other.clone(),
                reason: exact,
            }),
            2 => t.revoked_node_certs.push(RevokedNodeCert {
                cert_id: "x".repeat(128),
                reason: exact,
            }),
            _ => t.revoked_node_ids.push(RevokedNodeId {
                node_endpoint_id: "AB".repeat(32),
                reason: exact,
            }),
        };
        verify(&verifier(t.clone()), LONG).unwrap();
        let over = Some("λ".repeat(129));
        match kind {
            0 => t.trusted_owners[0].label = over,
            1 => t.revoked_owners[0].reason = over,
            2 => t.revoked_node_certs[0].reason = over,
            _ => t.revoked_node_ids[0].reason = over,
        };
        rejected(Mp001CertificateVerifier::new(pin(KEY), t));
    }
    let mut t = TrustStore {
        version: 2,
        ..TrustStore::default()
    };
    rejected(Mp001CertificateVerifier::new(pin(KEY), t.clone()));
    t.version = 1;
    t.revoked_owners.push(RevokedOwner {
        owner_id: OWNER_ID.to_uppercase(),
        reason: None,
    });
    rejected(Mp001CertificateVerifier::new(pin(KEY), t));
}
#[test]
fn cloned_trust_is_frozen_not_a_live_revocation_subscription() {
    let mut current = TrustStore::default();
    let old = verifier(current.clone());
    verify(&old, LONG).unwrap();
    current.revoke_owner(OWNER_ID.into(), None);
    // Documented snapshot limitation, NOT permission to use stale policy.
    verify(&old, LONG).unwrap();
    let rebuilt = verifier(current);
    rejected(verify(&rebuilt, LONG));
    // Trusted caller must reconstruct from current policy EVERY attempt.
}
