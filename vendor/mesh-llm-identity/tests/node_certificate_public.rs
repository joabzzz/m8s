//! External, public-data-only contract: run with --no-default-features.
//!
//! Provenance: synthetic Ed25519 seed [7; 32], endpoint [0x42; 32], and fixed
//! millisecond times. node_certificate::tests::canonical_public_vector signs
//! only in memory using the canonical encoder moved verbatim from cc08962a's
//! ownership.rs. It pins the exact domain-separated bytes and this signature.
//! No owner files, host signer, clock, keychain, or network are used here.

use mesh_llm_identity::node_certificate::{
    NODE_OWNERSHIP_VERSION, RevokedNodeCert, RevokedNodeId, RevokedOwner, TRUST_STORE_VERSION,
    TrustedOwner,
};
use mesh_llm_identity::{
    NodeOwnershipClaim, OwnershipStatus, OwnershipSummary, SignedNodeOwnership, TrustPolicy,
    TrustStore, certificate_needs_renewal, verify_node_ownership,
};

const CERTIFICATE_JSON: &str = r#"{"claim":{"version":1,"cert_id":"synthetic-cert","owner_id":"fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889","owner_sign_public_key":"ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c","node_endpoint_id":"4242424242424242424242424242424242424242424242424242424242424242","issued_at_unix_ms":2000,"expires_at_unix_ms":10000,"node_label":"studio-λ"},"signature":"f2386312948ff46ee62cc272d744cd49dfa6faaa9a09bb9e4d58222b7a83696fae6c38a284e9901821b2c016c3486e2e1d6b72fdd7f9050429555bff5b1d8502"}"#;

fn certificate() -> SignedNodeOwnership {
    serde_json::from_str(CERTIFICATE_JSON).unwrap()
}

fn summary(
    cert: &SignedNodeOwnership,
    store: &TrustStore,
    policy: TrustPolicy,
) -> OwnershipSummary {
    verify_node_ownership(Some(cert), &[0x42; 32], store, policy, 3_000)
}

#[test]
fn public_vector_roundtrips_and_verifies() {
    let cert = certificate();
    let _: &NodeOwnershipClaim = &cert.claim;
    assert_eq!(serde_json::to_string(&cert).unwrap(), CERTIFICATE_JSON);
    assert_eq!(cert.claim.version, NODE_OWNERSHIP_VERSION);
    assert_eq!(TrustStore::default().version, TRUST_STORE_VERSION);
    assert_eq!(
        summary(&cert, &TrustStore::default(), TrustPolicy::Off),
        OwnershipSummary {
            owner_id: Some(cert.claim.owner_id.clone()),
            cert_id: Some("synthetic-cert".into()),
            status: OwnershipStatus::Verified,
            expires_at_unix_ms: Some(10_000),
            node_label: Some("studio-λ".into()),
            hostname_hint: None,
            verified: true,
        }
    );
}

#[test]
fn every_signed_field_is_bound() {
    let original = certificate();
    let mut changes = Vec::new();
    let mut claim = original.claim.clone();
    claim.cert_id.push('x');
    changes.push(claim);
    let mut claim = original.claim.clone();
    claim.owner_id = "00".repeat(32);
    changes.push(claim);
    let mut claim = original.claim.clone();
    claim.owner_sign_public_key = "00".repeat(32);
    changes.push(claim);
    let mut claim = original.claim.clone();
    claim.issued_at_unix_ms += 1;
    changes.push(claim);
    let mut claim = original.claim.clone();
    claim.expires_at_unix_ms += 1;
    changes.push(claim);
    let mut claim = original.claim.clone();
    claim.node_label = None;
    changes.push(claim);
    let mut claim = original.claim.clone();
    claim.node_label = Some(String::new());
    changes.push(claim);
    let mut claim = original.claim.clone();
    claim.hostname_hint = Some(String::new());
    changes.push(claim);
    for claim in changes {
        let cert = SignedNodeOwnership {
            claim,
            ..original.clone()
        };
        assert_eq!(
            summary(&cert, &TrustStore::default(), TrustPolicy::Off).status,
            OwnershipStatus::InvalidSignature
        );
    }
}

