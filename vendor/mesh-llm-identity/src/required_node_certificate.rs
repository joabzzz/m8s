//! Fail-closed verification of a pre-signed node certificate that startup requires.
//!
//! Unlike [`verify_node_ownership`], which reports a status and lets legacy
//! callers fall back to `Unsigned`, this entry point returns an error for every
//! outcome except a certificate that is cryptographically valid for the actual
//! node, currently valid, not revoked, and issued by an owner on the caller's
//! explicit allowlist. A supplied certificate never approves its own owner.
//! Pure: no IO, key loading, or clock access; the caller supplies `now`.

use crate::node_certificate::{
    OwnershipStatus, SignedNodeOwnership, TrustPolicy, TrustStore, verify_node_ownership,
};

/// Fixed failure categories; certificate contents are not echoed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequiredCertificateError {
    #[error("required node ownership certificate is absent")]
    Absent,
    #[error("required node ownership certificate rejected: {0:?}")]
    Rejected(OwnershipStatus),
    #[error("required node ownership certificate is not yet valid")]
    NotYetValid,
    #[error("required node ownership certificate has an invalid validity window")]
    InvalidValidityWindow,
}

/// Verify `ownership` for `actual_node_endpoint_id` against the allowlist in
/// `trust_store`, rejecting anything short of a fully verified trusted owner.
pub fn verify_required_node_ownership(
    ownership: Option<&SignedNodeOwnership>,
    actual_node_endpoint_id: &[u8; 32],
    trust_store: &TrustStore,
    now_unix_ms: u64,
) -> Result<(), RequiredCertificateError> {
    let ownership = ownership.ok_or(RequiredCertificateError::Absent)?;
    let summary = verify_node_ownership(
        Some(ownership),
        actual_node_endpoint_id,
        trust_store,
        TrustPolicy::Allowlist,
        now_unix_ms,
    );
    if !summary.verified || summary.status != OwnershipStatus::Verified {
        return Err(RequiredCertificateError::Rejected(summary.status));
    }
    let claim = &ownership.claim;
    if claim.expires_at_unix_ms <= claim.issued_at_unix_ms {
        return Err(RequiredCertificateError::InvalidValidityWindow);
    }
    if claim.issued_at_unix_ms > now_unix_ms {
        return Err(RequiredCertificateError::NotYetValid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OwnerKeypair;
    use crate::exact_node_signing::{
        ExactNodeSigningRequest, ExactSigningPolicy, sign_node_ownership_exact,
    };
    use crate::node_certificate::NODE_OWNERSHIP_VERSION;

    const NODE: [u8; 32] = [0x42; 32];
    const NOW: u64 = 5_000;

    fn owner(seed: u8) -> OwnerKeypair {
        OwnerKeypair::from_bytes(&[seed; 32], &[seed.wrapping_add(1); 32]).unwrap()
    }

    fn sign(
        owner: &OwnerKeypair,
        node: [u8; 32],
        issued: u64,
        expires: u64,
    ) -> SignedNodeOwnership {
        // Signs through the one canonical encoder at explicit synthetic times.
        raw_sign(owner, node, issued, expires)
    }

    fn raw_sign(
        owner: &OwnerKeypair,
        node: [u8; 32],
        issued: u64,
        expires: u64,
    ) -> SignedNodeOwnership {
        let claim = crate::node_certificate::NodeOwnershipClaim {
            version: NODE_OWNERSHIP_VERSION,
            cert_id: crate::node_certificate::random_cert_id(),
            owner_id: owner.owner_id(),
            owner_sign_public_key: hex::encode(owner.verifying_key().as_bytes()),
            node_endpoint_id: hex::encode(node),
            issued_at_unix_ms: issued,
            expires_at_unix_ms: expires,
            node_label: None,
            hostname_hint: None,
        };
        let bytes = crate::node_certificate::canonical_claim_bytes(&claim).unwrap();
        SignedNodeOwnership {
            signature: hex::encode(owner.sign_bytes(&bytes)),
            claim,
        }
    }

    fn trusting(owner: &OwnerKeypair) -> TrustStore {
        let mut store = TrustStore::default();
        store.add_trusted_owner(owner.owner_id(), None);
        store
    }

    fn check(
        cert: Option<&SignedNodeOwnership>,
        node: &[u8; 32],
        store: &TrustStore,
    ) -> Result<(), RequiredCertificateError> {
        verify_required_node_ownership(cert, node, store, NOW)
    }

    #[test]
    fn valid_certificate_from_allowlisted_owner_is_accepted() {
        let o = owner(1);
        let cert = sign(&o, NODE, 2_000, 10_000);
        assert_eq!(check(Some(&cert), &NODE, &trusting(&o)), Ok(()));
    }

    #[test]
    fn exact_signer_output_is_accepted_when_current() {
        let o = owner(1);
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let (issued, expires) = (secs * 1000, secs * 1000 + 60_000);
        let request = ExactNodeSigningRequest::new(NODE, issued, expires).unwrap();
        let policy = ExactSigningPolicy::new(60_000, 30_000).unwrap();
        let cert = sign_node_ownership_exact(&o, &request, &policy).unwrap();
        assert_eq!(
            verify_required_node_ownership(Some(&cert), &NODE, &trusting(&o), issued + 1),
            Ok(())
        );
    }

    #[test]
    fn absent_certificate_is_an_error() {
        assert_eq!(
            check(None, &NODE, &TrustStore::default()),
            Err(RequiredCertificateError::Absent)
        );
    }

    #[test]
    fn untrusted_or_self_approved_owner_is_rejected() {
        let (o, other) = (owner(1), owner(3));
        let cert = sign(&o, NODE, 2_000, 10_000);
        for store in [TrustStore::default(), trusting(&other)] {
            assert_eq!(
                check(Some(&cert), &NODE, &store),
                Err(RequiredCertificateError::Rejected(
                    OwnershipStatus::UntrustedOwner
                ))
            );
        }
        // A non-allowlist store policy flag does not weaken the check.
        let store = TrustStore {
            policy: TrustPolicy::Off,
            ..TrustStore::default()
        };
        assert!(check(Some(&cert), &NODE, &store).is_err());
    }

    #[test]
    fn wrong_node_tamper_and_bad_signature_are_rejected() {
        let o = owner(1);
        let store = trusting(&o);
        let cert = sign(&o, NODE, 2_000, 10_000);
        assert_eq!(
            check(Some(&cert), &[0x24; 32], &store),
            Err(RequiredCertificateError::Rejected(
                OwnershipStatus::MismatchedNodeId
            ))
        );
        let mut tampered = cert.clone();
        tampered.claim.expires_at_unix_ms += 1_000;
        assert_eq!(
            check(Some(&tampered), &NODE, &store),
            Err(RequiredCertificateError::Rejected(
                OwnershipStatus::InvalidSignature
            ))
        );
        let mut garbage = cert.clone();
        garbage.signature = "zz".into();
        assert!(check(Some(&garbage), &NODE, &store).is_err());
        let mut versioned = cert;
        versioned.claim.version += 1;
        assert_eq!(
            check(Some(&versioned), &NODE, &store),
            Err(RequiredCertificateError::Rejected(
                OwnershipStatus::UnsupportedProtocol
            ))
        );
    }

    #[test]
    fn expired_future_and_inverted_windows_are_rejected() {
        let o = owner(1);
        let store = trusting(&o);
        assert_eq!(
            check(Some(&sign(&o, NODE, 1_000, NOW)), &NODE, &store),
            Err(RequiredCertificateError::Rejected(OwnershipStatus::Expired))
        );
        assert_eq!(
            check(Some(&sign(&o, NODE, NOW + 1, NOW + 9_000)), &NODE, &store),
            Err(RequiredCertificateError::NotYetValid)
        );
        assert_eq!(
            check(Some(&sign(&o, NODE, 9_000, 8_000)), &NODE, &store),
            Err(RequiredCertificateError::InvalidValidityWindow)
        );
        assert_eq!(
            check(Some(&sign(&o, NODE, 9_000, 9_000)), &NODE, &store),
            Err(RequiredCertificateError::InvalidValidityWindow)
        );
    }

    #[test]
    fn revocations_override_the_allowlist() {
        let o = owner(1);
        let cert = sign(&o, NODE, 2_000, 10_000);
        let revoked = |f: fn(&mut TrustStore, &SignedNodeOwnership)| {
            let mut store = trusting(&o);
            f(&mut store, &cert);
            check(Some(&cert), &NODE, &store)
        };
        assert_eq!(
            revoked(|s, c| s.revoke_owner(c.claim.owner_id.clone(), None)),
            Err(RequiredCertificateError::Rejected(
                OwnershipStatus::RevokedOwner
            ))
        );
        assert_eq!(
            revoked(|s, c| s.revoke_node_cert(c.claim.cert_id.clone(), None)),
            Err(RequiredCertificateError::Rejected(
                OwnershipStatus::RevokedCert
            ))
        );
        assert_eq!(
            revoked(|s, c| s.revoke_node_id(c.claim.node_endpoint_id.clone(), None)),
            Err(RequiredCertificateError::Rejected(
                OwnershipStatus::RevokedNodeId
            ))
        );
    }
}
