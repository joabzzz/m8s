//! Synthetic accounting and SQLite only. Never constructs an HTTPS client.
use super::*;
use crate::budget::*;
use std::{
    num::NonZeroU64,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn executor(offer: u64) -> VastExecutor {
    let identity = RecoveryIntent::new(
        "owner",
        "node",
        "smoke",
        &format!("example.invalid/mesh@sha256:{}", "a".repeat(64)),
    )
    .unwrap();
    let key = format!(
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI{}",
        "A".repeat(68 - "AAAAC3NzaC1lZDI1NTE5AAAAI".len())
    );
    VastExecutor::new(
        LaunchRequest::new(NonZeroU64::new(offer).unwrap(), identity, key, 10).unwrap(),
        "owner",
        "node",
        "smoke",
    )
    .unwrap()
}
fn setup(cap: u64, instances: u32) -> (Store, Directory) {
    let path = std::env::temp_dir().join(format!(
        "m8sd-vast-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    let store = Store::open(path.join("state.db"), "owner").unwrap();
    store.scale("smoke", 1).unwrap();
    let period = CalendarPeriod {
        id: "synthetic-month".into(),
        starts_at: 0,
        ends_at: 28 * 86400,
    };
    let policy = AdminBudget {
        owner: "owner".into(),
        approval_id: "synthetic-only".into(),
        baseline: AccountBaseline {
            prior_lifetime_spent: 0,
            prior_current_period_spent: 0,
            initial_period: period.clone(),
            external_liability: ExternalLiability::NoneConfirmed,
            observed_at: 100,
            status: EvidenceStatus::Complete,
            provenance: "synthetic".into(),
        },
        valid_from: 0,
        valid_until: 86400,
        trial_all_in: cap,
        monthly_all_in: cap,
        total_hourly: 30000,
        instance_hourly: 30000,
        max_instances: instances,
        max_runtime_seconds: 600,
        minimum_cleanup_seconds: 240,
        minimum_contingency: 100,
    };
    let request = BudgetRequest {
        owner: "owner".into(),
        run_id: "run".into(),
        request_id: "request".into(),
        node_uid: "node".into(),
        quote: CostQuote {
            provenance: "synthetic".into(),
            observed_at: 100,
            valid_until: 2000,
            status: EvidenceStatus::Complete,
            compute_unit: RateUnit::MicroUsdPerHour,
            storage_unit: RateUnit::MicroUsdPerHour,
            totals_unit: TotalUnit::MicroUsd,
            compute_hourly: 3600,
            storage_hourly: 0,
            transfer_total: 0,
            runtime_seconds: 60,
            cleanup_seconds: 240,
            contingency: 100,
        },
    };
    store
        .reserve_budget(&policy, &period, &request, 100)
        .unwrap();
    (store, Directory(path))
}
#[test]
fn prepare_is_atomic_idempotent_and_never_reuses_cancelled_history() {
    let (s, _directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = executor.intent("run", "request", "operation").unwrap();
    let prepared = executor.prepare_at(&s, &intent, || Ok(100)).unwrap();
    assert_eq!(prepared.state, CreateState::Prepared);
    assert_eq!(
        executor.prepare_at(&s, &intent, || Ok(100)).unwrap(),
        prepared
    );
    assert_eq!(
        s.conn
            .query_row("SELECT count(*) FROM gpu_node", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    s.cancel_create("operation", 101).unwrap();
    assert_eq!(
        executor.prepare_at(&s, &intent, || Ok(101)).unwrap().state,
        CreateState::Cancelled
    );
    let fresh = executor.intent("run2", "request2", "operation2").unwrap();
    assert!(executor.prepare_at(&s, &fresh, || Ok(101)).is_err());
    assert_eq!(s.create_operations().unwrap().len(), 1);
}
#[test]
fn excess_caps_and_multiple_instances_refuse_before_launch_row() {
    for (cap, instances) in [(6_000_001, 1), (6_000_000, 2)] {
        let (s, _directory) = setup(cap, instances);
        let executor = executor(42);
        let intent = executor.intent("run", "request", "operation").unwrap();
        assert!(executor.prepare_at(&s, &intent, || Ok(100)).is_err());
        assert!(s.create_operations().unwrap().is_empty());
        assert_eq!(
            s.conn
                .query_row("SELECT count(*) FROM gpu_node", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
#[test]
fn changed_offer_and_cancelled_desired_do_not_prepare() {
    let (s, _directory) = setup(6_000_000, 1);
    let first = executor(42);
    let second = executor(43);
    let intent = first.intent("run", "request", "operation").unwrap();
    assert_ne!(
        intent.launch_fingerprint,
        second
            .intent("run", "request", "operation")
            .unwrap()
            .launch_fingerprint
    );
    assert!(second.prepare_at(&s, &intent, || Ok(100)).is_err());
    s.scale("smoke", 0).unwrap();
    assert!(first.prepare_at(&s, &intent, || Ok(100)).is_err());
    assert!(s.create_operations().unwrap().is_empty());
}

#[test]
fn cleanup_plan_uses_the_claims_original_serving_deadline() {
    let (store, _directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = executor.intent("run", "request", "operation").unwrap();
    executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
    let deadline = executor
        .cleanup_starts_at_with_clock(&store, &intent, || Ok(100))
        .unwrap();
    let claim = store
        .claim_create_bound(
            "operation",
            100,
            Some((
                executor.token.clone(),
                executor.fingerprint(&intent).unwrap(),
                "smoke",
            )),
        )
        .unwrap();
    assert_eq!(deadline, 160);
    assert_eq!(deadline, claim.serving_deadline() as u64);
    assert_ne!(deadline, claim.cleanup_deadline() as u64);
    assert!(
        executor
            .cleanup_starts_at_with_clock(&store, &intent, || Ok(100))
            .is_err()
    );
}
