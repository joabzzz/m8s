//! Owner-side ADR-0006 policy with optional real encrypted iroh transport.
//! Certificate implementations and live provider observation remain integration gates.
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, net::IpAddr};
use zeroize::Zeroizing;

pub const BOOT_ALPN: &[u8] = b"m8s-boot/1";
pub const CONTROL_ALPN: &[u8] = b"m8s-control/1";
pub const NONCE_TTL_SECONDS: i64 = 900;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("bootstrap refused")]
    Refused,
    #[error("secure adapter unavailable")]
    Unavailable,
    #[error("bootstrap storage failure")]
    Storage,
    #[error("secure transport failed or timed out")]
    Transport,
    #[error("certificate signing or verification failed")]
    Certificate,
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}

/// Public iroh endpoint key bytes, not a relay address or arbitrary display string.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct EndpointId(pub [u8; 32]);

/// Not Debug, Clone, or Serialize. Generate with an OS CSPRNG outside this crate.
pub struct BootNonce(Zeroizing<[u8; 32]>);
impl BootNonce {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
    fn hash(&self) -> [u8; 32] {
        Sha256::digest(&self.0[..]).into()
    }
}

/// Supplied by the owner from its persisted launch + verified image provenance,
/// never from untrusted request fields. Container identity must be provider confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchBinding {
    pub owner: EndpointId,
    pub instance_id: String,
    pub container_id: String,
    pub provider_ip: IpAddr,
    pub image_sha256: [u8; 32],
}
impl LaunchBinding {
    fn validate(&self) -> Result<()> {
        if self.instance_id.is_empty() || self.container_id.is_empty() {
            return Err(Error::Refused);
        }
        Ok(())
    }
}

/// Transport-owned evidence, never accepted from a wire message. No public constructor.
/// Source IP must be direct, validated remote IP, NOT a relay or agent-reported IP.
/// A relayed connection without separately verified origin evidence must be refused.
pub struct IrohSession {
    owner: EndpointId,
    peer: EndpointId,
    source_ip: IpAddr,
    alpn: Vec<u8>,
}

/// Explicit fail-closed placeholder, not a plaintext or mock network implementation.
#[cfg(not(feature = "iroh-transport"))]
pub fn accept_iroh() -> Result<IrohSession> {
    Err(Error::Unavailable)
}

pub struct Redemption {
    pub nonce: BootNonce,
    pub instance_id: String,
    pub container_id: String,
    pub node: EndpointId,
    pub image_sha256: [u8; 32],
    /// Agent assertion only; not cryptographic proof of provider key possession.
    pub self_destruct_present: bool,
}

/// Exact public claims which the signer/verifier adapter binds. `owner` is the
/// pinned iroh owner endpoint, not an MP-001 signing-owner ID: a production adapter
/// must map it to the separately configured trusted mesh signing identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertificateClaims {
    pub owner: EndpointId,
    pub node: EndpointId,
    pub issued_at: i64,
    pub expires_at: i64,
}
pub struct NodeCertificate(pub Vec<u8>);
/// Implement using the local owner signer. Never export owner private keys.
pub trait NodeSigner {
    fn sign(&mut self, claims: &CertificateClaims) -> Result<NodeCertificate>;
}
/// Must perform real MP-001 signature verification AND exact claim binding,
/// including the configured owner-endpoint-to-signing-identity mapping, node and
/// validity. Never infer signing trust from an untrusted certificate public key. No production implementation is provided.
pub trait CertificateVerifier {
    fn verify(&self, certificate: &NodeCertificate, expected: &CertificateClaims) -> Result<()>;
}

/// Explicit endpoint allowlist, never "all admitted mesh members".
#[derive(Clone, Debug)]
pub struct ControlAcl {
    owner: EndpointId,
    endpoints: BTreeSet<EndpointId>,
}
impl ControlAcl {
    pub fn owner_only(owner: EndpointId) -> Self {
        Self {
            owner,
            endpoints: BTreeSet::from([owner]),
        }
    }
    pub fn with_delegates(
        owner: EndpointId,
        delegates: impl IntoIterator<Item = EndpointId>,
    ) -> Self {
        Self {
            owner,
            endpoints: std::iter::once(owner).chain(delegates).collect(),
        }
    }
    pub fn authorize(&self, session: &IrohSession) -> Result<()> {
        if session.owner == self.owner
            && session.alpn == CONTROL_ALPN
            && self.endpoints.contains(&session.peer)
        {
            Ok(())
        } else {
            Err(Error::Refused)
        }
    }
}

/// Owner-selected model/config/heartbeat settings. Config may contain sensitive
/// material: deliberately no Debug/Serialize, zeroizing bytes.
pub struct BundleSettings {
    pub model: String,
    pub config: Zeroizing<Vec<u8>>,
    pub heartbeat_ttl_seconds: u64,
}

