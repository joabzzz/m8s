//! Exact-time signing mechanics, not invoker authorization or nonce-consumption proof.
//!
//! Pure and available without `host-io`; also re-exported at the legacy
//! `ownership::exact_request` path. No key loading or IO is performed here; the
//! caller already holds a mesh-owned in-memory owner key.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::OwnerKeypair;
use crate::node_certificate::{
    NODE_OWNERSHIP_VERSION, NodeOwnershipClaim, SignedNodeOwnership, canonical_claim_bytes,
    random_cert_id,
};

/// Fixed failure categories; no request, key, or path data is exposed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExactSigningError {
    #[error("invalid exact signing request")]
    InvalidRequest,
    #[error("invalid exact signing policy")]
    InvalidPolicy,
    #[error("exact signing time refused")]
    TimeRefused,
    #[error("exact signing failed")]
    SigningFailed,
}

type Result<T> = std::result::Result<T, ExactSigningError>;

/// Trusted configuration, not remote authorization or evidence of Admin approval.
/// No defaults are supplied; numerical limits and invoker approval are owner decisions.
#[derive(Debug)]
pub struct ExactSigningPolicy {
    max_lifetime_ms: u64,
    max_issuance_age_ms: u64,
}

impl ExactSigningPolicy {
    /// Explicit finite millisecond limits. Zero age is allowed and never widened;
    /// it requires an observation exactly at issuance, including sub-ms precision.
    pub fn new(max_lifetime_ms: u64, max_issuance_age_ms: u64) -> Result<Self> {
        if max_lifetime_ms == 0
            || UNIX_EPOCH
                .checked_add(Duration::from_millis(max_lifetime_ms))
                .is_none()
            || UNIX_EPOCH
                .checked_add(Duration::from_millis(max_issuance_age_ms))
                .is_none()
        {
            return Err(ExactSigningError::InvalidPolicy);
        }
        Ok(Self {
            max_lifetime_ms,
            max_issuance_age_ms,
        })
    }
}

/// Exact public node and timestamps only: no key, owner, path, metadata or policy overrides.
#[derive(Debug)]
pub struct ExactNodeSigningRequest {
    node_endpoint_id: [u8; 32],
    issued_at_unix_ms: u64,
    expires_at_unix_ms: u64,
}

impl ExactNodeSigningRequest {
    /// Requires nonzero, ordered, whole-second timestamps represented in milliseconds.
    /// Seconds-to-milliseconds conversion, if needed, must be checked by the caller.
    pub fn new(
        node_endpoint_id: [u8; 32],
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
    ) -> Result<Self> {
        let request = Self {
            node_endpoint_id,
            issued_at_unix_ms,
            expires_at_unix_ms,
        };
        request.lifetime_ms()?;
        Ok(request)
    }

    fn lifetime_ms(&self) -> Result<u64> {
        if self.issued_at_unix_ms == 0
            || !self.issued_at_unix_ms.is_multiple_of(1000)
            || !self.expires_at_unix_ms.is_multiple_of(1000)
            || UNIX_EPOCH
                .checked_add(Duration::from_millis(self.expires_at_unix_ms))
                .is_none()
        {
            return Err(ExactSigningError::InvalidRequest);
        }
        self.expires_at_unix_ms
            .checked_sub(self.issued_at_unix_ms)
            .filter(|lifetime| *lifetime > 0)
            .ok_or(ExactSigningError::InvalidRequest)
    }
}

fn validate(
    request: &ExactNodeSigningRequest,
    policy: &ExactSigningPolicy,
    now: SystemTime,
) -> Result<()> {
    if request.lifetime_ms()? > policy.max_lifetime_ms {
        return Err(ExactSigningError::InvalidRequest);
    }
    let elapsed = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ExactSigningError::TimeRefused)?;
    // Refuse an out-of-range clock rather than truncating or saturating it.
    u64::try_from(elapsed.as_millis()).map_err(|_| ExactSigningError::TimeRefused)?;
    let issued = Duration::from_millis(request.issued_at_unix_ms);
    let age = elapsed
        .checked_sub(issued)
        .ok_or(ExactSigningError::TimeRefused)?;
    if elapsed >= Duration::from_millis(request.expires_at_unix_ms)
        || age > Duration::from_millis(policy.max_issuance_age_ms)
    {
        return Err(ExactSigningError::TimeRefused);
    }
    Ok(())
}

/// Optional real-clock preflight for a future owner integration *before* key access.
/// Returns no authority token; signing always rechecks. This cannot prove whether
/// a caller has already loaded a key. There is no wired CLI or machine route.
pub fn preflight_exact_node_ownership(
    request: &ExactNodeSigningRequest,
    policy: &ExactSigningPolicy,
) -> Result<()> {
    validate(request, policy, SystemTime::now())
}

