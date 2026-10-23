//! Atomic manual desired intent, not provider convergence or authorization.
use crate::{Error, Result, Store, nonce};
use rusqlite::{OptionalExtension, params};

impl Store {
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
}

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
        let mut provider = crate::mock_provider::Mock::default();
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
