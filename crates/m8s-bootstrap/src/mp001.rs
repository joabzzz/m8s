//! Default-off canonical MP-001 verification of public data only.
//!
//! This immutable snapshot is NOT live revocation authority or proof of policy
//! currentness. The trusted caller must rebuild it from current policy before
//! each redemption/startup attempt. Verification supplies no secret access or IO.
//! The separate owner-signing feature borrows an already-custodied owner key.
use crate::{CertificateClaims, CertificateVerifier, EndpointId, Error, NodeCertificate, Result};
use mesh_llm_identity::{
    NodeOwnershipClaim, OwnershipStatus, SignedNodeOwnership, TrustPolicy, TrustStore,
    owner_id_from_verifying_key, verify_node_ownership,
};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_JSON: usize = 128 * 1024;
const MAX_ENTRIES: usize = 256;

#[cfg(feature = "mp001-owner-signing")]
pub mod signing;

/// Trusted configuration, never reconstructed from certificate claims.
/// Private fields; deliberately no Deserialize or mutation API.
pub struct Mp001TrustPin {
    endpoint: EndpointId,
    key: [u8; 32],
    owner_id: String,
}
impl Mp001TrustPin {
    pub fn new(endpoint: EndpointId, mesh_signing_public_key: [u8; 32]) -> Result<Self> {
        let key = ed25519_dalek::VerifyingKey::from_bytes(&mesh_signing_public_key)
            .map_err(|_| Error::Certificate)?;
        Ok(Self {
            endpoint,
            key: mesh_signing_public_key,
            owner_id: owner_id_from_verifying_key(&key),
        })
    }
}

/// Bounded immutable policy snapshot, not live propagation.
/// Reconstruct from current trusted policy before EVERY attempt.
pub struct Mp001CertificateVerifier {
    pin: Mp001TrustPin,
    trust: TrustStore,
}
impl Mp001CertificateVerifier {
    pub fn new(pin: Mp001TrustPin, mut trust: TrustStore) -> Result<Self> {
        validate_trust(&trust)?;
        // A pin is explicit allowlist authority, but NEVER overrides revocations.
        if !trust
            .trusted_owners
            .iter()
            .any(|e| e.owner_id == pin.owner_id)
        {
            if trust.trusted_owners.len() == MAX_ENTRIES {
                return Err(Error::Certificate);
            }
            trust.add_trusted_owner(pin.owner_id.clone(), None);
        }
        trust.policy = TrustPolicy::Allowlist;
        Ok(Self { pin, trust })
    }

    fn verify_inner(
        &self,
        certificate: &NodeCertificate,
        expected: &CertificateClaims,
        now_ms: u64,
    ) -> Result<()> {
        let issued = seconds_ms(expected.issued_at)?;
        let expires = seconds_ms(expected.expires_at)?;
        if expected.owner != self.pin.endpoint
            || expires <= issued
            || now_ms < issued
            || now_ms >= expires
        {
            return Err(Error::Certificate);
        }
        let signed = parse(&certificate.0)?;
        let claim = &signed.claim;
        if claim.owner_id != self.pin.owner_id
            || decode32(&claim.owner_sign_public_key)? != self.pin.key
            || decode32(&claim.node_endpoint_id)? != expected.node.0
            || claim.issued_at_unix_ms != issued
            || claim.expires_at_unix_ms != expires
        {
            return Err(Error::Certificate);
        }
        let result = verify_node_ownership(
            Some(&signed),
            &expected.node.0,
            &self.trust,
            TrustPolicy::Allowlist,
            now_ms,
        );
        if result.verified && result.status == OwnershipStatus::Verified {
            Ok(())
        } else {
            Err(Error::Certificate)
        }
    }