/// Consumed-nonce output. Join material stays private and zeroized; the optional
/// real transport adapter delivers it only on its original encrypted stream.
pub struct PendingDelivery {
    certificate: NodeCertificate,
    join_token: Zeroizing<Vec<u8>>,
    claims: CertificateClaims,
    acl: ControlAcl,
    settings: Option<BundleSettings>,
}
impl PendingDelivery {
    pub fn claims(&self) -> &CertificateClaims {
        &self.claims
    }
    pub fn with_settings(mut self, settings: BundleSettings) -> Result<Self> {
        if settings.model.is_empty() || settings.heartbeat_ttl_seconds == 0 {
            return Err(Error::Refused);
        }
        self.settings = Some(settings);
        Ok(self)
    }
    #[cfg(not(feature = "iroh-transport"))]
    pub fn deliver(self, _session: &IrohSession) -> Result<()> {
        // Explicitly retain ownership of sensitive fields until zeroizing drop.
        let _ = (&self.certificate, &self.join_token, &self.acl);
        Err(Error::Unavailable)
    }
}

/// Separate durable protocol ledger; m8sd integration must share its launch intent
/// transaction or adapt its existing ledger. Never stores nonce/join/certificate secrets.
pub struct Ledger(Connection);
impl Ledger {
    pub fn new(connection: Connection) -> Result<Self> {
        // A rollback after signing would make a consumed nonce reusable.
        if !connection.is_autocommit() {
            return Err(Error::Refused);
        }
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS bootstrap_nonce (
            hash BLOB PRIMARY KEY, owner BLOB NOT NULL, instance TEXT NOT NULL,
            container TEXT NOT NULL, ip TEXT NOT NULL, digest BLOB NOT NULL,
            issued INTEGER NOT NULL, redeemed INTEGER, endpoint BLOB);",
        )?;
        Ok(Self(connection))
    }
    pub fn issue(&self, nonce: &BootNonce, binding: &LaunchBinding, now: i64) -> Result<()> {
        binding.validate()?;
        if now < 0 {
            return Err(Error::Refused);
        }
        self.0.execute("INSERT INTO bootstrap_nonce(hash,owner,instance,container,ip,digest,issued) VALUES (?,?,?,?,?,?,?)",
            params![nonce.hash().as_slice(), binding.owner.0.as_slice(), binding.instance_id,
                binding.container_id, binding.provider_ip.to_string(), binding.image_sha256.as_slice(), now])?;
        Ok(())
    }
    /// Burn atomically BEFORE signing/delivery. Any later failure requires a new
    /// launch/nonce and revocation investigation, never retrying a consumed nonce.
    #[allow(clippy::too_many_arguments)]
    pub fn redeem(
        &self,
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
        let changed = self.0.execute(
            "UPDATE bootstrap_nonce SET redeemed=?,endpoint=? WHERE hash=?
            AND owner=? AND instance=? AND container=? AND ip=? AND digest=?
            AND redeemed IS NULL AND issued<=? AND issued>?",
            params![
                now,
                request.node.0.as_slice(),
                request.nonce.hash().as_slice(),
                expected.owner.0.as_slice(),
                expected.instance_id,
                expected.container_id,
                expected.provider_ip.to_string(),
                expected.image_sha256.as_slice(),
                now,
                now.saturating_sub(NONCE_TTL_SECONDS)
            ],
        )?;
        if changed != 1 {
            return Err(Error::Refused);
        }
        let claims = CertificateClaims {
            owner: expected.owner,
            node: request.node,
            issued_at: now,
            expires_at: cert_expires_at,
        };
        let certificate = signer.sign(&claims).map_err(|_| Error::Certificate)?;
        verifier
            .verify(&certificate, &claims)
            .map_err(|_| Error::Certificate)?;
        Ok(PendingDelivery {
            certificate,
            join_token,
            claims,
            acl: ControlAcl::owner_only(expected.owner),
            settings: None,
        })
    }
}

/// Owner-side inbound heartbeat guard. Does not imply model readiness/admission.
/// Caller must persist sequence/last_seen with the node record before acknowledging.
pub struct HeartbeatGuard {
    owner: EndpointId,
    node: EndpointId,
    cert_expires_at: i64,
    last_seen: i64,
    sequence: Option<u64>,
    ttl: i64,
}
impl HeartbeatGuard {
    pub fn new(claims: &CertificateClaims, ttl: i64) -> Result<Self> {
        if ttl <= 0 || claims.issued_at < 0 || claims.expires_at <= claims.issued_at {
            return Err(Error::Refused);
        }
        Ok(Self {
            owner: claims.owner,
            node: claims.node,
            cert_expires_at: claims.expires_at,
            last_seen: claims.issued_at,
            sequence: None,
            ttl,
        })
    }
    pub fn receive(&mut self, session: &IrohSession, sequence: u64, now: i64) -> Result<()> {
        if session.alpn != CONTROL_ALPN
            || session.owner != self.owner
            || session.peer != self.node
            || now < self.last_seen
            || self.expired(now)
            || self.sequence.is_some_and(|s| sequence <= s)
        {
            return Err(Error::Refused);
        }
        self.sequence = Some(sequence);
        self.last_seen = now;
        Ok(())
    }
    pub fn expired(&self, now: i64) -> bool {
        now < self.last_seen
            || now >= self.cert_expires_at
            || now.saturating_sub(self.last_seen) >= self.ttl
    }
}

#[cfg(test)]
mod tests;

#[cfg(feature = "iroh-transport")]
pub mod transport;
#[cfg(any(feature = "iroh-transport", test))]
mod wire;

#[cfg(feature = "iroh-transport")]
pub use transport::{AuthenticatedControlSession, ControlPolicy, VerifiedHeartbeat};