/// Sign exact requested times or refuse, using fresh real entry/pre/post clocks.
///
/// The owner key is already in memory. A future invocation must preflight before
/// loading it, then call this rechecking entry. Post-sign failure discards the
/// response, but cannot undo signing. Scheduling cannot be preempted and clock
/// jumps between observations cannot all be detected. No nonce ledger or dedupe
/// exists here; repeated successful invocations mint independent certificates.
pub fn sign_node_ownership_exact(
    owner: &OwnerKeypair,
    request: &ExactNodeSigningRequest,
    policy: &ExactSigningPolicy,
) -> Result<SignedNodeOwnership> {
    sign_with_clock(owner, request, policy, SystemTime::now)
}

// Same production engine, private deterministic observation seam only for tests.
fn sign_with_clock(
    owner: &OwnerKeypair,
    request: &ExactNodeSigningRequest,
    policy: &ExactSigningPolicy,
    mut clock: impl FnMut() -> SystemTime,
) -> Result<SignedNodeOwnership> {
    let entry = clock();
    validate(request, policy, entry)?;
    let claim = NodeOwnershipClaim {
        version: NODE_OWNERSHIP_VERSION,
        cert_id: random_cert_id(),
        owner_id: owner.owner_id(),
        owner_sign_public_key: hex::encode(owner.verifying_key().as_bytes()),
        node_endpoint_id: hex::encode(request.node_endpoint_id),
        issued_at_unix_ms: request.issued_at_unix_ms,
        expires_at_unix_ms: request.expires_at_unix_ms,
        node_label: None,
        hostname_hint: None,
    };
    let bytes = canonical_claim_bytes(&claim).map_err(|_| ExactSigningError::SigningFailed)?;
    let pre_sign = clock();
    check_observation(request, policy, entry, pre_sign)?;
    let signature = owner.sign_bytes(&bytes);
    let post_sign = clock();
    check_observation(request, policy, pre_sign, post_sign)?;
    Ok(SignedNodeOwnership {
        claim,
        signature: hex::encode(signature),
    })
}

