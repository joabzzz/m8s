//! Durable mock-first reconcile. No bootstrap admission or live provider is available.
use m8s_core::policy::{LifecycleEvidence, may_delete, transition};
use m8s_core::{GpuNode, InstanceState, Observation, Phase, Timestamp};
use m8s_provider::{LaunchSpec, Provider};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("state database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("provider outcome uncertain: {0}")]
    Provider(#[from] m8s_provider::Error),
    #[error("invalid state request or bootstrap binding")]
    Invalid,
    #[error("database belongs to another owner")]
    OwnerMismatch,
    #[error("private state path unavailable or unsafe")]
    PrivateState,
    #[error("secure bootstrap unavailable")]
    BootstrapUnavailable,
    #[error("invalid persisted phase")]
    Corrupt,
}
pub type Result<T> = std::result::Result<T, Error>;
/// A single-reconciler store, not a replica-safe distributed controller.
/// Callers must hold an exclusive process lock across the entire reconcile loop,
/// including provider effects. The daemon supplies this with its state-directory
/// fs2 lock; SQLite transactions alone protect reservations, not whole reconciles.
pub struct Store {
    conn: Connection,
    owner: String,
}
impl Store {
    pub fn open(path: impl AsRef<Path>, owner: &str) -> Result<Self> {
        if owner.is_empty() {
            return Err(Error::Invalid);
        }
        let private = private::prepare(path.as_ref())?;
        let path = &private.path;
        let mut conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::default() | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        private.recheck()?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS pool(owner TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS tier(name TEXT PRIMARY KEY, desired INTEGER NOT NULL CHECK(desired BETWEEN 0 AND 16));
        CREATE TABLE IF NOT EXISTS gpu_node(uid TEXT PRIMARY KEY, tier TEXT NOT NULL, instance_id TEXT UNIQUE, phase TEXT NOT NULL, created_at INTEGER NOT NULL, orphan INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS launch_attempt(uid TEXT PRIMARY KEY, tier TEXT NOT NULL, at INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS launch_window ON launch_attempt(tier,at);
        CREATE TABLE IF NOT EXISTS event(seq INTEGER PRIMARY KEY, at INTEGER NOT NULL, message TEXT NOT NULL);")?;
        private.recheck()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let count: i64 = tx.query_row("SELECT count(*) FROM pool", [], |r| r.get(0))?;
        if count > 1 {
            return Err(Error::Corrupt);
        }
        let existing: Option<String> = tx
            .query_row("SELECT owner FROM pool LIMIT 1", [], |r| r.get(0))
            .optional()?;
        match existing {
            Some(o) if o != owner => return Err(Error::OwnerMismatch),
            None => {
                tx.execute("INSERT INTO pool VALUES (?)", [owner])?;
            }
            _ => {}
        }
        tx.commit()?;
        nonce::initialize(&conn)?;
        budget::initialize(&conn)?;
        Ok(Self {
            conn,
            owner: owner.into(),
        })
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }
    pub fn scale(&self, tier: &str, desired: u32) -> Result<()> {
        if tier.is_empty()
            || tier.len() > 64
            || !tier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || desired > 16
        {
            return Err(Error::Invalid);
        }
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        tx.execute("INSERT INTO tier VALUES (?,?) ON CONFLICT(name) DO UPDATE SET desired=excluded.desired",params![tier,desired])?;
        if desired == 0 {
            nonce::cancel_tier(&tx, &self.owner, tier)?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn down(&self) -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        tx.execute("UPDATE tier SET desired=0", [])?;
        nonce::cancel_all(&tx, &self.owner)?;
        tx.commit()?;
        Ok(())
    }
    fn event(&self, now: Timestamp, message: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO event(at,message) VALUES (?,?)",
            params![now, message],
        )?;
        Ok(())
    }
    pub fn logs(&self) -> Result<Vec<String>> {
        let mut s=self.conn.prepare("SELECT message FROM (SELECT seq,message FROM event ORDER BY seq DESC LIMIT 200) ORDER BY seq")?;
        Ok(s.query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?)
    }
    pub fn tiers(&self) -> Result<Vec<(String, u32)>> {
        let mut s = self
            .conn
            .prepare("SELECT name,desired FROM tier ORDER BY name")?;
        Ok(s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?)
    }
    pub fn nodes(&self) -> Result<Vec<GpuNode>> {
        let mut s = self.conn.prepare(
            "SELECT uid,tier,instance_id,phase,created_at FROM gpu_node ORDER BY created_at,uid",
        )?;
        let rows = s.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        rows.map(|r| {
            let (uid, tier, instance_id, phase, created_at) = r?;
            Ok(GpuNode {
                uid,
                tier,
                instance_id,
                phase: serde_json::from_str(&phase).map_err(|_| Error::Corrupt)?,
                created_at,
                owner: self.owner.clone(),
            })
        })
        .collect()
    }
    fn phase(&self, uid: &str, phase: Phase) -> Result<()> {
        let current = self
            .nodes()?
            .into_iter()
            .find(|n| n.uid == uid)
            .ok_or(Error::Corrupt)?;
        transition(current.phase, phase, LifecycleEvidence::default())
            .map_err(|_| Error::Corrupt)?;
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        if phase == Phase::Destroying {
            nonce::cancel_uid(&tx, &self.owner, uid)?;
        }
        tx.execute(
            "UPDATE gpu_node SET phase=? WHERE uid=?",
            params![
                serde_json::to_string(&phase).map_err(|_| Error::Corrupt)?,
                uid
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Transaction commits a launch reservation before any provider side effect.
    fn reserve(&mut self, tier: &str, now: Timestamp) -> Result<Option<String>> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let desired: Option<u32> = tx
            .query_row("SELECT desired FROM tier WHERE name=?", [tier], |r| {
                r.get(0)
            })
            .optional()?;
        if desired.unwrap_or(0) == 0 {
            return Ok(None);
        }
        let future: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM launch_attempt WHERE tier=? AND at>?)",
            params![tier, now],
            |r| r.get(0),
        )?;
        if future {
            return Ok(None);
        }
        let count: u32 = tx.query_row(
            "SELECT count(*) FROM launch_attempt WHERE tier=? AND at>?",
            params![tier, now.saturating_sub(3600)],
            |r| r.get(0),
        )?;
        if count >= 3 {
            return Ok(None);
        }
        let uid: String = tx.query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))?;
        tx.execute(
            "INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES (?,?,'\"Launching\"',?)",
            params![uid, tier, now],
        )?;
        tx.execute(
            "INSERT INTO launch_attempt VALUES (?,?,?)",
            params![uid, tier, now],
        )?;
        tx.commit()?;
        Ok(Some(uid))
    }
    pub fn launch_count(&self, tier: &str, now: Timestamp) -> Result<u32> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM launch_attempt WHERE tier=? AND at>?",
            params![tier, now.saturating_sub(3600)],
            |r| r.get(0),
        )?)
    }
    pub fn reconcile(&mut self, p: &mut impl Provider, now: Timestamp) -> Result<()> {
        // A list failure does not become an empty snapshot.
        let owned = p.list_owned(&self.owner)?;
        for instance in owned {
            if instance.owner != self.owner {
                continue;
            }
            let node = self.nodes()?.into_iter().find(|n| {
                n.uid == instance.node_uid
                    && n.tier == instance.tier
                    && (n.instance_id.is_none() || n.instance_id.as_deref() == Some(&instance.id))
            });
            if let Some(n) = node {
                if n.instance_id.is_none() {
                    self.conn.execute(
                        "UPDATE gpu_node SET instance_id=?,phase='\"Booting\"' WHERE uid=?",
                        params![instance.id, n.uid],
                    )?;
                    self.event(now, "adopted instance from durable launch intent")?;
                }
            } else {
                // Track orphans before destroying; stopped boxes still bill storage.
                self.conn.execute("INSERT OR IGNORE INTO gpu_node(uid,tier,instance_id,phase,created_at,orphan) VALUES (?, ?, ?, '\"Orphaned\"', ?,1)",
                    params![format!("orphan:{}",instance.id),instance.tier,instance.id,now])?;
            }
        }
        let tiers = self.tiers()?;
        let mut retained = std::collections::BTreeMap::<String, u32>::new();
        for n in self.nodes()? {
            let desired = tiers
                .iter()
                .find(|(t, _)| *t == n.tier)
                .map(|(_, d)| *d)
                .unwrap_or(0);
            let Some(id) = &n.instance_id else {
                // No returned id means an uncertain launch. Do not retry or discard the intent.
                *retained.entry(n.tier.clone()).or_default() += 1;
                self.event(now, "unresolved launch intent; launch not retried")?;
                continue;
            };
            match p.describe(id) {
                Err(_) => {
                    self.event(now, "describe uncertain; finalizer retained")?;
                    *retained.entry(n.tier.clone()).or_default() += 1;
                }
                Ok(Observation::Absent) => {
                    // The sole deletion site: explicit per-instance positive absence.
                    // Advance through teardown even when the provider removed it externally.
                    let evidence = LifecycleEvidence {
                        provider_confirmed_absent: true,
                        ..LifecycleEvidence::default()
                    };
                    let destroying = transition(n.phase, Phase::Destroying, evidence)
                        .map_err(|_| Error::Corrupt)?;
                    let gone = transition(destroying, Phase::Gone, evidence)
                        .map_err(|_| Error::Corrupt)?;
                    if !may_delete(gone, evidence) {
                        return Err(Error::Corrupt);
                    }
                    let tx = self
                        .conn
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                    nonce::cancel_uid(&tx, &self.owner, &n.uid)?;
                    tx.execute("DELETE FROM gpu_node WHERE uid=?", [&n.uid])?;
                    tx.commit()?;
                    self.event(now, "provider confirmed absence; finalizer released")?;
                }
                Ok(Observation::Present(i)) => {
                    if i.owner != self.owner || i.id != *id {
                        // Refuse provider mutation, but invalidate our local grant
                        // and retain certificate cleanup intent on observed mismatch.
                        let tx = self
                            .conn
                            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                        nonce::cancel_uid(&tx, &self.owner, &n.uid)?;
                        tx.commit()?;
                        self.event(now, "provider ownership mismatch; refusing mutation")?;
                        *retained.entry(n.tier.clone()).or_default() += 1;
                        continue;
                    }
                    let count = retained.entry(n.tier.clone()).or_default();
                    let deleting: bool = self.conn.query_row(
                        "SELECT deleting FROM gpu_node WHERE uid=?",
                        [&n.uid],
                        |r| r.get(0),
                    )?;
                    if deleting
                        || n.uid.starts_with("orphan:")
                        || matches!(n.phase, Phase::Destroying | Phase::Draining)
                        || matches!(i.state, InstanceState::Stopped | InstanceState::Exited)
                        || *count >= desired
                    {
                        self.phase(&n.uid, Phase::Destroying)?;
                        match p.destroy(id) {
                            Ok(()) => self.event(
                                now,
                                "destroy acknowledged; waiting for confirmed absence",
                            )?,
                            Err(_) => self.event(now, "destroy uncertain; retry required")?,
                        }
                    }
                    // Include destroy-pending nodes to prevent overlapping replacement spend.
                    *count += 1;
                }
            }
        }
        for (tier, desired) in tiers {
            let have = retained.get(&tier).copied().unwrap_or(0);
            for _ in have..desired {
                // Mock has no real cost, image or security admission path.
                if p.id() != "mock" {
                    self.event(
                        now,
                        "live launch unavailable: secure admission not implemented",
                    )?;
                    break;
                }
                let offers = p.search()?;
                let Some(offer) = offers.first() else {
                    break;
                };
                let Some(uid) = self.reserve(&tier, now)? else {
                    self.event(now, "Paused: persisted 3 launches/tier/hour limit")?;
                    break;
                };
                let spec = LaunchSpec {
                    owner: self.owner.clone(),
                    node_uid: uid.clone(),
                    tier: tier.clone(),
                };
                match p.launch(offer, &spec) {
                    Ok(i) if i.owner == self.owner && i.node_uid == uid && i.tier == tier => {
                        self.conn.execute(
                            "UPDATE gpu_node SET instance_id=?,phase='\"Booting\"' WHERE uid=?",
                            params![i.id, uid],
                        )?;
                        self.event(
                            now,
                            "mock launched; secure bootstrap unavailable; not Ready",
                        )?;
                    }
                    _ => self.event(
                        now,
                        "launch uncertain; intent retained for owner-scoped adoption",
                    )?,
                }
            }
        }
        Ok(())
    }
}

pub mod budget;
pub mod nonce;

mod private;