    #[cfg(test)]
    fn verify_at(
        &self,
        certificate: &NodeCertificate,
        expected: &CertificateClaims,
        now_ms: u64,
    ) -> Result<()> {
        self.verify_inner(certificate, expected, now_ms)
    }
}
impl CertificateVerifier for Mp001CertificateVerifier {
    fn verify(&self, certificate: &NodeCertificate, expected: &CertificateClaims) -> Result<()> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Certificate)?;
        let now_ms = u64::try_from(duration.as_millis()).map_err(|_| Error::Certificate)?;
        self.verify_inner(certificate, expected, now_ms)
    }
}

fn seconds_ms(seconds: i64) -> Result<u64> {
    u64::try_from(seconds)
        .ok()
        .and_then(|s| s.checked_mul(1000))
        .ok_or(Error::Certificate)
}
fn hex_string(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn owner_id(value: &str) -> bool {
    hex_string(value, 64) && !value.bytes().any(|b| b.is_ascii_uppercase())
}
fn cert_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128
}
fn metadata(value: &Option<String>) -> bool {
    value.as_ref().is_none_or(|v| v.len() <= 256)
}
fn decode32(value: &str) -> Result<[u8; 32]> {
    if !hex_string(value, 64) {
        return Err(Error::Certificate);
    }
    let mut out = [0; 32];
    hex::decode_to_slice(value, &mut out).map_err(|_| Error::Certificate)?;
    Ok(out)
}
fn validate_trust(trust: &TrustStore) -> Result<()> {
    if trust.version != mesh_llm_identity::node_certificate::TRUST_STORE_VERSION
        || [
            trust.trusted_owners.len(),
            trust.revoked_owners.len(),
            trust.revoked_node_certs.len(),
            trust.revoked_node_ids.len(),
        ]
        .into_iter()
        .any(|n| n > MAX_ENTRIES)
        || trust
            .trusted_owners
            .iter()
            .any(|e| !owner_id(&e.owner_id) || !metadata(&e.label))
        || trust
            .revoked_owners
            .iter()
            .any(|e| !owner_id(&e.owner_id) || !metadata(&e.reason))
        || trust
            .revoked_node_certs
            .iter()
            .any(|e| !cert_id(&e.cert_id) || !metadata(&e.reason))
        || trust
            .revoked_node_ids
            .iter()
            .any(|e| !hex_string(&e.node_endpoint_id, 64) || !metadata(&e.reason))
    {
        return Err(Error::Certificate);
    }
    Ok(())
}

