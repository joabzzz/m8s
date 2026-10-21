//! One authoritative daemon launch/nonce table. No provider/TLS listener is wired.
//! Issuance metadata comes from the trusted owner/provider/image adapter, never
//! request claims. Short transactions commit burn+cleanup intent before signing.
use crate::{Error, Result, Store};
use m8s_bootstrap::{
    CertificateVerifier, EndpointId, IrohSession, LaunchBinding, NodeCertificate, NodeSigner,
    NonceConsumer, PendingDelivery, Redemption, ValidatedRedemption,
};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Trusted owner observation of one already-persisted launch. Not proof from a peer.
pub struct LaunchIdentity<'a> {
    pub owner: &'a str,
    pub node_uid: &'a str,
    pub binding: &'a LaunchBinding,
    /// Local owner-adapter observation time, never a provider/wire timestamp.
    pub observed_at: i64,
    pub status: ObservationStatus,
}
/// Trusted adapter assertion, not cryptographic provider proof. Only a complete
/// successful exact-identity snapshot is eligible. Unknown is not absence.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ObservationStatus {
    Complete,
    Partial,
    Unknown,
    Error,
}
pub const MAX_OBSERVATION_AGE_SECONDS: i64 = 30;
impl LaunchIdentity<'_> {
    fn validate(&self, owner: &str, binding: &LaunchBinding, now: i64) -> Result<()> {
        if self.owner != owner
            || self.node_uid.is_empty()
            || self.binding != binding
            || self.status != ObservationStatus::Complete
            || self.observed_at < 0
            || now < self.observed_at
            || now - self.observed_at > MAX_OBSERVATION_AGE_SECONDS
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapStage {
    Issued,
    Consumed,
    Certified,
    Delivered,
    Cancelled,
    LegacyQuarantined,
}
pub struct BootstrapAttempt {
    nonce_hash: [u8; 32],
    pub owner: String,
    pub node_uid: String,
    pub instance_id: String,
    pub container_id: String,
    pub node_endpoint: Option<EndpointId>,
    /// Untrusted legacy recovery metadata; never endpoint authority.
    pub legacy_endpoint: Option<String>,
    pub cert_expires_at: Option<i64>,
    pub redeemed_at: Option<i64>,
    pub stage: BootstrapStage,
    pub cancelled: bool,
    pub cleanup_required: bool,
}
/// Old raw-string API cannot prove transport/container/owner identity. Retained
/// only to fail closed for old callers, not a second authorization path.
pub struct Binding<'a> {
    pub instance_id: &'a str,
    pub source_ip: &'a str,
    pub node_endpoint: &'a str,
    pub image_digest: &'a str,
    pub self_destruct_present: bool,
}