#[test]
fn node_binding_and_precedence_are_unchanged() {
    let cert = certificate();
    assert_eq!(
        verify_node_ownership(
            Some(&cert),
            &[0x24; 32],
            &TrustStore::default(),
            TrustPolicy::Off,
            3_000
        )
        .status,
        OwnershipStatus::MismatchedNodeId
    );
    let mut cert = cert;
    cert.claim.node_endpoint_id = hex::encode([0x24; 32]);
    assert_eq!(
        summary(&cert, &TrustStore::default(), TrustPolicy::Off).status,
        OwnershipStatus::MismatchedNodeId
    );
    assert_eq!(
        verify_node_ownership(
            Some(&cert),
            &[0x24; 32],
            &TrustStore::default(),
            TrustPolicy::Off,
            3_000
        )
        .status,
        OwnershipStatus::InvalidSignature
    );
}

#[test]
fn malformed_keys_and_signatures_fail_closed() {
    for malformed in ["zz", "00", ""] {
        let mut cert = certificate();
        cert.claim.owner_sign_public_key = malformed.into();
        assert_eq!(
            summary(&cert, &TrustStore::default(), TrustPolicy::Off).status,
            OwnershipStatus::InvalidSignature
        );
        let mut cert = certificate();
        cert.claim.node_endpoint_id = malformed.into();
        assert_eq!(
            summary(&cert, &TrustStore::default(), TrustPolicy::Off).status,
            OwnershipStatus::InvalidSignature
        );
        let mut cert = certificate();
        cert.signature = malformed.into();
        assert_eq!(
            summary(&cert, &TrustStore::default(), TrustPolicy::Off).status,
            OwnershipStatus::InvalidSignature
        );
    }
    let mut cert = certificate();
    cert.signature = "00".repeat(64);
    assert_eq!(
        summary(&cert, &TrustStore::default(), TrustPolicy::Off).status,
        OwnershipStatus::InvalidSignature
    );
}

#[test]
fn future_issuance_and_versions_preserve_existing_policy() {
    let cert = certificate();
    // The base verifier does not reject future issuance, even at epoch zero.
    assert!(
        verify_node_ownership(
            Some(&cert),
            &[0x42; 32],
            &TrustStore::default(),
            TrustPolicy::Off,
            0
        )
        .verified
    );
    let store = TrustStore {
        version: u32::MAX,
        ..TrustStore::default()
    };
    // Trust-store version validation belongs to the host loader, not this verifier.
    assert!(summary(&cert, &store, TrustPolicy::Off).verified);
    let mut cert = cert;
    cert.claim.version += 1;
    cert.signature.clear();
    assert_eq!(
        summary(&cert, &store, TrustPolicy::Off).status,
        OwnershipStatus::UnsupportedProtocol
    );
}

#[test]
fn expiry_and_renewal_boundaries_are_unchanged() {
    let cert = certificate();
    for now in [10_000, 10_001] {
        let result = verify_node_ownership(
            Some(&cert),
            &[0x42; 32],
            &TrustStore::default(),
            TrustPolicy::Off,
            now,
        );
        assert_eq!(result.status, OwnershipStatus::Expired);
        assert!(!result.verified);
    }
    assert!(
        verify_node_ownership(
            Some(&cert),
            &[0x42; 32],
            &TrustStore::default(),
            TrustPolicy::Off,
            9_999
        )
        .verified
    );
    assert!(!certificate_needs_renewal(&cert, 1, 8_999));
    assert!(certificate_needs_renewal(&cert, 1, 9_000));
    assert!(certificate_needs_renewal(&cert, 0, 10_000));
    assert!(certificate_needs_renewal(&cert, u64::MAX, 0));
}

#[test]
fn missing_certificate_is_unsigned_under_every_policy() {
    for policy in [
        TrustPolicy::Off,
        TrustPolicy::PreferOwned,
        TrustPolicy::RequireOwned,
        TrustPolicy::Allowlist,
    ] {
        assert_eq!(
            verify_node_ownership(None, &[0x42; 32], &TrustStore::default(), policy, 3_000),
            OwnershipSummary::default()
        );
    }
}

