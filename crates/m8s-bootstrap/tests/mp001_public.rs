#![cfg(feature = "mp001-certificates")]
// Exact public vectors from accepted canonical tests; no signer or encoder.
use m8s_bootstrap::mp001::{Mp001CertificateVerifier, Mp001TrustPin};
use m8s_bootstrap::{CertificateClaims, CertificateVerifier, EndpointId, NodeCertificate};
use mesh_llm_identity::TrustStore;
const CERTIFICATE_JSON: &str = r#"{"claim":{"version":1,"cert_id":"synthetic-cert","owner_id":"fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889","owner_sign_public_key":"ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c","node_endpoint_id":"4242424242424242424242424242424242424242424242424242424242424242","issued_at_unix_ms":2000,"expires_at_unix_ms":10000,"node_label":"studio-λ"},"signature":"f2386312948ff46ee62cc272d744cd49dfa6faaa9a09bb9e4d58222b7a83696fae6c38a284e9901821b2c016c3486e2e1d6b72fdd7f9050429555bff5b1d8502"}"#;
const OPTIONAL_JSON: &str = r#"{"claim":{"version":1,"cert_id":"synthetic-optional","owner_id":"5c29b78f10a35a49a6231d08ee840a04bcc3a37a4f9d46df327dec56b8a9b7c9","owner_sign_public_key":"1398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca","node_endpoint_id":"abababababababababababababababababababababababababababababababab","issued_at_unix_ms":2000,"expires_at_unix_ms":10000,"hostname_hint":""},"signature":"0ffaf90b34e267b8e1243d309de4eecb457005ea9b8bc97ed5a5d4d53b9ca162b2654981bc80f675cea3e75566648ef3bf7ac766b284025fe9dbdeb188b5190a"}"#;
fn verifier() -> Mp001CertificateVerifier {
    let key = hex::decode("ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c")
        .unwrap()
        .try_into()
        .unwrap();
    Mp001CertificateVerifier::new(
        Mp001TrustPin::new(EndpointId([1; 32]), key).unwrap(),
        TrustStore::default(),
    )
    .unwrap()
}
#[test]
fn real_trait_refuses_expired_static_vectors() {
    for vector in [CERTIFICATE_JSON, OPTIONAL_JSON] {
        assert!(
            verifier()
                .verify(
                    &NodeCertificate(vector.as_bytes().to_vec()),
                    &CertificateClaims {
                        owner: EndpointId([1; 32]),
                        node: EndpointId([0x42; 32]),
                        issued_at: 2,
                        expires_at: 10,
                    }
                )
                .is_err()
        );
    }
}
#[test]
fn pin_and_snapshot_public_construction_fail_closed() {
    // Use a known valid public key for snapshot validation, no secret material.
    let key: [u8; 32] =
        hex::decode("ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c")
            .unwrap()
            .try_into()
            .unwrap();
    let mut trust = TrustStore {
        version: 2,
        ..TrustStore::default()
    };
    assert!(
        Mp001CertificateVerifier::new(
            Mp001TrustPin::new(EndpointId([1; 32]), key).unwrap(),
            trust.clone()
        )
        .is_err()
    );
    trust.version = 1;
    trust.revoke_owner("invalid".into(), None);
    assert!(
        Mp001CertificateVerifier::new(Mp001TrustPin::new(EndpointId([1; 32]), key).unwrap(), trust)
            .is_err()
    );
}

// PRIMARY-reviewed capture: agor://kb/document/01a160ea-273a-7685-8f4b-5c11b428bde1
// Accepted c2190fdd6bc4f9100ac5029621622341c3e8c8e6 disposable TESTCOPY,
// existing PRIVATE canonical_claim_bytes, in-memory PUBLIC synthetic seed [7;32].
// No encoder/signing API is copied here; only exact public output constants.
// Capture patch SHA f13f76d98d510e66c75f097789944c0eee2fd1634ab181c72831dd363703f620,
// recipe SHA 07c1460a8474972dde7b8cb9744f02adeed6491e7a7b97ee207a6f82946925f3.
// All have node[0x42;32], label Some("synthetic-only"), hostname None.
// ABSURD synthetic2020..2100 lifetime is NOT production issuance authorization.
// Constants include the original one trailing newline; hashes pin exact output.
// Full capture/toolchain/provenance/18-file manifest details in crate README.
const LONG_WINDOW_JSON: &str = r#"{"claim":{"version":1,"cert_id":"m8s-synthetic-public-long-window-v1","owner_id":"fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889","owner_sign_public_key":"ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c","node_endpoint_id":"4242424242424242424242424242424242424242424242424242424242424242","issued_at_unix_ms":1577836800000,"expires_at_unix_ms":4102444800000,"node_label":"synthetic-only"},"signature":"688e2126eb9a0cd91ec25dffb465f9c19b3f6f065e8445be7171377a45da0ba674db0d311464c6b05971cc5a258eed9211a6caca7f7cc1d8e4ba7127c1c33907"}
"#;
const FUTURE_JSON: &str = r#"{"claim":{"version":1,"cert_id":"m8s-synthetic-public-future-v1","owner_id":"fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889","owner_sign_public_key":"ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c","node_endpoint_id":"4242424242424242424242424242424242424242424242424242424242424242","issued_at_unix_ms":4102444800000,"expires_at_unix_ms":4102531200000,"node_label":"synthetic-only"},"signature":"3c3182a278a4c9119652c9657ff98636cbe0f3d3d216bf9c7324d5579dba4322999cca59dcf972d45120b9e34878f9d9d649b3ddaeb45c717150f35dfdec6809"}
"#;
const REMAINDER_JSON: &str = r#"{"claim":{"version":1,"cert_id":"m8s-synthetic-public-millisecond-remainder-v1","owner_id":"fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889","owner_sign_public_key":"ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c","node_endpoint_id":"4242424242424242424242424242424242424242424242424242424242424242","issued_at_unix_ms":1577836800001,"expires_at_unix_ms":4102444800000,"node_label":"synthetic-only"},"signature":"86cce9c931cf6e0356bb08cd5471681eaebdc971e6a91c6f1528ac6877a924f066b9742730bbf501bc3b09e5aa509548e040777972caf3a194c170152357450c"}
"#;

