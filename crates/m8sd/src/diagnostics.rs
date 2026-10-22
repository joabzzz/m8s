//! Read-only stored launch-gate observations, not launch authority.
use crate::{Error, Result, Store, launch_rate_available};
use m8s_core::{Phase, Timestamp};
use rusqlite::{OptionalExtension, params};

impl Store {
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
