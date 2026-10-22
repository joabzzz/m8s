//! Public node ownership certificates and pure trust/revocation verification.

use serde::{Deserialize, Serialize};

use super::{CryptoError, owner_id_from_verifying_key};

pub const NODE_OWNERSHIP_VERSION: u32 = 1;
pub const TRUST_STORE_VERSION: u32 = 1;
pub const DEFAULT_NODE_CERT_LIFETIME_SECS: u64 = 7 * 24 * 60 * 60;
pub const DEFAULT_NODE_CERT_RENEW_WINDOW_SECS: u64 = 36 * 60 * 60;
const SIGNING_DOMAIN_TAG: &[u8] = b"mesh-llm-node-ownership-v1:";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TrustPolicy {
    #[default]
    Off,
    PreferOwned,
    RequireOwned,
    Allowlist,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeOwnershipClaim {
    pub version: u32,
    pub cert_id: String,
    pub owner_id: String,
    pub owner_sign_public_key: String,
    pub node_endpoint_id: String,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedNodeOwnership {
    pub claim: NodeOwnershipClaim,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedOwner {
    pub owner_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevokedOwner {
    pub owner_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevokedNodeCert {
    pub cert_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevokedNodeId {
    pub node_endpoint_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustStore {
    pub version: u32,
    pub policy: TrustPolicy,
    #[serde(default)]
    pub trusted_owners: Vec<TrustedOwner>,
    #[serde(default)]
    pub revoked_owners: Vec<RevokedOwner>,
    #[serde(default)]
    pub revoked_node_certs: Vec<RevokedNodeCert>,
    #[serde(default)]
    pub revoked_node_ids: Vec<RevokedNodeId>,
}

impl Default for TrustStore {
    fn default() -> Self {
        Self {
            version: TRUST_STORE_VERSION,
            policy: TrustPolicy::Off,
            trusted_owners: Vec::new(),
            revoked_owners: Vec::new(),
            revoked_node_certs: Vec::new(),
            revoked_node_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipStatus {
    Verified,
    #[default]
    Unsigned,
    Expired,
    InvalidSignature,
    MismatchedNodeId,
    RevokedOwner,
    RevokedCert,
    RevokedNodeId,
    UnsupportedProtocol,
    UntrustedOwner,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OwnershipSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert_id: Option<String>,
    pub status: OwnershipStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname_hint: Option<String>,
    pub verified: bool,
}

fn decode_hex_32(label: &str, value: &str) -> Result<[u8; 32], CryptoError> {
    let decoded = hex::decode(value).map_err(|e| CryptoError::InvalidKeyMaterial {
        reason: format!("bad {label} hex: {e}"),
    })?;
    decoded
        .try_into()
        .map_err(|_| CryptoError::InvalidKeyMaterial {
            reason: format!("{label} must be 32 bytes"),
        })
}

fn write_string(buf: &mut Vec<u8>, value: &str) {
    buf.extend_from_slice(&(value.len() as u64).to_le_bytes());
    buf.extend_from_slice(value.as_bytes());
}

fn write_optional_string(buf: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => {
            buf.push(1);
            write_string(buf, value);
        }
        None => buf.push(0),
    }
}

pub(crate) fn canonical_claim_bytes(claim: &NodeOwnershipClaim) -> Result<Vec<u8>, CryptoError> {
    let owner_sign_public_key =
        decode_hex_32("owner_sign_public_key", &claim.owner_sign_public_key)?;
    let node_endpoint_id = decode_hex_32("node_endpoint_id", &claim.node_endpoint_id)?;
    let mut buf = Vec::with_capacity(256);
    buf.extend_from_slice(SIGNING_DOMAIN_TAG);
    buf.extend_from_slice(&claim.version.to_le_bytes());
    write_string(&mut buf, &claim.cert_id);
    write_string(&mut buf, &claim.owner_id);
    buf.extend_from_slice(&owner_sign_public_key);
    buf.extend_from_slice(&node_endpoint_id);
    buf.extend_from_slice(&claim.issued_at_unix_ms.to_le_bytes());
    buf.extend_from_slice(&claim.expires_at_unix_ms.to_le_bytes());
    write_optional_string(&mut buf, claim.node_label.as_deref());
    write_optional_string(&mut buf, claim.hostname_hint.as_deref());
    Ok(buf)
}

pub fn certificate_needs_renewal(
    ownership: &SignedNodeOwnership,
    renew_window_secs: u64,
    now_unix_ms: u64,
) -> bool {
    if ownership.claim.expires_at_unix_ms <= now_unix_ms {
        return true;
    }
    let remaining_ms = ownership
        .claim
        .expires_at_unix_ms
        .saturating_sub(now_unix_ms);
    remaining_ms <= renew_window_secs.saturating_mul(1000)
}

pub fn verify_node_ownership(
    ownership: Option<&SignedNodeOwnership>,
    actual_node_endpoint_id: &[u8; 32],
    trust_store: &TrustStore,
    policy: TrustPolicy,
    now_unix_ms: u64,
) -> OwnershipSummary {
    let Some(ownership) = ownership else {
        return OwnershipSummary::default();
    };

    let mut summary = OwnershipSummary {
        owner_id: Some(ownership.claim.owner_id.clone()),
        cert_id: Some(ownership.claim.cert_id.clone()),
        expires_at_unix_ms: Some(ownership.claim.expires_at_unix_ms),
        node_label: ownership.claim.node_label.clone(),
        hostname_hint: ownership.claim.hostname_hint.clone(),
        ..OwnershipSummary::default()
    };
    if ownership.claim.version != NODE_OWNERSHIP_VERSION {
        summary.status = OwnershipStatus::UnsupportedProtocol;
        return summary;
    }

    let owner_sign_public_key = match decode_hex_32(
        "owner_sign_public_key",
        &ownership.claim.owner_sign_public_key,
    ) {
        Ok(bytes) => bytes,
        Err(_) => {
            summary.status = OwnershipStatus::InvalidSignature;
            return summary;
        }
    };
    let node_endpoint_id =
        match decode_hex_32("node_endpoint_id", &ownership.claim.node_endpoint_id) {
            Ok(bytes) => bytes,
            Err(_) => {
                summary.status = OwnershipStatus::InvalidSignature;
                return summary;
            }
        };

    if node_endpoint_id != *actual_node_endpoint_id {
        summary.status = OwnershipStatus::MismatchedNodeId;
        return summary;
    }

    let verifying_key = match ed25519_dalek::VerifyingKey::from_bytes(&owner_sign_public_key) {
        Ok(value) => value,
        Err(_) => {
            summary.status = OwnershipStatus::InvalidSignature;
            return summary;
        }
    };
    let expected_owner_id = owner_id_from_verifying_key(&verifying_key);
    if expected_owner_id != ownership.claim.owner_id {
        summary.status = OwnershipStatus::InvalidSignature;
        return summary;
    }

    let canonical = match canonical_claim_bytes(&ownership.claim) {
        Ok(value) => value,
        Err(_) => {
            summary.status = OwnershipStatus::InvalidSignature;
            return summary;
        }
    };
    let sig_bytes = match hex::decode(&ownership.signature) {
        Ok(value) => value,
        Err(_) => {
            summary.status = OwnershipStatus::InvalidSignature;
            return summary;
        }
    };
    let sig_bytes: [u8; 64] = match sig_bytes.try_into() {
        Ok(value) => value,
        Err(_) => {
            summary.status = OwnershipStatus::InvalidSignature;
            return summary;
        }
    };
    let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
    if verifying_key.verify_strict(&canonical, &signature).is_err() {
        summary.status = OwnershipStatus::InvalidSignature;
        return summary;
    }

    if ownership.claim.expires_at_unix_ms <= now_unix_ms {
        summary.status = OwnershipStatus::Expired;
        return summary;
    }

    if trust_store
        .revoked_owners
        .iter()
        .any(|entry| entry.owner_id == ownership.claim.owner_id)
    {
        summary.status = OwnershipStatus::RevokedOwner;
        return summary;
    }

    if trust_store
        .revoked_node_certs
        .iter()
        .any(|entry| entry.cert_id == ownership.claim.cert_id)
    {
        summary.status = OwnershipStatus::RevokedCert;
        return summary;
    }

    if trust_store.revoked_node_ids.iter().any(|entry| {
        entry
            .node_endpoint_id
            .eq_ignore_ascii_case(&ownership.claim.node_endpoint_id)
    }) {
        summary.status = OwnershipStatus::RevokedNodeId;
        return summary;
    }

    if matches!(policy, TrustPolicy::Allowlist)
        && !trust_store
            .trusted_owners
            .iter()
            .any(|entry| entry.owner_id == ownership.claim.owner_id)
    {
        summary.status = OwnershipStatus::UntrustedOwner;
        return summary;
    }

    summary.status = OwnershipStatus::Verified;
    summary.verified = true;
    summary
}

impl TrustStore {
    pub fn merged_with_trusted_owners(mut self, owners: &[String]) -> Self {
        for owner_id in owners {
            self.add_trusted_owner(owner_id.clone(), None);
        }
        self
    }

    pub fn add_trusted_owner(&mut self, owner_id: String, label: Option<String>) {
        if let Some(existing) = self
            .trusted_owners
            .iter_mut()
            .find(|entry| entry.owner_id == owner_id)
        {
            if label.is_some() {
                existing.label = label;
            }
            return;
        }
        self.trusted_owners.push(TrustedOwner { owner_id, label });
        self.trusted_owners
            .sort_by(|a, b| a.owner_id.cmp(&b.owner_id));
    }

    pub fn remove_trusted_owner(&mut self, owner_id: &str) -> bool {
        let before = self.trusted_owners.len();
        self.trusted_owners
            .retain(|entry| entry.owner_id != owner_id);
        before != self.trusted_owners.len()
    }

    pub fn revoke_owner(&mut self, owner_id: String, reason: Option<String>) {
        if self
            .revoked_owners
            .iter()
            .any(|entry| entry.owner_id == owner_id)
        {
            return;
        }
        self.revoked_owners.push(RevokedOwner { owner_id, reason });
        self.revoked_owners
            .sort_by(|a, b| a.owner_id.cmp(&b.owner_id));
    }

    pub fn revoke_node_cert(&mut self, cert_id: String, reason: Option<String>) {
        if self
            .revoked_node_certs
            .iter()
            .any(|entry| entry.cert_id == cert_id)
        {
            return;
        }
        self.revoked_node_certs
            .push(RevokedNodeCert { cert_id, reason });
        self.revoked_node_certs
            .sort_by(|a, b| a.cert_id.cmp(&b.cert_id));
    }

    pub fn revoke_node_id(&mut self, node_endpoint_id: String, reason: Option<String>) {
        if self.revoked_node_ids.iter().any(|entry| {
            entry
                .node_endpoint_id
                .eq_ignore_ascii_case(&node_endpoint_id)
        }) {
            return;
        }
        self.revoked_node_ids.push(RevokedNodeId {
            node_endpoint_id,
            reason,
        });
        self.revoked_node_ids
            .sort_by(|a, b| a.node_endpoint_id.cmp(&b.node_endpoint_id));
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;

    // Fixed synthetic seed, never an ambient owner key. The public fixture pins
    // the cc08962a encoder's exact bytes, not a second implementation of it.
    #[test]
    fn canonical_public_vector() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let claim = NodeOwnershipClaim {
            version: 1,
            cert_id: "synthetic-cert".into(),
            owner_id: owner_id_from_verifying_key(&key.verifying_key()),
            owner_sign_public_key: hex::encode(key.verifying_key().as_bytes()),
            node_endpoint_id: hex::encode([0x42; 32]),
            issued_at_unix_ms: 2_000,
            expires_at_unix_ms: 10_000,
            node_label: Some("studio-λ".into()),
            hostname_hint: None,
        };
        let canonical = canonical_claim_bytes(&claim).unwrap();
        let certificate = SignedNodeOwnership {
            claim,
            signature: hex::encode(key.sign(&canonical).to_bytes()),
        };
        assert_eq!(
            hex::encode(&canonical),
            "6d6573682d6c6c6d2d6e6f64652d6f776e6572736869702d76313a010000000e0000000000000073796e7468657469632d63657274400000000000000066653831326331326633616234636536616335646236396163333532663930366362316231316566343366623333653235326566376666353532323633383839ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c4242424242424242424242424242424242424242424242424242424242424242d007000000000000102700000000000001090000000000000073747564696f2dcebb00"
        );
        assert_eq!(
            certificate.signature,
            "f2386312948ff46ee62cc272d744cd49dfa6faaa9a09bb9e4d58222b7a83696fae6c38a284e9901821b2c016c3486e2e1d6b72fdd7f9050429555bff5b1d8502"
        );
    }

    #[test]
    fn canonical_optional_vector() {
        let key = SigningKey::from_bytes(&[8; 32]);
        let claim = NodeOwnershipClaim {
            version: 1,
            cert_id: "synthetic-optional".into(),
            owner_id: owner_id_from_verifying_key(&key.verifying_key()),
            owner_sign_public_key: hex::encode(key.verifying_key().as_bytes()),
            node_endpoint_id: hex::encode([0xab; 32]),
            issued_at_unix_ms: 2_000,
            expires_at_unix_ms: 10_000,
            node_label: None,
            hostname_hint: Some(String::new()),
        };
        let canonical = canonical_claim_bytes(&claim).unwrap();
        let certificate = SignedNodeOwnership {
            claim,
            signature: hex::encode(key.sign(&canonical).to_bytes()),
        };
        assert_eq!(
            hex::encode(&canonical),
            "6d6573682d6c6c6d2d6e6f64652d6f776e6572736869702d76313a01000000120000000000000073796e7468657469632d6f7074696f6e616c4000000000000000356332396237386631306133356134396136323331643038656538343061303462636333613337613466396434366466333237646563353662386139623763391398f62c6d1a457c51ba6a4b5f3dbd2f69fca93216218dc8997e416bd17d93caababababababababababababababababababababababababababababababababd007000000000000102700000000000000010000000000000000"
        );
        assert_eq!(
            certificate.signature,
            "0ffaf90b34e267b8e1243d309de4eecb457005ea9b8bc97ed5a5d4d53b9ca162b2654981bc80f675cea3e75566648ef3bf7ac766b284025fe9dbdeb188b5190a"
        );
    }
}