fn check_observation(
    request: &ExactNodeSigningRequest,
    policy: &ExactSigningPolicy,
    previous: SystemTime,
    now: SystemTime,
) -> Result<()> {
    if now < previous {
        return Err(ExactSigningError::TimeRefused);
    }
    validate(request, policy, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_certificate::{TrustPolicy, TrustStore, verify_node_ownership};

    // All limits below are TEST VALUES, never a production profile recommendation.
    const NODE: [u8; 32] = [0x42; 32];

    fn owner() -> OwnerKeypair {
        OwnerKeypair::from_bytes(&[7; 32], &[8; 32]).unwrap()
    }

    fn request() -> ExactNodeSigningRequest {
        ExactNodeSigningRequest::new(NODE, 2_000, 10_000).unwrap()
    }

    fn at(ms: u64) -> SystemTime {
        UNIX_EPOCH.checked_add(Duration::from_millis(ms)).unwrap()
    }

    fn sign_at(times: [u64; 3], age: u64) -> Result<SignedNodeOwnership> {
        let mut times = times.into_iter();
        sign_with_clock(
            &owner(),
            &request(),
            &ExactSigningPolicy::new(8_000, age).unwrap(),
            || at(times.next().expect("unexpected clock observation")),
        )
    }

    #[test]
    fn exact_canonical_crypto_and_repeated_requests() {
        let certificate = sign_at([2_999, 3_500, 3_500], 1_500).unwrap();
        assert_eq!(certificate.claim.node_endpoint_id, hex::encode(NODE));
        assert_eq!(certificate.claim.issued_at_unix_ms, 2_000);
        assert_eq!(certificate.claim.expires_at_unix_ms, 10_000);
        assert_eq!(certificate.claim.owner_id, owner().owner_id());
        assert_eq!(certificate.claim.node_label, None);
        assert_eq!(certificate.claim.hostname_hint, None);
        let verify = |value: &SignedNodeOwnership| {
            verify_node_ownership(
                Some(value),
                &NODE,
                &TrustStore::default(),
                TrustPolicy::Off,
                3_500,
            )
            .verified
        };
        assert!(verify(&certificate));
        assert!(serde_json::to_vec(&certificate).unwrap().len() < 8 * 1024);
        let mut changed = certificate.clone();
        changed.claim.issued_at_unix_ms += 1000;
        assert!(!verify(&changed));
        changed = certificate.clone();
        changed.claim.expires_at_unix_ms += 1000;
        assert!(!verify(&changed));
        changed = certificate.clone();
        changed.claim.node_endpoint_id = hex::encode([0x24; 32]);
        assert!(!verify(&changed));
        changed = certificate.clone();
        changed.claim.owner_id = hex::encode([0; 32]);
        assert!(!verify(&changed));
        changed = certificate.clone();
        changed.claim.owner_sign_public_key = hex::encode([0; 32]);
        assert!(!verify(&changed));
        let repeated = sign_at([2_999, 3_500, 3_500], 1_500).unwrap();
        assert_ne!(certificate.claim.cert_id, repeated.claim.cert_id);
    }

    #[test]
    fn fractional_and_sql_style_handoff_boundaries() {
        // Original redemption seconds captured BEFORE SQL, checked conversion only.
        let original_seconds = 2_u64;
        assert_eq!(original_seconds.checked_mul(1000), Some(2_000));
        assert!(sign_at([2_999, 2_999, 2_999], 999).is_ok());
        assert!(sign_at([2_999, 3_500, 3_500], 1_500).is_ok()); // A
        assert_eq!(
            sign_at([2_999, 3_501, 3_501], 1_500),
            Err(ExactSigningError::TimeRefused)
        ); // A+1
        assert_eq!(
            sign_at([2_999, 3_500, 3_501], 1_500),
            Err(ExactSigningError::TimeRefused)
        );
    }

    #[test]
    fn future_expiry_backward_and_signing_delay_refuse() {
        for times in [
            [1_999, 2_000, 2_000],
            [10_000, 10_000, 10_000],
            [2_999, 10_000, 10_000],
            [2_999, 2_998, 2_999],
            [2_999, 3_000, 2_999],
            [2_999, 9_999, 10_000],
        ] {
            assert_eq!(sign_at(times, 20_000), Err(ExactSigningError::TimeRefused));
        }
    }

    #[test]
    fn structural_lifetime_and_overflow_validation() {
        for (issued, expires) in [
            (0, 10_000),
            (2_001, 10_000),
            (2_000, 10_001),
            (2_000, 2_000),
            (10_000, 2_000),
            (2_000, u64::MAX),
        ] {
            assert!(ExactNodeSigningRequest::new(NODE, issued, expires).is_err());
        }
        assert_eq!(u64::MAX.checked_mul(1000), None);
        assert!(ExactSigningPolicy::new(0, 0).is_err());
        assert!(
            validate(
                &request(),
                &ExactSigningPolicy::new(8_000, 999).unwrap(),
                at(2_999)
            )
            .is_ok()
        );
        assert_eq!(
            validate(
                &request(),
                &ExactSigningPolicy::new(7_999, 999).unwrap(),
                at(2_999)
            ),
            Err(ExactSigningError::InvalidRequest)
        );
        assert_eq!(
            validate(
                &request(),
                &ExactSigningPolicy::new(8_000, 999).unwrap(),
                UNIX_EPOCH - Duration::from_nanos(1)
            ),
            Err(ExactSigningError::TimeRefused)
        );
    }

    #[test]
    fn explicit_zero_age_is_not_rounded_or_widened() {
        assert!(sign_at([2_000; 3], 0).is_ok());
        let policy = ExactSigningPolicy::new(8_000, 0).unwrap();
        assert_eq!(
            validate(&request(), &policy, at(2_000) + Duration::from_nanos(1)),
            Err(ExactSigningError::TimeRefused)
        );
        let policy = ExactSigningPolicy::new(8_000, 999).unwrap();
        assert_eq!(
            validate(&request(), &policy, at(2_999) + Duration::from_nanos(1)),
            Err(ExactSigningError::TimeRefused)
        );
    }

    #[test]
    fn real_entry_uses_synthetic_memory_key_only() {
        // Wide TEST-only limits avoid depending on scheduling for this smoke test.
        let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let issued = elapsed.as_secs().checked_mul(1000).unwrap();
        let expires = issued.checked_add(60_000).unwrap();
        let request = ExactNodeSigningRequest::new(NODE, issued, expires).unwrap();
        let policy = ExactSigningPolicy::new(60_000, 30_000).unwrap();
        preflight_exact_node_ownership(&request, &policy).unwrap();
        let certificate = sign_node_ownership_exact(&owner(), &request, &policy).unwrap();
        assert_eq!(certificate.claim.issued_at_unix_ms, issued);
        assert_eq!(certificate.claim.expires_at_unix_ms, expires);
        assert!(
            verify_node_ownership(
                Some(&certificate),
                &NODE,
                &TrustStore::default(),
                TrustPolicy::Off,
                issued
            )
            .verified
        );
    }
}
