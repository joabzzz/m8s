//! Validated transport-to-storage boundary. No daemon/provider dependencies.
use crate::*;

/// Opaque output of full transport/request validation, not an authentication
/// constructor. Only redeem_with creates it. Contains public claims + nonce hash,
/// never raw nonce/join bytes. No Clone/Deserialize/public fields.
pub struct ValidatedRedemption {
    nonce_hash: [u8; 32],
    binding: LaunchBinding,
    claims: CertificateClaims,
}
impl ValidatedRedemption {
    pub fn nonce_hash(&self) -> &[u8; 32] {
        &self.nonce_hash
    }
    pub fn binding(&self) -> &LaunchBinding {
        &self.binding
    }
    pub fn claims(&self) -> &CertificateClaims {
        &self.claims
    }
}

/// One configured authoritative ledger. consume MUST durably commit one-use burn
/// and recovery/cleanup intent before returning. Never retain an open transaction
/// across this call: signing is the next external effect. Daemon uses Store, not
/// the standalone Ledger in parallel. Implementors are trusted owner-side code.
pub trait NonceConsumer {
    fn consume(&self, request: &ValidatedRedemption) -> Result<()>;
    /// After real signature verification, persist certificate/public recovery
    /// metadata and recheck launch state before allowing a bundle to be returned.
    /// A failure never restores the nonce; cleanup intent must already be durable.
    fn certified(
        &self,
        _request: &ValidatedRedemption,
        _certificate: &NodeCertificate,
    ) -> Result<()> {
        Ok(())
    }
}

/// Validates transport/request binding once and delegates burn to exactly one
/// ledger. No transaction or await crosses signing, verification or delivery.
#[allow(clippy::too_many_arguments)]
pub fn redeem_with(
    consumer: &impl NonceConsumer,
    session: &IrohSession,
    request: Redemption,
    expected: &LaunchBinding,
    now: i64,
    cert_expires_at: i64,
    join_token: Zeroizing<Vec<u8>>,
    signer: &mut impl NodeSigner,
    verifier: &impl CertificateVerifier,
) -> Result<PendingDelivery> {
    expected.validate()?;
    if session
        .delivery_binding
        .as_ref()
        .is_some_and(|b| b.request_hash != request.fingerprint())
    {
        return Err(Error::Refused);
    }
    if session.alpn != BOOT_ALPN
        || session.owner != expected.owner
        || session.peer != request.node
        || session.source_ip != expected.provider_ip
        || request.instance_id != expected.instance_id
        || request.container_id != expected.container_id
        || request.image_sha256 != expected.image_sha256
        || !request.self_destruct_present
        || now < 0
        || cert_expires_at <= now
        || join_token.is_empty()
    {
        return Err(Error::Refused);
    }
    let validated = ValidatedRedemption {
        nonce_hash: request.nonce.hash(),
        binding: expected.clone(),
        claims: CertificateClaims {
            owner: expected.owner,
            node: request.node,
            issued_at: now,
            expires_at: cert_expires_at,
        },
    };
    consumer.consume(&validated)?;
    let certificate = signer
        .sign(&validated.claims)
        .map_err(|_| Error::Certificate)?;
    verifier
        .verify(&certificate, &validated.claims)
        .map_err(|_| Error::Certificate)?;
    consumer.certified(&validated, &certificate)?;
    Ok(PendingDelivery {
        delivery_token: session.delivery_binding.as_ref().map(|b| b.token.clone()),
        certificate,
        join_token,
        claims: validated.claims,
        acl: ControlAcl::owner_only(expected.owner),
        settings: None,
    })
}
