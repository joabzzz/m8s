//! Owner-side composition of authenticated receipt, Store burn and TLS delivery.
//! The caller owns the listener, current observation, signing custody and cleanup.
use crate::{
    Store,
    nonce::{BootstrapAttempt, BootstrapStage, LaunchIdentity},
};
use m8s_bootstrap::{
    BundleSettings, CertificateClaims, Error, NodeSigner, Result, mp001::Mp001CertificateVerifier,
    transport::BootConnection,
};
use zeroize::Zeroizing;

pub mod session;

/// Trusted inputs for one attempt. No Debug/Serialize: contains join/config bytes.
pub struct DeliveryPlan<'a> {
    pub observation: LaunchIdentity<'a>,
    pub cert_expires_at: i64,
    pub join_token: Zeroizing<Vec<u8>>,
    pub settings: BundleSettings,
}

/// Original stream acknowledged and Store confirmation committed. This is not
/// native startup, DMS, owner-routed inference, or a Ready/BootGate capability.
/// No public constructor, Clone or Deserialize.
///
/// ```compile_fail
/// use m8sd::bootstrap_delivery::DeliveredBootstrap;
/// let receipt = DeliveredBootstrap {
///     node_uid: String::new(), container_id: String::new(),
///     claims: panic!("caller-supplied claims cannot mint a receipt"),
/// };
/// ```
pub struct DeliveredBootstrap {
    node_uid: String,
    container_id: String,
    claims: CertificateClaims,
}
impl DeliveredBootstrap {
    pub fn node_uid(&self) -> &str {
        &self.node_uid
    }
    pub fn container_id(&self) -> &str {
        &self.container_id
    }
    pub fn claims(&self) -> &CertificateClaims {
        &self.claims
    }
}

impl Store {
    /// Process exactly one request on an already authenticated direct boot stream.
    /// Cancellation or failure after burn preserves the durable cleanup intent;
    /// callers must retain ownership and clean up, never retry with a fresh nonce.
    /// No database transaction crosses an await or external signer invocation.
    pub async fn deliver_bootstrap(
        &self,
        mut connection: BootConnection,
        plan: DeliveryPlan<'_>,
        signer: &mut impl NodeSigner,
        verifier: &Mp001CertificateVerifier,
        clock: impl Fn() -> i64,
    ) -> Result<DeliveredBootstrap> {
        let request = connection.receive().await?;
        let node_uid = plan.observation.node_uid.to_owned();
        let binding = plan.observation.binding.clone();
        let pending = self
            .redeem_bootstrap(
                connection.session(),
                request,
                &binding,
                plan.observation,
                clock(),
                plan.cert_expires_at,
                plan.join_token,
                signer,
                verifier,
            )?
            .with_settings(plan.settings)?;
        let claims = pending.claims().clone();
        // Select the exact durable certified attempt before delivering. A failed
        // lookup leaves the already-burned nonce and cleanup intent intact.
        let attempt = certified_attempt(
            self.bootstrap_attempts().map_err(|_| Error::Storage)?,
            &self.owner,
            &node_uid,
            &binding,
            &claims,
        )?;
        connection.deliver(pending, clock()).await?;
        // Sample again AFTER acknowledgement: expiry/down during delivery must
        // not clear cleanup intent or return successful delivery evidence.
        if !self
            .confirm_bootstrap_delivery(&attempt, clock())
            .map_err(|_| Error::Storage)?
        {
            return Err(Error::Refused);
        }
        Ok(DeliveredBootstrap {
            node_uid,
            container_id: binding.container_id,
            claims,
        })
    }
}

pub(crate) fn certified_attempt(
    attempts: Vec<BootstrapAttempt>,
    owner: &str,
    node_uid: &str,
    binding: &m8s_bootstrap::LaunchBinding,
    claims: &CertificateClaims,
) -> Result<BootstrapAttempt> {
    if claims.owner != binding.owner {
        return Err(Error::Refused);
    }
    let mut selected = attempts.into_iter().filter(|a| {
        a.owner == owner
            && a.node_uid == node_uid
            && a.instance_id == binding.instance_id
            && a.container_id == binding.container_id
            && a.node_endpoint == Some(claims.node)
            && a.redeemed_at == Some(claims.issued_at)
            && a.cert_expires_at == Some(claims.expires_at)
            && a.stage == BootstrapStage::Certified
            && !a.cancelled
            && a.cleanup_required
    });
    let attempt = selected.next().ok_or(Error::Refused)?;
    if selected.next().is_some() {
        return Err(Error::Refused);
    }
    Ok(attempt)
}