// Strict input schema only. This is NOT a canonical encoder and never signs.
// Derive rejects duplicate fields, including optional fields, unknown nested
// fields and wrong types. from_slice also rejects trailing non-whitespace data.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCertificate {
    claim: WireClaim,
    signature: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireClaim {
    version: u32,
    cert_id: String,
    owner_id: String,
    owner_sign_public_key: String,
    node_endpoint_id: String,
    issued_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    node_label: Option<String>,
    hostname_hint: Option<String>,
}
fn parse(bytes: &[u8]) -> Result<SignedNodeOwnership> {
    if bytes.is_empty() || bytes.len() > MAX_JSON {
        return Err(Error::Certificate);
    }
    let wire: WireCertificate = serde_json::from_slice(bytes).map_err(|_| Error::Certificate)?;
    let c = wire.claim;
    if !cert_id(&c.cert_id)
        || !owner_id(&c.owner_id)
        || !hex_string(&c.owner_sign_public_key, 64)
        || !hex_string(&c.node_endpoint_id, 64)
        || !hex_string(&wire.signature, 128)
        || !metadata(&c.node_label)
        || !metadata(&c.hostname_hint)
    {
        return Err(Error::Certificate);
    }
    Ok(SignedNodeOwnership {
        claim: NodeOwnershipClaim {
            version: c.version,
            cert_id: c.cert_id,
            owner_id: c.owner_id,
            owner_sign_public_key: c.owner_sign_public_key,
            node_endpoint_id: c.node_endpoint_id,
            issued_at_unix_ms: c.issued_at_unix_ms,
            expires_at_unix_ms: c.expires_at_unix_ms,
            node_label: c.node_label,
            hostname_hint: c.hostname_hint,
        },
        signature: wire.signature,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesh_llm_identity::node_certificate::{
        RevokedNodeCert, RevokedNodeId, RevokedOwner, TrustedOwner,
    };
    const CERTIFICATE_JSON: &str = r#"{"claim":{"version":1,"cert_id":"synthetic-cert","owner_id":"fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889","owner_sign_public_key":"ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c","node_endpoint_id":"4242424242424242424242424242424242424242424242424242424242424242","issued_at_unix_ms":2000,"expires_at_unix_ms":10000,"node_label":"studio-λ"},"signature":"f2386312948ff46ee62cc272d744cd49dfa6faaa9a09bb9e4d58222b7a83696fae6c38a284e9901821b2c016c3486e2e1d6b72fdd7f9050429555bff5b1d8502"}"#;
    const OPTIONAL_JSON: &str = r#"{"claim":{"version":1,"cert_id":"synthetic-optional","owner_id":"5c29b78f10a35a49a6231d08ee840a04bcc3a37a4f9d46df327dec56b8a9b7c9","owner_sign_public_key":"1398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93ca","node_endpoint_id":"abababababababababababababababababababababababababababababababab","issued_at_unix_ms":2000,"expires_at_unix_ms":10000,"hostname_hint":""},"signature":"0ffaf90b34e267b8e1243d309de4eecb457005ea9b8bc97ed5a5d4d53b9ca162b2654981bc80f675cea3e75566648ef3bf7ac766b284025fe9dbdeb188b5190a"}"#;
    fn claims() -> CertificateClaims {
        CertificateClaims {
            owner: EndpointId([1; 32]),
            node: EndpointId([0x42; 32]),
            issued_at: 2,
            expires_at: 10,
        }
    }
    fn verifier(trust: TrustStore) -> Mp001CertificateVerifier {
        Mp001CertificateVerifier::new(
            Mp001TrustPin::new(
                EndpointId([1; 32]),
                decode32("ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c")
                    .unwrap(),
            )
            .unwrap(),
            trust,
        )
        .unwrap()
    }
    fn cert(s: &str) -> NodeCertificate {
        NodeCertificate(s.as_bytes().to_vec())
    }
    fn rejects(s: &str) {
        assert!(
            verifier(TrustStore::default())
                .verify_at(&cert(s), &claims(), 3000)
                .is_err()
        );
    }

    #[test]
    fn canonical_vectors_real_crypto_private_clock_and_case() {
        let v = verifier(TrustStore::default());
        for now in [2000, 3000, 9999] {
            v.verify_at(&cert(CERTIFICATE_JSON), &claims(), now)
                .unwrap();
        }
        for now in [0, 1999, 10000, 10001, u64::MAX] {
            assert!(
                v.verify_at(&cert(CERTIFICATE_JSON), &claims(), now)
                    .is_err()
            );
        }
        let mut c: SignedNodeOwnership = serde_json::from_str(OPTIONAL_JSON).unwrap();
        let key = decode32(&c.claim.owner_sign_public_key).unwrap();
        let v = Mp001CertificateVerifier::new(
            Mp001TrustPin::new(EndpointId([1; 32]), key).unwrap(),
            TrustStore::default(),
        )
        .unwrap();
        let expected = CertificateClaims {
            node: EndpointId([0xab; 32]),
            ..claims()
        };
        v.verify_at(&cert(OPTIONAL_JSON), &expected, 3000).unwrap();
        c.claim.owner_sign_public_key.make_ascii_uppercase();
        c.claim.node_endpoint_id.make_ascii_uppercase();
        c.signature.make_ascii_uppercase();
        v.verify_at(
            &NodeCertificate(serde_json::to_vec(&c).unwrap()),
            &expected,
            3000,
        )
        .unwrap();
    }
    #[test]
    fn exact_expected_bindings_and_checked_time() {
        let v = verifier(TrustStore::default());
        let original = claims();
        for expected in [
            CertificateClaims {
                owner: EndpointId([2; 32]),
                ..original.clone()
            },
            CertificateClaims {
                node: EndpointId([2; 32]),
                ..original.clone()
            },
            CertificateClaims {
                issued_at: -1,
                ..original.clone()
            },
            CertificateClaims {
                expires_at: -1,
                ..original.clone()
            },
            CertificateClaims {
                issued_at: 10,
                ..original.clone()
            },
            CertificateClaims {
                expires_at: 2,
                ..original.clone()
            },
            CertificateClaims {
                issued_at: 3,
                ..original.clone()
            },
            CertificateClaims {
                expires_at: 11,
                ..original.clone()
            },
            CertificateClaims {
                expires_at: i64::MAX,
                ..original.clone()
            },
            CertificateClaims {
                issued_at: i64::MAX,
                ..original.clone()
            },
        ] {
            assert!(
                v.verify_at(&cert(CERTIFICATE_JSON), &expected, 3000)
                    .is_err()
            );
        }
        assert!(seconds_ms(-1).is_err());
        assert!(seconds_ms(i64::MAX).is_err());
        assert_eq!(seconds_ms(0).unwrap(), 0);
        assert_eq!(
            seconds_ms(18_446_744_073_709_551).unwrap(),
            18_446_744_073_709_551_000
        );
        assert!(seconds_ms(18_446_744_073_709_552).is_err());
    }
    #[test]
    fn valid_alternate_pin_is_not_certificate_supplied_trust() {
        let other: SignedNodeOwnership = serde_json::from_str(OPTIONAL_JSON).unwrap();
        let v = Mp001CertificateVerifier::new(
            Mp001TrustPin::new(
                EndpointId([1; 32]),
                decode32(&other.claim.owner_sign_public_key).unwrap(),
            )
            .unwrap(),
            TrustStore::default(),
        )
        .unwrap();
        assert!(
            v.verify_at(&cert(CERTIFICATE_JSON), &claims(), 3000)
                .is_err()
        );
    }
    #[test]
    fn pin_never_clears_revocations_and_snapshot_is_owned() {
        let original: SignedNodeOwnership = serde_json::from_str(CERTIFICATE_JSON).unwrap();
        let c = original.claim;
        let mut stores = Vec::new();
        let mut s = TrustStore::default();
        s.revoke_owner(c.owner_id.clone(), None);
        stores.push(s);
        let mut s = TrustStore::default();
        s.revoke_node_cert(c.cert_id.clone(), None);
        stores.push(s);
        let mut s = TrustStore::default();
        s.revoke_node_id(c.node_endpoint_id.to_uppercase(), None);
        stores.push(s);
        for store in stores {
            assert!(
                verifier(store)
                    .verify_at(&cert(CERTIFICATE_JSON), &claims(), 3000)
                    .is_err()
            );
        }
        let mut s = TrustStore::default();
        let v = verifier(s.clone());
        s.revoke_owner(c.owner_id, None);
        v.verify_at(&cert(CERTIFICATE_JSON), &claims(), 3000)
            .unwrap();
        assert!(
            verifier(s)
                .verify_at(&cert(CERTIFICATE_JSON), &claims(), 3000)
                .is_err()
        );
    }
    #[test]
    fn strict_json_duplicates_unknown_trailing_and_bound() {
        for s in [
            String::new(),
            " ".repeat(MAX_JSON + 1),
            format!("{CERTIFICATE_JSON}{{}}"),
            CERTIFICATE_JSON.replacen("{", "{\"extra\":0,", 1),
            CERTIFICATE_JSON.replace("\"version\":1", "\"version\":1,\"extra\":0"),
            CERTIFICATE_JSON.replace("\"version\":1", "\"version\":1,\"version\":1"),
            CERTIFICATE_JSON.replace("\"claim\":", "\"signature\":\"x\",\"claim\":"),
            CERTIFICATE_JSON.replace(
                "\"node_label\":\"studio-λ\"",
                "\"node_label\":null,\"node_label\":\"studio-λ\"",
            ),
            CERTIFICATE_JSON.replace(
                "\"node_label\":\"studio-λ\"",
                "\"hostname_hint\":null,\"hostname_hint\":null",
            ),
            CERTIFICATE_JSON.replace("\"version\":1", "\"version\":{}"),
        ] {
            rejects(&s);
        }
        let padded = format!(
            "{CERTIFICATE_JSON}{}",
            " ".repeat(MAX_JSON - CERTIFICATE_JSON.len())
        );
        verifier(TrustStore::default())
            .verify_at(&cert(&padded), &claims(), 3000)
            .unwrap();
    }
    #[test]
    fn signed_fields_grammar_bounds_and_millisecond_remainders() {
        let original: SignedNodeOwnership = serde_json::from_str(CERTIFICATE_JSON).unwrap();
        for field in [
            "cert_id",
            "owner_id",
            "owner_sign_public_key",
            "node_endpoint_id",
            "signature",
            "node_label",
            "hostname_hint",
        ] {
            for bad in [
                "".to_string(),
                "g".repeat(64),
                "0".repeat(63),
                "0".repeat(65),
                "λ".repeat(129),
            ] {
                let mut value = serde_json::to_value(&original).unwrap();
                if field == "signature" {
                    value[field] = bad.into();
                } else {
                    value["claim"][field] = bad.into();
                }
                // Empty optional metadata is grammatically legal, but still not signed.
                rejects(&value.to_string());
            }
        }
        for (field, val) in [
            ("version", 2),
            ("issued_at_unix_ms", 2001),
            ("expires_at_unix_ms", 10001),
            ("issued_at_unix_ms", 0),
        ] {
            let mut value = serde_json::to_value(&original).unwrap();
            value["claim"][field] = val.into();
            rejects(&value.to_string());
        }
        let mut value = serde_json::to_value(&original).unwrap();
        value["claim"]["owner_id"] = original.claim.owner_id.to_uppercase().into();
        rejects(&value.to_string());
        assert!(cert_id(&"λ".repeat(64)));
        assert!(!cert_id(&"λ".repeat(65)));
        assert!(metadata(&Some("λ".repeat(128))));
        assert!(!metadata(&Some("λ".repeat(129))));
        assert!(parse(&cert(CERTIFICATE_JSON).0).is_ok());
    }
    #[test]
    fn all_trust_lists_bounds_grammar_metadata_and_version() {
        let owner = "0".repeat(64);
        let mut s = TrustStore {
            version: 2,
            ..TrustStore::default()
        };
        assert!(validate_trust(&s).is_err());
        s.version = 1;
        for n in [256, 257] {
            let allowed = n == 256;
            s.trusted_owners = vec![
                TrustedOwner {
                    owner_id: owner.clone(),
                    label: None
                };
                n
            ];
            assert_eq!(validate_trust(&s).is_ok(), allowed);
            s.trusted_owners.clear();
            s.revoked_owners = vec![
                RevokedOwner {
                    owner_id: owner.clone(),
                    reason: None
                };
                n
            ];
            assert_eq!(validate_trust(&s).is_ok(), allowed);
            s.revoked_owners.clear();
            s.revoked_node_certs = vec![
                RevokedNodeCert {
                    cert_id: "c".into(),
                    reason: None
                };
                n
            ];
            assert_eq!(validate_trust(&s).is_ok(), allowed);
            s.revoked_node_certs.clear();
            s.revoked_node_ids = vec![
                RevokedNodeId {
                    node_endpoint_id: owner.clone(),
                    reason: None
                };
                n
            ];
            assert_eq!(validate_trust(&s).is_ok(), allowed);
            s.revoked_node_ids.clear();
        }
        s.trusted_owners = vec![TrustedOwner {
            owner_id: owner.clone(),
            label: Some("λ".repeat(129)),
        }];
        assert!(validate_trust(&s).is_err());
        s.trusted_owners[0].label = None;
        s.trusted_owners[0].owner_id = "AB".repeat(32);
        assert!(validate_trust(&s).is_err());
        s.trusted_owners.clear();
        s.revoke_owner("bad".into(), None);
        assert!(validate_trust(&s).is_err());
        s.revoked_owners.clear();
        s.revoke_node_cert("".into(), None);
        assert!(validate_trust(&s).is_err());
        s.revoked_node_certs.clear();
        s.revoke_node_id("zz".repeat(32), None);
        assert!(validate_trust(&s).is_err());
        let s = TrustStore {
            trusted_owners: vec![
                TrustedOwner {
                    owner_id: owner,
                    label: None
                };
                256
            ],
            ..TrustStore::default()
        };
        assert!(
            Mp001CertificateVerifier::new(
                Mp001TrustPin::new(
                    EndpointId([1; 32]),
                    decode32("ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c")
                        .unwrap()
                )
                .unwrap(),
                s
            )
            .is_err()
        );
    }
    #[test]
    fn grammar_edges_are_checked_before_canonical_conversion() {
        let original: serde_json::Value = serde_json::from_str(CERTIFICATE_JSON).unwrap();
        for (field, good, bad) in [
            ("cert_id", "λ".repeat(64), "λ".repeat(65)),
            ("node_label", "λ".repeat(128), "λ".repeat(129)),
            ("hostname_hint", "λ".repeat(128), "λ".repeat(129)),
            ("owner_sign_public_key", "AB".repeat(32), "AG".repeat(32)),
            ("node_endpoint_id", "AB".repeat(32), "A".repeat(63)),
            ("owner_id", "ab".repeat(32), "AB".repeat(32)),
        ] {
            let mut value = original.clone();
            value["claim"][field] = good.into();
            assert!(parse(value.to_string().as_bytes()).is_ok());
            value["claim"][field] = bad.into();
            assert!(parse(value.to_string().as_bytes()).is_err());
        }
        let mut value = original;
        value["signature"] = "AB".repeat(64).into();
        assert!(parse(value.to_string().as_bytes()).is_ok());
        value["signature"] = "AB".repeat(65).into();
        assert!(parse(value.to_string().as_bytes()).is_err());
    }
    #[test]
    fn every_retained_trust_metadata_field_is_bounded() {
        for len in [256, 257] {
            let good = len == 256;
            let mut s = TrustStore::default();
            s.add_trusted_owner("ab".repeat(32), Some("x".repeat(len)));
            assert_eq!(validate_trust(&s).is_ok(), good);
            let mut s = TrustStore::default();
            s.revoke_owner("ab".repeat(32), Some("x".repeat(len)));
            assert_eq!(validate_trust(&s).is_ok(), good);
            let mut s = TrustStore::default();
            s.revoke_node_cert("λ".repeat(64), Some("x".repeat(len)));
            assert_eq!(validate_trust(&s).is_ok(), good);
            let mut s = TrustStore::default();
            s.revoke_node_id("AB".repeat(32), Some("x".repeat(len)));
            assert_eq!(validate_trust(&s).is_ok(), good);
        }
        let key =
            decode32("ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c").unwrap();
        let mut s = TrustStore::default();
        s.add_trusted_owner(
            "fe812c12f3ab4ce6ac5db69ac352f906cb1b11ef43fb33e252ef7ff552263889".into(),
            None,
        );
        s.trusted_owners = vec![s.trusted_owners[0].clone(); 256];
        Mp001CertificateVerifier::new(Mp001TrustPin::new(EndpointId([1; 32]), key).unwrap(), s)
            .unwrap();
    }
}