const LONG_ISSUED_SECONDS: i64 = 1_577_836_800;
const LONG_EXPIRES_SECONDS: i64 = 4_102_444_800;

fn current_fixture_time_ms() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("fixture suite requires a UTC clock after epoch");
    let millis = u64::try_from(now.as_millis()).expect("fixture clock must fit u64 milliseconds");
    assert!(
        (1_577_836_800_000..4_102_444_800_000).contains(&millis),
        "approved fixture suite only supports real UTC2020-01-01 <= now <2100-01-01; refresh reviewed fixtures, never skip"
    );
    millis
}
fn long_claims() -> CertificateClaims {
    CertificateClaims {
        owner: EndpointId([1; 32]),
        node: EndpointId([0x42; 32]),
        issued_at: LONG_ISSUED_SECONDS,
        expires_at: LONG_EXPIRES_SECONDS,
    }
}
fn canonical_positive(json: &str, now_ms: u64) {
    use mesh_llm_identity::{
        OwnershipStatus, SignedNodeOwnership, TrustPolicy, verify_node_ownership,
    };
    let certificate: SignedNodeOwnership = serde_json::from_str(json).unwrap();
    let mut trust = TrustStore::default();
    trust.add_trusted_owner(certificate.claim.owner_id.clone(), None);
    let summary = verify_node_ownership(
        Some(&certificate),
        &[0x42; 32],
        &trust,
        TrustPolicy::Allowlist,
        now_ms,
    );
    assert!(summary.verified);
    assert_eq!(summary.status, OwnershipStatus::Verified);
}

#[test]
fn approved_public_fixture_bytes_match_capture_hashes() {
    use sha2::{Digest, Sha256};
    for (json, digest) in [
        (
            LONG_WINDOW_JSON,
            "5594d0ca6b4c6af64ff3f89414ca9dc0c710d45e666f95c20cc0f19a4a2d4f88",
        ),
        (
            FUTURE_JSON,
            "a3979316e53a105f5cecfbecf7924e777bb9323a52e42a8c88113c5713c3ea45",
        ),
        (
            REMAINDER_JSON,
            "cc81f9d23007237496ef3b4b91ad7cc39c865b7f20f819045eb5af1c70ef3dc5",
        ),
    ] {
        assert_eq!(hex::encode(Sha256::digest(json.as_bytes())), digest);
    }
}
#[test]
fn real_current_canonical_signature_and_trait_positive() {
    let now_ms = current_fixture_time_ms();
    canonical_positive(LONG_WINDOW_JSON, now_ms);
    // Empty incoming allowlist is intentional: the immutable trusted pin adds
    // its canonical-derived owner; default incoming Off cannot bypass policy.
    verifier()
        .verify(
            &NodeCertificate(LONG_WINDOW_JSON.as_bytes().to_vec()),
            &long_claims(),
        )
        .unwrap();
}
#[test]
fn real_current_uppercase_hex_preserves_signature_and_binding() {
    current_fixture_time_ms();
    let certificate: mesh_llm_identity::SignedNodeOwnership =
        serde_json::from_str(LONG_WINDOW_JSON).unwrap();
    // Raw text substitutions change only hex case, never re-encode/sign claims.
    let uppercase = LONG_WINDOW_JSON
        .replace(
            &certificate.claim.owner_sign_public_key,
            &certificate.claim.owner_sign_public_key.to_ascii_uppercase(),
        )
        .replace(
            &certificate.claim.node_endpoint_id,
            &certificate.claim.node_endpoint_id.to_ascii_uppercase(),
        )
        .replace(
            &certificate.signature,
            &certificate.signature.to_ascii_uppercase(),
        );
    assert_ne!(uppercase, LONG_WINDOW_JSON);
    verifier()
        .verify(&NodeCertificate(uppercase.into_bytes()), &long_claims())
        .unwrap();
}
#[test]
fn genuine_signed_future_and_remainder_base_controls_but_trait_refuses() {
    let now_ms = current_fixture_time_ms();
    for (json, expected) in [
        (
            FUTURE_JSON,
            CertificateClaims {
                issued_at: 4_102_444_800,
                expires_at: 4_102_531_200,
                ..long_claims()
            },
        ),
        (REMAINDER_JSON, long_claims()),
    ] {
        // Isolate adapter future-issued/exact-second restrictions from invalid
        // signature rejection: these DIFFERENT signed certs verify canonically.
        canonical_positive(json, now_ms);
        assert!(matches!(
            verifier().verify(&NodeCertificate(json.as_bytes().to_vec()), &expected),
            Err(m8s_bootstrap::Error::Certificate)
        ));
    }
}