#[test]
fn allowlist_and_all_revocations_apply_under_every_policy() {
    let cert = certificate();
    for policy in [
        TrustPolicy::Off,
        TrustPolicy::PreferOwned,
        TrustPolicy::RequireOwned,
        TrustPolicy::Allowlist,
    ] {
        let mut store = TrustStore::default();
        let result = summary(&cert, &store, policy);
        assert_eq!(
            result.status,
            if policy == TrustPolicy::Allowlist {
                OwnershipStatus::UntrustedOwner
            } else {
                OwnershipStatus::Verified
            }
        );
        store.add_trusted_owner(cert.claim.owner_id.clone(), Some("synthetic".into()));
        assert!(summary(&cert, &store, policy).verified);
        let mut revoked = store.clone();
        revoked.revoke_owner(cert.claim.owner_id.clone(), None);
        assert_eq!(
            summary(&cert, &revoked, policy).status,
            OwnershipStatus::RevokedOwner
        );
        let mut revoked = store.clone();
        revoked.revoke_node_cert(cert.claim.cert_id.clone(), None);
        assert_eq!(
            summary(&cert, &revoked, policy).status,
            OwnershipStatus::RevokedCert
        );
        store.revoke_node_id(cert.claim.node_endpoint_id.to_uppercase(), None);
        assert_eq!(
            summary(&cert, &store, policy).status,
            OwnershipStatus::RevokedNodeId
        );
    }
}

#[test]
fn trust_metadata_mutations_keep_sorting_and_idempotence() {
    let mut store =
        TrustStore::default().merged_with_trusted_owners(&["b".into(), "a".into(), "b".into()]);
    store.add_trusted_owner("a".into(), Some("label".into()));
    store.add_trusted_owner("a".into(), None);
    assert_eq!(
        store.trusted_owners,
        vec![
            TrustedOwner {
                owner_id: "a".into(),
                label: Some("label".into())
            },
            TrustedOwner {
                owner_id: "b".into(),
                label: None
            }
        ]
    );
    assert!(store.remove_trusted_owner("a"));
    assert!(!store.remove_trusted_owner("a"));
    for id in ["b", "a", "b"] {
        store.revoke_owner(id.into(), None);
        store.revoke_node_cert(id.into(), None);
        store.revoke_node_id(id.into(), None);
    }
    store.revoke_node_id("B".into(), Some("ignored duplicate".into()));
    assert_eq!(
        store.revoked_owners,
        vec![
            RevokedOwner {
                owner_id: "a".into(),
                reason: None
            },
            RevokedOwner {
                owner_id: "b".into(),
                reason: None
            }
        ]
    );
    assert_eq!(
        store.revoked_node_certs,
        vec![
            RevokedNodeCert {
                cert_id: "a".into(),
                reason: None
            },
            RevokedNodeCert {
                cert_id: "b".into(),
                reason: None
            }
        ]
    );
    assert_eq!(
        store.revoked_node_ids,
        vec![
            RevokedNodeId {
                node_endpoint_id: "a".into(),
                reason: None
            },
            RevokedNodeId {
                node_endpoint_id: "b".into(),
                reason: None
            }
        ]
    );
}

#[cfg(feature = "host-io")]
#[test]
fn legacy_module_and_root_paths_are_identical_types_and_functions() {
    use mesh_llm_identity::ownership;
    let cert: ownership::SignedNodeOwnership = certificate();
    let store: ownership::TrustStore = TrustStore::default();
    let legacy: OwnershipSummary = ownership::verify_node_ownership(
        Some(&cert),
        &[0x42; 32],
        &store,
        ownership::TrustPolicy::Off,
        3_000,
    );
    assert_eq!(legacy, summary(&cert, &store, TrustPolicy::Off));
    assert_eq!(ownership::NODE_OWNERSHIP_VERSION, NODE_OWNERSHIP_VERSION);
    assert_eq!(ownership::TRUST_STORE_VERSION, TRUST_STORE_VERSION);
    let _: ownership::RevokedOwner = RevokedOwner {
        owner_id: "synthetic".into(),
        reason: None,
    };
}

