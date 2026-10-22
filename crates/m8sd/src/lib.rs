//! Durable mock-first reconcile. No bootstrap admission or live provider is available.
#[cfg(all(unix, feature = "local-development"))]
pub mod local_demo;

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
struct StoreOrigin;
/// A single-reconciler store, not a replica-safe distributed controller.
/// Callers must hold an exclusive process lock across the entire reconcile loop,
/// including provider effects. The daemon supplies this with its state-directory
/// fs2 lock; SQLite transactions alone protect reservations, not whole reconciles.
pub struct Store {
    origin: std::sync::Arc<StoreOrigin>,
    conn: Connection,
    owner: String,
}
// Exact existing mock launch gate, shared by managed LOCAL preparation. Read
// only: no desired/capacity/accounting mutations or new authority here.
fn launch_rate_available(conn: &Connection, tier: &str, now: Timestamp) -> Result<bool> {
    let future: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM launch_attempt WHERE tier=? AND at>?)",
        params![tier, now],
        |r| r.get(0),
    )?;
    if future {
        return Ok(false);
    }
    let count: u32 = conn.query_row(
        "SELECT count(*) FROM launch_attempt WHERE tier=? AND at>?",
        params![tier, now.saturating_sub(3600)],
        |r| r.get(0),
    )?;
    Ok(count < 3)
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
        operation::initialize(&conn)?;
        Ok(Self {
            origin: std::sync::Arc::new(StoreOrigin),
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
    /// Atomic manual desired changes and explicit-zero nonce cancellation only.
    /// Unmentioned tiers stay unchanged; this is not provider convergence/authority.
    pub fn scale_batch(&self, targets: &[(String, u32)]) -> Result<()> {
        let mut seen = std::collections::BTreeSet::new();
        if targets.is_empty() || targets.len() > 32 {
            return Err(Error::Invalid);
        }
        for (tier, desired) in targets {
            if tier.is_empty()
                || tier.len() > 64
                || !tier
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || *desired > 16
                || !seen.insert(tier)
            {
                return Err(Error::Invalid);
            }
        }
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        for (tier, desired) in targets {
            if tx.execute("INSERT INTO tier VALUES (?,?) ON CONFLICT(name) DO UPDATE SET desired=excluded.desired",params![tier,desired])? != 1 {return Err(Error::Corrupt);}
            if *desired == 0 {
                nonce::cancel_tier(&tx, &self.owner, tier)?;
            }
        }
        for (tier, desired) in targets {
            let actual: Option<u32> = tx
                .query_row("SELECT desired FROM tier WHERE name=?", [tier], |r| {
                    r.get(0)
                })
                .optional()?;
            if actual != Some(*desired) {
                return Err(Error::Corrupt);
            }
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
    /// ONE stored read snapshot of the existing rate gate, NOT overall eligibility.
    pub fn explain_tier(&self, tier: &str, now: Timestamp) -> Result<m8s_api::StoredLaunchGates> {
        if now < 0
            || tier.is_empty()
            || tier.len() > 64
            || !tier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(Error::Invalid);
        }
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Deferred,
        )?;
        let desired: Option<i64> = tx
            .query_row("SELECT desired FROM tier WHERE name=?", [tier], |r| {
                r.get(0)
            })
            .optional()?;
        let desired = desired
            .map(|n| {
                u32::try_from(n)
                    .ok()
                    .filter(|n| *n <= 16)
                    .ok_or(Error::Corrupt)
            })
            .transpose()?;
        let count = |sql: &str| -> Result<u32> {
            let n: i64 = tx.query_row(sql, [tier], |r| r.get(0))?;
            u32::try_from(n).map_err(|_| Error::Corrupt)
        };
        let stored_node_count = count("SELECT count(*) FROM gpu_node WHERE tier=?")?;
        let stored_deleting_count =
            count("SELECT count(*) FROM gpu_node WHERE tier=? AND deleting=1")?;
        if count(
            "SELECT count(*) FROM gpu_node WHERE tier=? AND (typeof(deleting)!='integer' OR deleting NOT IN(0,1))",
        )? > 0
            || count("SELECT count(*) FROM launch_attempt WHERE tier=? AND typeof(at)!='integer'")?
                > 0
        {
            return Err(Error::Corrupt);
        }
        if count(
            "SELECT count(*) FROM gpu_node WHERE tier=? AND (typeof(phase)!='text' OR length(CAST(phase AS BLOB))>32)",
        )? > 0
        {
            return Err(Error::Corrupt);
        }
        let mut phases = std::collections::BTreeMap::new();
        {
            let mut s =
                tx.prepare("SELECT phase,count(*) FROM gpu_node WHERE tier=? GROUP BY phase")?;
            let mut rows = s.query([tier])?;
            while let Some(r) = rows.next()? {
                let raw: String = r.get(0)?;
                let phase: Phase = serde_json::from_str(&raw).map_err(|_| Error::Corrupt)?;
                let n: i64 = r.get(1)?;
                let count = u32::try_from(n).map_err(|_| Error::Corrupt)?;
                let label = serde_json::to_string(&phase).map_err(|_| Error::Corrupt)?;
                if count == 0
                    || phases
                        .insert(label, m8s_api::StoredPhaseCount { phase, count })
                        .is_some()
                    || phases.len() > 13
                {
                    return Err(Error::Corrupt);
                }
            }
        }
        let sum = phases
            .values()
            .try_fold(0u32, |sum, p| sum.checked_add(p.count))
            .ok_or(Error::Corrupt)?;
        if sum != stored_node_count || stored_deleting_count > stored_node_count {
            return Err(Error::Corrupt);
        }
        let cutoff = now.saturating_sub(3600);
        let n: i64 = tx.query_row(
            "SELECT count(*) FROM launch_attempt WHERE tier=? AND at>?",
            params![tier, cutoff],
            |r| r.get(0),
        )?;
        let launches_after_cutoff = u32::try_from(n).map_err(|_| Error::Corrupt)?;
        let future_attempt_present: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM launch_attempt WHERE tier=? AND at>?)",
            params![tier, now],
            |r| r.get(0),
        )?;
        let launch_rate_available = launch_rate_available(&tx, tier, now)?;
        let data = m8s_api::StoredLaunchGates {
            tier: tier.into(),
            evaluated_at: now,
            desired,
            stored_node_count,
            stored_deleting_count,
            stored_phases: phases.into_values().collect(),
            launches_after_cutoff,
            future_attempt_present,
            launch_rate_available,
            rate_limit: 3,
            window_seconds: 3600,
            cutoff_exclusive: cutoff,
        };
        tx.commit()?;
        Ok(data)
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
    /// Read one SELECT snapshot of this state's history. No continuity across
    /// databases, current-clock, event-loss or later-change inclusion guarantee.
    pub fn log_page(&self, after: u64, limit: u32) -> Result<(Vec<m8s_api::LogEvent>, bool)> {
        let after = i64::try_from(after).map_err(|_| Error::Invalid)?;
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid);
        }
        // Reject oversized/non-text messages before materializing their contents.
        // The extra row is checked too; no truncation/omission advances a cursor.
        let mut query = self.conn.prepare("SELECT seq,at,CASE WHEN typeof(message)='text' AND length(CAST(message AS BLOB))<=4096 THEN message ELSE NULL END FROM event WHERE seq>? ORDER BY seq ASC LIMIT ?")?;
        let mut records = query
            .query_map(params![after, i64::from(limit) + 1], |row| {
                Ok(m8s_api::LogEvent {
                    seq: row.get(0)?,
                    at: row.get(1)?,
                    message: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let has_more = records.len() > limit as usize;
        records.truncate(limit as usize);
        Ok((records, has_more))
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
        if !launch_rate_available(&tx, tier, now)? {
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
pub mod operation;

mod private;

#[cfg(test)]
mod batch_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            use std::os::unix::fs::DirBuilderExt;
            let root = std::env::temp_dir().join(format!(
                "m8batch-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .unwrap();
            Self(root)
        }
        fn store(&self) -> Store {
            Store::open(self.0.join("state.db"), "synthetic-owner").unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn atomic_manual_changes_validate_all_and_reopen_retains_unmentioned() {
        let f = Fixture::new();
        let s = f.store();
        s.scale("untouched", 3).unwrap();
        s.scale_batch(&[("a".into(), 2), ("b".into(), 0)]).unwrap();
        let before = s.tiers().unwrap();
        assert_eq!(
            before,
            vec![("a".into(), 2), ("b".into(), 0), ("untouched".into(), 3)]
        );
        for targets in [
            vec![],
            vec![("a".into(), 1); 33],
            vec![("a".into(), 1), ("a".into(), 0)],
            vec![("a".into(), 1), ("bad.id".into(), 2)],
            vec![("a".into(), 1), ("b".into(), 17)],
            vec![("x".repeat(65), 0)],
        ] {
            assert!(s.scale_batch(&targets).is_err());
            assert_eq!(s.tiers().unwrap(), before);
        }
        s.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(s.scale_batch(&[("a".into(), 0)]).is_err());
        assert_eq!(s.tiers().unwrap(), before);
        s.conn.execute_batch("ROLLBACK").unwrap();
        drop(s);
        assert_eq!(f.store().tiers().unwrap(), before);
    }
    #[test]
    fn sql_abort_and_ignored_writes_roll_back_desired_and_actual_nonce_cancellation() {
        use m8s_bootstrap::{EndpointId, LaunchBinding};
        use nonce::{LaunchIdentity, ObservationStatus};
        let f = Fixture::new();
        let mut s = f.store();
        let mut provider = m8s_provider::Mock::default();
        s.scale("first", 1).unwrap();
        s.reconcile(&mut provider, 0).unwrap();
        let node = s.nodes().unwrap().remove(0);
        let binding = LaunchBinding {
            owner: EndpointId([1; 32]),
            instance_id: node.instance_id.unwrap(),
            container_id: "synthetic-container".into(),
            provider_ip: "203.0.113.1".parse().unwrap(),
            image_sha256: [2; 32],
        };
        s.issue_bootstrap_nonce(
            &[7; 32],
            LaunchIdentity {
                owner: "synthetic-owner",
                node_uid: &node.uid,
                binding: &binding,
                observed_at: 10,
                status: ObservationStatus::Complete,
            },
            10,
        )
        .unwrap();
        s.conn.execute_batch("CREATE TEMP TRIGGER fail_second BEFORE INSERT ON tier WHEN NEW.name='second' BEGIN SELECT RAISE(ABORT,'synthetic'); END;").unwrap();
        assert!(
            s.scale_batch(&[("first".into(), 0), ("second".into(), 2)])
                .is_err()
        );
        assert_eq!(s.tiers().unwrap(), vec![("first".into(), 1)]);
        let a = s.bootstrap_attempts().unwrap();
        assert!(!a[0].cancelled && !a[0].cleanup_required);
        s.conn.execute_batch("DROP TRIGGER fail_second; CREATE TEMP TRIGGER ignore_second BEFORE INSERT ON tier WHEN NEW.name='second' BEGIN SELECT RAISE(IGNORE); END;").unwrap();
        assert!(
            s.scale_batch(&[("first".into(), 0), ("second".into(), 2)])
                .is_err()
        );
        assert_eq!(s.tiers().unwrap(), vec![("first".into(), 1)]);
        assert!(!s.bootstrap_attempts().unwrap()[0].cancelled);
        s.conn.execute_batch("DROP TRIGGER ignore_second; CREATE TEMP TRIGGER rewrite_first AFTER INSERT ON tier WHEN NEW.name='second' BEGIN UPDATE tier SET desired=1 WHERE name='first'; END;").unwrap();
        assert!(
            s.scale_batch(&[("first".into(), 0), ("second".into(), 2)])
                .is_err()
        );
        assert_eq!(s.tiers().unwrap(), vec![("first".into(), 1)]);
        assert!(!s.bootstrap_attempts().unwrap()[0].cancelled);
        let deleting: bool = s
            .conn
            .query_row(
                "SELECT deleting FROM gpu_node WHERE uid=?",
                [&node.uid],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!deleting);
        s.conn.execute_batch("DROP TRIGGER rewrite_first").unwrap();
        s.scale_batch(&[("first".into(), 0), ("second".into(), 2)])
            .unwrap();
        let a = s.bootstrap_attempts().unwrap();
        assert!(a[0].cancelled && !a[0].cleanup_required);
        assert_eq!(a[0].stage, nonce::BootstrapStage::Cancelled);
        assert_eq!(
            s.tiers().unwrap(),
            vec![("first".into(), 0), ("second".into(), 2)]
        );
    }
}

#[cfg(test)]
mod explanation_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            use std::os::unix::fs::DirBuilderExt;
            let path = std::env::temp_dir().join(format!(
                "m8ex-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .unwrap();
            Self(path)
        }
        fn store(&self) -> Store {
            Store::open(self.0.join("s.db"), "synthetic").unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn exact_existing_cutoff_future_and_absent_vs_zero_are_read_only() {
        let f = Fixture::new();
        let s = f.store();
        let missing = s.explain_tier("t", 3600).unwrap();
        assert_eq!(missing.desired, None);
        assert_eq!(missing.stored_node_count, 0);
        assert!(missing.launch_rate_available);
        s.scale("t", 0).unwrap();
        for (id, at) in [("cutoff", 0), ("inside", 1), ("now", 3600)] {
            s.conn
                .execute(
                    "INSERT INTO launch_attempt VALUES (?,'t',?)",
                    params![id, at],
                )
                .unwrap();
        }
        let before = s.tiers().unwrap();
        let total: i64 = s
            .conn
            .query_row("SELECT total_changes()", [], |r| r.get(0))
            .unwrap();
        let d = s.explain_tier("t", 3600).unwrap();
        assert_eq!(d.desired, Some(0));
        assert_eq!(d.launches_after_cutoff, 2);
        assert!(d.launch_rate_available);
        assert!(!d.future_attempt_present);
        assert_eq!(d.cutoff_exclusive, 0);
        assert_eq!(
            d.launch_rate_available,
            launch_rate_available(&s.conn, "t", 3600).unwrap()
        );
        s.conn
            .execute("INSERT INTO launch_attempt VALUES ('future','t',3601)", [])
            .unwrap();
        let d = s.explain_tier("t", 3600).unwrap();
        assert_eq!(d.launches_after_cutoff, 3);
        assert!(d.future_attempt_present && !d.launch_rate_available);
        assert_eq!(s.tiers().unwrap(), before);
        assert_eq!(
            s.conn
                .query_row::<i64, _, _>("SELECT total_changes()", [], |r| r.get(0))
                .unwrap(),
            total + 1
        );
        s.conn
            .execute("DELETE FROM launch_attempt WHERE uid='future'", [])
            .unwrap();
        s.conn
            .execute("INSERT INTO launch_attempt VALUES ('third','t',2)", [])
            .unwrap();
        assert!(!s.explain_tier("t", 3600).unwrap().launch_rate_available);
        let edge = s.explain_tier("absent", i64::MAX).unwrap();
        assert_eq!(edge.cutoff_exclusive, i64::MAX - 3600);
        assert!(edge.launch_rate_available);
    }
    #[test]
    fn all_retained_phases_corruption_and_nested_reads_fail_without_writes() {
        let f = Fixture::new();
        let s = f.store();
        let phases = [
            Phase::Pending,
            Phase::Searching,
            Phase::Launching,
            Phase::Booting,
            Phase::Bootstrapped,
            Phase::Loading,
            Phase::Ready,
            Phase::Draining,
            Phase::Destroying,
            Phase::Gone,
            Phase::Failed,
            Phase::Orphaned,
            Phase::Paused,
        ];
        for (i, p) in phases.into_iter().enumerate() {
            s.conn
                .execute(
                    "INSERT INTO gpu_node(uid,tier,phase,created_at,deleting) VALUES (?,'t',?,0,?)",
                    params![
                        format!("n{i}"),
                        serde_json::to_string(&p).unwrap(),
                        i % 2 == 1
                    ],
                )
                .unwrap();
        }
        let d = s.explain_tier("t", 0).unwrap();
        assert_eq!(d.stored_node_count, 13);
        assert_eq!(d.stored_deleting_count, 6);
        assert_eq!(d.stored_phases.len(), 13);
        assert_eq!(d.desired, None);
        assert_eq!(d.cutoff_exclusive, -3600);
        let changes: i64 = s
            .conn
            .query_row("SELECT total_changes()", [], |r| r.get(0))
            .unwrap();
        s.conn.execute_batch("BEGIN DEFERRED").unwrap();
        assert!(s.explain_tier("t", 0).is_err());
        s.conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            s.conn
                .query_row::<i64, _, _>("SELECT total_changes()", [], |r| r.get(0))
                .unwrap(),
            changes
        );
        s.conn
            .execute("UPDATE gpu_node SET phase='not-a-phase' WHERE uid='n0'", [])
            .unwrap();
        assert!(s.explain_tier("t", 0).is_err());
        assert_eq!(
            s.conn
                .query_row::<String, _, _>("SELECT phase FROM gpu_node WHERE uid='n0'", [], |r| r
                    .get(0))
                .unwrap(),
            "not-a-phase"
        );
        assert!(s.explain_tier("bad.id", 0).is_err());
        assert!(s.explain_tier("t", -1).is_err());
    }
}