pub(crate) fn initialize(conn: &Connection) -> Result<()> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS nonce(hash BLOB PRIMARY KEY, instance_id TEXT NOT NULL, ip TEXT NOT NULL, digest TEXT NOT NULL, issued_at INTEGER NOT NULL, redeemed_at INTEGER, node_endpoint TEXT);")?;
    let columns: Vec<String> = {
        let mut q = tx.prepare("PRAGMA table_info(nonce)")?;
        q.query_map([], |r| r.get(1))?
            .collect::<std::result::Result<_, _>>()?
    };
    for (name, decl) in [
        ("owner", "TEXT NOT NULL DEFAULT ''"),
        ("node_uid", "TEXT NOT NULL DEFAULT ''"),
        ("container_id", "TEXT NOT NULL DEFAULT ''"),
        ("owner_endpoint", "BLOB NOT NULL DEFAULT X''"),
        ("cancelled", "INTEGER NOT NULL DEFAULT 1"),
        ("stage", "TEXT NOT NULL DEFAULT 'legacy-refused'"),
        ("cleanup_required", "INTEGER NOT NULL DEFAULT 0"),
        ("cert_expires_at", "INTEGER"),
        ("certificate_hash", "BLOB"),
    ] {
        if !columns.iter().any(|c| c == name) {
            tx.execute_batch(&format!("ALTER TABLE nonce ADD COLUMN {name} {decl}"))?;
        }
    }
    let columns: Vec<String> = {
        let mut q = tx.prepare("PRAGMA table_info(gpu_node)")?;
        q.query_map([], |r| r.get(1))?
            .collect::<std::result::Result<_, _>>()?
    };
    if !columns.iter().any(|c| c == "deleting") {
        tx.execute_batch("ALTER TABLE gpu_node ADD COLUMN deleting INTEGER NOT NULL DEFAULT 0")?;
    }
    // Old unbound tokens cannot be upgraded into authority or reissued for the
    // same live launch. Quarantine matching persisted nodes for owner-scoped
    // reconciliation; this does not assert absence or remove any row.
    tx.execute_batch("UPDATE gpu_node SET deleting=1 WHERE instance_id IN (SELECT instance_id FROM nonce WHERE stage='legacy-refused');
        UPDATE nonce SET stage='legacy-quarantined',cancelled=1,cleanup_required=CASE WHEN redeemed_at IS NULL THEN 0 ELSE 1 END WHERE stage='legacy-refused';")?;
    tx.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS nonce_launch ON nonce(owner,node_uid) WHERE owner<>'' AND node_uid<>'';
        CREATE INDEX IF NOT EXISTS nonce_cleanup ON nonce(owner,cleanup_required);")?;
    tx.commit()?;
    Ok(())
}
fn digest_text(digest: &[u8; 32]) -> String {
    let mut out = String::from("sha256:");
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}
fn cancel_sql(
    tx: &Transaction<'_>,
    owner: &str,
    uid: Option<&str>,
    tier: Option<&str>,
) -> Result<()> {
    let owns: bool = tx.query_row(
        "SELECT count(*)=1 AND min(owner)=? FROM pool",
        [owner],
        |r| r.get(0),
    )?;
    if !owns {
        return Err(Error::OwnerMismatch);
    }

    tx.execute(
        "UPDATE gpu_node SET deleting=1 WHERE (?1 IS NULL OR uid=?1) AND (?2 IS NULL OR tier=?2)",
        params![uid, tier],
    )?;
    tx.execute("UPDATE nonce SET cancelled=1,cleanup_required=CASE WHEN redeemed_at IS NULL THEN 0 ELSE 1 END,
        stage=CASE WHEN redeemed_at IS NULL THEN 'cancelled' ELSE stage END WHERE owner=?1
        AND (?2 IS NULL OR node_uid=?2) AND (?3 IS NULL OR node_uid IN (SELECT uid FROM gpu_node WHERE tier=?3))",params![owner,uid,tier])?;
    Ok(())
}
pub(crate) fn cancel_all(tx: &Transaction<'_>, owner: &str) -> Result<()> {
    cancel_sql(tx, owner, None, None)
}
pub(crate) fn cancel_tier(tx: &Transaction<'_>, owner: &str, tier: &str) -> Result<()> {
    cancel_sql(tx, owner, None, Some(tier))
}
pub(crate) fn cancel_uid(tx: &Transaction<'_>, owner: &str, uid: &str) -> Result<()> {
    cancel_sql(tx, owner, Some(uid), None)
}

// Internal SQL-policy record, never exported as transport authentication. Unit
// fixtures can test SQL races without minting production transport proof types.
struct ConsumeRecord<'a> {
    expected_uid: Option<&'a str>,
    hash: &'a [u8; 32],
    binding: &'a LaunchBinding,
    node: EndpointId,
    now: i64,
    cert_expires_at: i64,
}
impl Store {
    pub fn issue_bootstrap_nonce(
        &self,
        nonce: &[u8; 32],
        launch: LaunchIdentity<'_>,
        now: i64,
    ) -> Result<()> {
        launch.validate(&self.owner, launch.binding, now)?;
        if launch.owner != self.owner
            || launch.node_uid.is_empty()
            || launch.binding.instance_id.is_empty()
            || launch.binding.container_id.is_empty()
            || now < 0
        {
            return Err(Error::Invalid);
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let hash = Sha256::digest(nonce);
        let changed=tx.execute("INSERT INTO nonce(hash,instance_id,ip,digest,issued_at,owner,node_uid,container_id,owner_endpoint,cancelled,stage,cleanup_required)
            SELECT ?1,n.instance_id,?2,?3,?4,?5,n.uid,?6,?7,0,'issued',0 FROM gpu_node n JOIN tier t ON t.name=n.tier
            WHERE n.uid=?8 AND n.instance_id=?9 AND n.phase='\"Booting\"' AND n.deleting=0 AND t.desired>0
            AND EXISTS(SELECT 1 FROM pool WHERE owner=?5)",
            params![hash.as_slice(),launch.binding.provider_ip.to_string(),digest_text(&launch.binding.image_sha256),now,self.owner,
                launch.binding.container_id,launch.binding.owner.0.as_slice(),launch.node_uid,launch.binding.instance_id])?;
        if changed != 1 {
            return Err(Error::Invalid);
        }
        tx.commit()?;
        Ok(())
    }
    fn consume_record(&self, r: &ConsumeRecord<'_>) -> Result<()> {
        if r.now < 0 || r.cert_expires_at <= r.now {
            return Err(Error::Invalid);
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let changed=tx.execute("UPDATE nonce SET redeemed_at=?1,node_endpoint=?2,cert_expires_at=?3,stage='consumed',cleanup_required=1
            WHERE hash=?4 AND owner=?5 AND owner_endpoint=?6 AND instance_id=?7 AND container_id=?8 AND ip=?9 AND digest=?10
            AND (?12 IS NULL OR node_uid=?12) AND redeemed_at IS NULL AND cancelled=0 AND stage='issued' AND issued_at<=?1 AND issued_at>?11
            AND EXISTS(SELECT 1 FROM gpu_node n JOIN tier t ON t.name=n.tier WHERE n.uid=nonce.node_uid AND n.instance_id=nonce.instance_id
                AND n.phase='\"Booting\"' AND n.deleting=0 AND t.desired>0)
            AND EXISTS(SELECT 1 FROM pool WHERE owner=?5)",
            params![r.now,r.node.0.as_slice(),r.cert_expires_at,r.hash.as_slice(),self.owner,r.binding.owner.0.as_slice(),
                r.binding.instance_id,r.binding.container_id,r.binding.provider_ip.to_string(),digest_text(&r.binding.image_sha256),
                r.now.saturating_sub(m8s_bootstrap::NONCE_TTL_SECONDS),r.expected_uid])?;
        if changed != 1 {
            return Err(Error::Invalid);
        }
        tx.commit()?;
        Ok(())
    }
    fn certify_record(&self, r: &ConsumeRecord<'_>, certificate: &NodeCertificate) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let changed=tx.execute("UPDATE nonce SET stage='certified',certificate_hash=?1 WHERE hash=?2 AND owner=?3
            AND node_endpoint=?4 AND redeemed_at=?5 AND cert_expires_at=?6 AND cancelled=0 AND stage='consumed'
            AND EXISTS(SELECT 1 FROM gpu_node n JOIN tier t ON t.name=n.tier WHERE n.uid=nonce.node_uid AND n.instance_id=nonce.instance_id
                AND n.phase='\"Booting\"' AND n.deleting=0 AND t.desired>0) AND EXISTS(SELECT 1 FROM pool WHERE owner=?3)",
            params![Sha256::digest(&certificate.0).as_slice(),r.hash.as_slice(),self.owner,r.node.0.as_slice(),r.now,r.cert_expires_at])?;
        if changed != 1 {
            return Err(Error::Invalid);
        }
        tx.commit()?;
        Ok(())
    }
    /// Production proof boundary. This uses Store's nonce table only; never
    /// instantiates m8s-bootstrap::Ledger. No open transaction crosses signing.
    #[allow(clippy::too_many_arguments)]
    pub fn redeem_bootstrap(
        &self,
        session: &IrohSession,
        request: Redemption,
        expected: &LaunchBinding,
        observation: LaunchIdentity<'_>,
        now: i64,
        cert_expires_at: i64,
        join_token: Zeroizing<Vec<u8>>,
        signer: &mut impl NodeSigner,
        verifier: &impl CertificateVerifier,
    ) -> m8s_bootstrap::Result<PendingDelivery> {
        m8s_bootstrap::redeem_with(
            &ObservedConsumer {
                store: self,
                observation,
            },
            session,
            request,
            expected,
            now,
            cert_expires_at,
            join_token,
            signer,
            verifier,
        )
    }
    pub fn bootstrap_attempts(&self) -> Result<Vec<BootstrapAttempt>> {
        let mut q=self.conn.prepare("SELECT hash,owner,node_uid,instance_id,container_id,node_endpoint,cert_expires_at,stage,cancelled,cleanup_required,redeemed_at FROM nonce WHERE owner=? OR (owner='' AND stage='legacy-quarantined') ORDER BY node_uid")?;
        let rows = q.query_map([&self.owner], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, rusqlite::types::Value>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, bool>(8)?,
                r.get::<_, bool>(9)?,
                r.get::<_, Option<i64>>(10)?,
            ))
        })?;
        rows.map(|r| {
            let (
                hash,
                owner,
                node_uid,
                instance_id,
                container_id,
                node,
                cert_expires_at,
                stage,
                cancelled,
                cleanup_required,
                redeemed_at,
            ) = r?;
            let stage = match stage.as_str() {
                "issued" => BootstrapStage::Issued,
                "consumed" => BootstrapStage::Consumed,
                "certified" => BootstrapStage::Certified,
                "delivered" => BootstrapStage::Delivered,
                "cancelled" => BootstrapStage::Cancelled,
                "legacy-quarantined" => BootstrapStage::LegacyQuarantined,
                _ => return Err(Error::Corrupt),
            };
            Ok(BootstrapAttempt {
                nonce_hash: hash.try_into().map_err(|_| Error::Corrupt)?,
                owner,
                node_uid,
                instance_id,
                container_id,
                node_endpoint: match &node {
                    _ if stage == BootstrapStage::LegacyQuarantined => None,
                    rusqlite::types::Value::Null => None,
                    rusqlite::types::Value::Blob(n) => Some(EndpointId(
                        n.clone().try_into().map_err(|_| Error::Corrupt)?,
                    )),
                    _ => return Err(Error::Corrupt),
                },
                legacy_endpoint: if stage == BootstrapStage::LegacyQuarantined {
                    match node {
                        rusqlite::types::Value::Text(t) => Some(t),
                        _ => None,
                    }
                } else {
                    None
                },
                cert_expires_at,
                redeemed_at,
                stage,
                cancelled,
                cleanup_required,
            })
        })
        .collect()
    }
    /// Owner calls only after actual encrypted delivery acknowledgement. This is
    /// a persistence hook, NOT proof a listener/cert admission/canary was wired.
    /// A concurrent down or stale confirmation keeps cleanup_required=true.
    pub fn confirm_bootstrap_delivery(&self, attempt: &BootstrapAttempt, now: i64) -> Result<bool> {
        if attempt.owner != self.owner || now < 0 {
            return Err(Error::Invalid);
        }
        let Some(node) = attempt.node_endpoint else {
            return Err(Error::Invalid);
        };
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let changed=tx.execute("UPDATE nonce SET stage='delivered',cleanup_required=0 WHERE hash=?1 AND owner=?2 AND node_uid=?3 AND instance_id=?4
            AND node_endpoint=?5 AND cert_expires_at=?6 AND stage='certified' AND cancelled=0 AND redeemed_at<=?7 AND cert_expires_at>?7
            AND EXISTS(SELECT 1 FROM gpu_node n JOIN tier t ON t.name=n.tier WHERE n.uid=nonce.node_uid AND n.instance_id=nonce.instance_id
                AND n.phase='\"Booting\"' AND n.deleting=0 AND t.desired>0) AND EXISTS(SELECT 1 FROM pool WHERE owner=?2)",params![attempt.nonce_hash.as_slice(),self.owner,attempt.node_uid,attempt.instance_id,node.0.as_slice(),attempt.cert_expires_at,now])?;
        tx.commit()?;
        Ok(changed == 1)
    }
    /// Legacy entrypoints are deliberately unavailable: they lack UID/container/
    /// owner transport binding and must never bypass the authoritative consumer.
    pub fn issue_nonce(
        &self,
        _nonce: &[u8],
        _instance_id: &str,
        _ip: &str,
        _digest: &str,
        _now: i64,
    ) -> Result<()> {
        Err(Error::BootstrapUnavailable)
    }
    pub fn redeem_nonce(&self, _nonce: &[u8], _binding: Binding<'_>, _now: i64) -> Result<bool> {
        Err(Error::BootstrapUnavailable)
    }
}
fn protocol_error(e: Error) -> m8s_bootstrap::Error {
    match e {
        Error::Invalid | Error::OwnerMismatch => m8s_bootstrap::Error::Refused,
        _ => m8s_bootstrap::Error::Storage,
    }
}
struct ObservedConsumer<'a> {
    store: &'a Store,
    observation: LaunchIdentity<'a>,
}
impl ObservedConsumer<'_> {
    fn consume_observed(&self, r: &ConsumeRecord<'_>) -> Result<()> {
        self.observation
            .validate(&self.store.owner, r.binding, r.now)?;
        self.store.consume_record(&ConsumeRecord {
            expected_uid: Some(self.observation.node_uid),
            hash: r.hash,
            binding: r.binding,
            node: r.node,
            now: r.now,
            cert_expires_at: r.cert_expires_at,
        })
    }
}
impl NonceConsumer for ObservedConsumer<'_> {
    fn consume(&self, request: &ValidatedRedemption) -> m8s_bootstrap::Result<()> {
        self.consume_observed(&ConsumeRecord {
            expected_uid: None,
            hash: request.nonce_hash(),
            binding: request.binding(),
            node: request.claims().node,
            now: request.claims().issued_at,
            cert_expires_at: request.claims().expires_at,
        })
        .map_err(protocol_error)
    }
    fn certified(
        &self,
        request: &ValidatedRedemption,
        certificate: &NodeCertificate,
    ) -> m8s_bootstrap::Result<()> {
        self.store
            .certify_record(
                &ConsumeRecord {
                    expected_uid: Some(self.observation.node_uid),
                    hash: request.nonce_hash(),
                    binding: request.binding(),
                    node: request.claims().node,
                    now: request.claims().issued_at,
                    cert_expires_at: request.claims().expires_at,
                },
                certificate,
            )
            .map_err(protocol_error)
    }
}

#[cfg(test)]
mod tests;