// Second fixed vector: seed [8; 32], None label, Some(empty) hostname. This
// distinguishes the optional tags and exercises ASCII-case-insensitive node IDs.
const OPTIONAL_JSON: &str = r#"{"claim":{"version":1,"cert_id":"synthetic-optional","owner_id":"5c29b78f10a35a49a6231d08ee840a04bcc3a37a4f9d46df327dec56b8a9b7c9","owner_sign_public_key":"1398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca","node_endpoint_id":"abababababababababababababababababababababababababababababababab","issued_at_unix_ms":2000,"expires_at_unix_ms":10000,"hostname_hint":""},"signature":"0ffaf90b34e267b8e1243d309de4eecb457005ea9b8bc97ed5a5d4d53b9ca162b2654981bc80f675cea3e75566648ef3bf7ac766b284025fe9dbdeb188b5190a"}"#;

#[test]
fn optional_vector_and_hex_case_preserve_wire_semantics() {
    let mut cert: SignedNodeOwnership = serde_json::from_str(OPTIONAL_JSON).unwrap();
    assert_eq!(serde_json::to_string(&cert).unwrap(), OPTIONAL_JSON);
    let mut store = TrustStore::default();
    assert!(
        verify_node_ownership(Some(&cert), &[0xab; 32], &store, TrustPolicy::Off, 3_000).verified
    );
    cert.claim.node_endpoint_id.make_ascii_uppercase();
    cert.claim.owner_sign_public_key.make_ascii_uppercase();
    cert.signature.make_ascii_uppercase();
    assert!(
        verify_node_ownership(Some(&cert), &[0xab; 32], &store, TrustPolicy::Off, 3_000).verified
    );
    store.revoke_node_id("ab".repeat(32), None);
    assert_eq!(
        verify_node_ownership(Some(&cert), &[0xab; 32], &store, TrustPolicy::Off, 3_000).status,
        OwnershipStatus::RevokedNodeId
    );
    cert.claim.hostname_hint = None;
    assert_eq!(
        verify_node_ownership(Some(&cert), &[0xab; 32], &store, TrustPolicy::Off, 3_000).status,
        OwnershipStatus::InvalidSignature
    );
}

#[test]
fn substituting_another_valid_signer_and_owner_id_does_not_verify() {
    let other: SignedNodeOwnership = serde_json::from_str(OPTIONAL_JSON).unwrap();
    let mut cert = certificate();
    cert.claim.owner_id = other.claim.owner_id;
    cert.claim.owner_sign_public_key = other.claim.owner_sign_public_key;
    assert_eq!(
        summary(&cert, &TrustStore::default(), TrustPolicy::Off).status,
        OwnershipStatus::InvalidSignature
    );
}

#[test]
fn rejection_precedence_is_expiry_then_owner_then_cert_then_node() {
    let cert = certificate();
    let mut store = TrustStore::default();
    store.revoke_owner(cert.claim.owner_id.clone(), None);
    store.revoke_node_cert(cert.claim.cert_id.clone(), None);
    store.revoke_node_id(cert.claim.node_endpoint_id.clone(), None);
    assert_eq!(
        verify_node_ownership(
            Some(&cert),
            &[0x42; 32],
            &store,
            TrustPolicy::Allowlist,
            10_000
        )
        .status,
        OwnershipStatus::Expired
    );
    assert_eq!(
        summary(&cert, &store, TrustPolicy::Allowlist).status,
        OwnershipStatus::RevokedOwner
    );
    store.revoked_owners.clear();
    assert_eq!(
        summary(&cert, &store, TrustPolicy::Allowlist).status,
        OwnershipStatus::RevokedCert
    );
    store.revoked_node_certs.clear();
    assert_eq!(
        summary(&cert, &store, TrustPolicy::Allowlist).status,
        OwnershipStatus::RevokedNodeId
    );
}
