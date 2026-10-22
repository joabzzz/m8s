//! Owner-side adapter for canonical exact signing. No key loading or host IO.
use super::{Mp001TrustPin, seconds_ms};
use crate::{CertificateClaims, Error, NodeCertificate, NodeSigner, Result};
use mesh_llm_identity::OwnerKeypair;
use mesh_llm_identity::exact_node_signing::{
    ExactNodeSigningRequest, ExactSigningPolicy, sign_node_ownership_exact,
};

/// Borrows an already-custodied owner key. Never send this object to the node.
/// No Debug/Serialize/Clone, keystore access, or default lifetime policy.
/// The Store must durably burn the nonce before calling `sign`.
pub struct Mp001NodeSigner<'a> {
    owner: &'a OwnerKeypair,
    pin: &'a Mp001TrustPin,
    policy: ExactSigningPolicy,
}

impl<'a> Mp001NodeSigner<'a> {
    /// The trusted endpoint-to-signing-key pin must match the retained key.
    /// This constructor does not authorize an invoker or attest nonce consumption.
    pub fn new(
        owner: &'a OwnerKeypair,
        pin: &'a Mp001TrustPin,
        max_lifetime_seconds: u64,
        max_issuance_age_seconds: u64,
    ) -> Result<Self> {
        if owner.verifying_key().as_bytes() != &pin.key || owner.owner_id() != pin.owner_id {
            return Err(Error::Certificate);
        }
        let policy = ExactSigningPolicy::new(
            max_lifetime_seconds
                .checked_mul(1000)
                .ok_or(Error::Refused)?,
            max_issuance_age_seconds
                .checked_mul(1000)
                .ok_or(Error::Refused)?,
        )
        .map_err(|_| Error::Refused)?;
        Ok(Self { owner, pin, policy })
    }

    fn request(&self, claims: &CertificateClaims) -> Result<ExactNodeSigningRequest> {
        if claims.owner != self.pin.endpoint {
            return Err(Error::Certificate);
        }
        ExactNodeSigningRequest::new(
            claims.node.0,
            seconds_ms(claims.issued_at)?,
            seconds_ms(claims.expires_at)?,
        )
        .map_err(|_| Error::Certificate)
    }
}

impl NodeSigner for Mp001NodeSigner<'_> {
    fn sign(&mut self, claims: &CertificateClaims) -> Result<NodeCertificate> {
        let request = self.request(claims)?;
        let signed = sign_node_ownership_exact(self.owner, &request, &self.policy)
            .map_err(|_| Error::Certificate)?;
        // Serialize the canonical type; never duplicate its signed-byte encoder.
        serde_json::to_vec(&signed)
            .map(NodeCertificate)
            .map_err(|_| Error::Certificate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EndpointId;

    #[test]
    fn key_binding_and_policy_overflow_refuse_before_signing() {
        let owner = OwnerKeypair::from_bytes(&[7; 32], &[8; 32]).unwrap();
        let other = OwnerKeypair::from_bytes(&[9; 32], &[10; 32]).unwrap();
        let pin =
            Mp001TrustPin::new(EndpointId([1; 32]), *owner.verifying_key().as_bytes()).unwrap();
        assert!(Mp001NodeSigner::new(&other, &pin, 60, 2).is_err());
        for (lifetime, age) in [(0, 2), (u64::MAX, 2), (60, u64::MAX)] {
            assert!(Mp001NodeSigner::new(&owner, &pin, lifetime, age).is_err());
        }
        assert!(Mp001NodeSigner::new(&owner, &pin, 60, 0).is_ok());
    }

    #[test]
    fn exact_claim_conversion_rejects_wrong_owner_and_invalid_times() {
        let owner = OwnerKeypair::from_bytes(&[7; 32], &[8; 32]).unwrap();
        let pin =
            Mp001TrustPin::new(EndpointId([1; 32]), *owner.verifying_key().as_bytes()).unwrap();
        let signer = Mp001NodeSigner::new(&owner, &pin, 60, 2).unwrap();
        let claims = CertificateClaims {
            owner: pin.endpoint,
            node: EndpointId([2; 32]),
            issued_at: 2,
            expires_at: 10,
        };
        assert!(signer.request(&claims).is_ok());
        for (issued_at, expires_at) in [(-1, 10), (0, 10), (10, 10), (11, 10), (2, i64::MAX)] {
            assert!(
                signer
                    .request(&CertificateClaims {
                        issued_at,
                        expires_at,
                        ..claims.clone()
                    })
                    .is_err()
            );
        }
        assert!(
            signer
                .request(&CertificateClaims {
                    owner: EndpointId([3; 32]),
                    ..claims
                })
                .is_err()
        );
    }
}
