use std::path::{Path, PathBuf};

use super::keystore::write_keystore_bytes_atomically;
use super::node_certificate::canonical_claim_bytes;
pub use super::node_certificate::{
    DEFAULT_NODE_CERT_LIFETIME_SECS, DEFAULT_NODE_CERT_RENEW_WINDOW_SECS, NODE_OWNERSHIP_VERSION,
    NodeOwnershipClaim, OwnershipStatus, OwnershipSummary, RevokedNodeCert, RevokedNodeId,
    RevokedOwner, SignedNodeOwnership, TRUST_STORE_VERSION, TrustPolicy, TrustStore, TrustedOwner,
    certificate_needs_renewal, verify_node_ownership,
};
use super::{CryptoError, OwnerKeypair};

fn mesh_dir() -> Result<PathBuf, CryptoError> {
    let home = dirs::home_dir().ok_or_else(|| {
        CryptoError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "cannot determine home directory",
        ))
    })?;
    Ok(home.join(".mesh-llm"))
}

pub fn default_node_ownership_path() -> Result<PathBuf, CryptoError> {
    if let Some(path) = std::env::var_os("MESH_LLM_NODE_OWNERSHIP_PATH").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    Ok(mesh_dir()?.join("node-ownership.json"))
}

pub fn default_trust_store_path() -> Result<PathBuf, CryptoError> {
    Ok(mesh_dir()?.join("trusted-owners.json"))
}

pub fn load_node_ownership(path: &Path) -> Result<SignedNodeOwnership, CryptoError> {
    let raw = std::fs::read_to_string(path)?;
    serde_json::from_str(&raw).map_err(Into::into)
}

pub fn save_node_ownership(
    path: &Path,
    ownership: &SignedNodeOwnership,
) -> Result<(), CryptoError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(ownership)?;
    write_keystore_bytes_atomically(path, &bytes)?;
    Ok(())
}

pub fn load_trust_store(path: &Path) -> Result<TrustStore, CryptoError> {
    if !path.exists() {
        return Ok(TrustStore::default());
    }
    let raw = std::fs::read_to_string(path)?;
    let store: TrustStore = serde_json::from_str(&raw)?;
    if store.version != TRUST_STORE_VERSION {
        return Err(CryptoError::UnsupportedVersion {
            version: store.version,
        });
    }
    Ok(store)
}

pub fn save_trust_store(path: &Path, store: &TrustStore) -> Result<(), CryptoError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(store)?;
    write_keystore_bytes_atomically(path, &bytes)?;
    Ok(())
}

fn current_time_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn random_cert_id() -> String {
    let bytes: [u8; 16] = rand::random();
    hex::encode(bytes)
}

pub fn sign_node_ownership(
    owner: &OwnerKeypair,
    node_endpoint_id: &[u8; 32],
    expires_at_unix_ms: u64,
    node_label: Option<String>,
    hostname_hint: Option<String>,
) -> Result<SignedNodeOwnership, CryptoError> {
    let claim = NodeOwnershipClaim {
        version: NODE_OWNERSHIP_VERSION,
        cert_id: random_cert_id(),
        owner_id: owner.owner_id(),
        owner_sign_public_key: hex::encode(owner.verifying_key().as_bytes()),
        node_endpoint_id: hex::encode(node_endpoint_id),
        issued_at_unix_ms: current_time_unix_ms(),
        expires_at_unix_ms,
        node_label,
        hostname_hint,
    };
    let bytes = canonical_claim_bytes(&claim)?;
    let signature = owner.sign_bytes(&bytes);
    Ok(SignedNodeOwnership {
        claim,
        signature: hex::encode(signature),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_ownership_verifies() {
        let owner = OwnerKeypair::generate();
        let node_endpoint_id = [0x42; 32];
        let ownership = sign_node_ownership(
            &owner,
            &node_endpoint_id,
            current_time_unix_ms() + 60_000,
            Some("studio".into()),
            Some("studio-host".into()),
        )
        .unwrap();

        let summary = verify_node_ownership(
            Some(&ownership),
            &node_endpoint_id,
            &TrustStore::default(),
            TrustPolicy::Off,
            current_time_unix_ms(),
        );

        assert_eq!(summary.status, OwnershipStatus::Verified);
        assert!(summary.verified);
        assert_eq!(summary.node_label.as_deref(), Some("studio"));
    }

    #[test]
    fn mismatched_node_id_is_rejected() {
        let owner = OwnerKeypair::generate();
        let node_endpoint_id = [0x42; 32];
        let wrong_node_endpoint_id = [0x24; 32];
        let ownership = sign_node_ownership(
            &owner,
            &node_endpoint_id,
            current_time_unix_ms() + 60_000,
            None,
            None,
        )
        .unwrap();

        let summary = verify_node_ownership(
            Some(&ownership),
            &wrong_node_endpoint_id,
            &TrustStore::default(),
            TrustPolicy::Off,
            current_time_unix_ms(),
        );

        assert_eq!(summary.status, OwnershipStatus::MismatchedNodeId);
        assert!(!summary.verified);
    }

    #[test]
    fn revoked_owner_is_rejected() {
        let owner = OwnerKeypair::generate();
        let node_endpoint_id = [0x42; 32];
        let ownership = sign_node_ownership(
            &owner,
            &node_endpoint_id,
            current_time_unix_ms() + 60_000,
            None,
            None,
        )
        .unwrap();
        let mut trust_store = TrustStore::default();
        trust_store.revoke_owner(owner.owner_id(), Some("compromised".into()));

        let summary = verify_node_ownership(
            Some(&ownership),
            &node_endpoint_id,
            &trust_store,
            TrustPolicy::Off,
            current_time_unix_ms(),
        );

        assert_eq!(summary.status, OwnershipStatus::RevokedOwner);
    }

    #[test]
    fn allowlist_requires_trusted_owner() {
        let owner = OwnerKeypair::generate();
        let node_endpoint_id = [0x42; 32];
        let ownership = sign_node_ownership(
            &owner,
            &node_endpoint_id,
            current_time_unix_ms() + 60_000,
            None,
            None,
        )
        .unwrap();

        let summary = verify_node_ownership(
            Some(&ownership),
            &node_endpoint_id,
            &TrustStore::default(),
            TrustPolicy::Allowlist,
            current_time_unix_ms(),
        );

        assert_eq!(summary.status, OwnershipStatus::UntrustedOwner);
    }

    #[test]
    fn unsupported_claim_version_is_rejected() {
        let owner = OwnerKeypair::generate();
        let node_endpoint_id = [0x42; 32];
        let mut ownership = sign_node_ownership(
            &owner,
            &node_endpoint_id,
            current_time_unix_ms() + 60_000,
            None,
            None,
        )
        .unwrap();
        ownership.claim.version = NODE_OWNERSHIP_VERSION + 1;

        let summary = verify_node_ownership(
            Some(&ownership),
            &node_endpoint_id,
            &TrustStore::default(),
            TrustPolicy::Off,
            current_time_unix_ms(),
        );

        assert_eq!(summary.status, OwnershipStatus::UnsupportedProtocol);
        assert!(!summary.verified);
    }
}
