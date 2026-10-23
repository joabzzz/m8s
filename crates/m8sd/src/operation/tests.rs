//! Actual SQLite/typed-claim tests with synthetic trusted cost/result metadata.
//! No executor, provider, transport or spend qualification.
use super::*;
use crate::budget::*;
use std::{
    path::PathBuf,
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn path() -> PathBuf {
    use std::os::unix::fs::DirBuilderExt;
    let p = std::env::temp_dir().join(format!(
        "m8sd-operation-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
    p.join("state.db")
}
fn period() -> CalendarPeriod {
    CalendarPeriod {
        id: "synthetic-month".into(),
        starts_at: 0,
        ends_at: 28 * 86400,
    }
}
fn policy() -> AdminBudget {
    AdminBudget {
        owner: "owner".into(),
        approval_id: "synthetic-not-real-approval".into(),
        baseline: AccountBaseline {
            prior_lifetime_spent: 0,
            prior_current_period_spent: 0,
            initial_period: period(),
            external_liability: ExternalLiability::NoneConfirmed,
            observed_at: 100,
            status: EvidenceStatus::Complete,
            provenance: "synthetic-baseline".into(),
        },
        valid_from: 0,
        valid_until: 86400,
        trial_all_in: 10000,
        monthly_all_in: 10000,
        total_hourly: 20000,
        instance_hourly: 10000,
        max_instances: 2,
        max_runtime_seconds: 1000,
        minimum_cleanup_seconds: 60,
        minimum_contingency: 100,
    }
}
fn request(id: &str) -> BudgetRequest {
    BudgetRequest {
        owner: "owner".into(),
        run_id: "run".into(),
        request_id: id.into(),
        node_uid: format!("node-{id}"),
        quote: CostQuote {
            provenance: "synthetic-bounded-rates".into(),
            observed_at: 100,
            valid_until: 2000,
            status: EvidenceStatus::Complete,
            compute_unit: RateUnit::MicroUsdPerHour,
            storage_unit: RateUnit::MicroUsdPerHour,
            totals_unit: TotalUnit::MicroUsd,
            compute_hourly: 3600,
            storage_hourly: 360,
            transfer_total: 500,
            runtime_seconds: 1000,
            cleanup_seconds: 60,
            contingency: 100,
        },
    }
}
fn intent(id: &str) -> CreateIntent {
    CreateIntent {
        owner: "owner".into(),
        run_id: "run".into(),
        request_id: id.into(),
        node_uid: format!("node-{id}"),
        operation_id: format!("op-{id}"),
        launch_fingerprint: [4; 32],
    }
}
fn seed(s: &Store, id: &str) {
    s.initialize_budget(&policy(), &period(), 100).unwrap();
    s.scale("tier", 1).unwrap();
    s.conn
        .execute(
            "INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES (?,'tier','\"Launching\"',100)",
            [format!("node-{id}")],
        )
        .unwrap();
    s.reserve_budget(&policy(), &period(), &request(id), 100)
        .unwrap();
}
fn fixture() -> (PathBuf, Store) {
    let p = path();
    let s = Store::open(&p, "owner").unwrap();
    seed(&s, "a");
    (p, s)
}
fn result(id: &str) -> ResourceBinding {
    ResourceBinding {
        owner: "owner".into(),
        run_id: "run".into(),
        node_uid: format!("node-{id}"),
        instance_id: format!("instance-{id}"),
        volumes: VolumeBinding::Ids(vec![format!("volume-{id}")]),
        provenance: "synthetic-complete-owned-resource-observation".into(),
        observed_at: 102,
        status: EvidenceStatus::Complete,
    }
}
#[test]
fn one_claim_burns_dispatch_before_return_and_metadata_never_reissues() {
    let (_, s) = fixture();
    let prepared = s.prepare_create(&intent("a"), 100).unwrap();
    assert_eq!(prepared.state, CreateState::Prepared);
    let changes = s.conn.total_changes();
    assert_eq!(s.prepare_create(&intent("a"), 100).unwrap(), prepared);
    assert_eq!(s.conn.total_changes(), changes);
    let claim = s.claim_create("op-a", 101).unwrap();
    assert_eq!(claim.intent(), &intent("a"));
    assert_eq!(claim.serving_deadline(), 1100);
    assert_eq!(claim.cleanup_deadline(), 1160);
    let r = s.create_operations().unwrap().remove(0);
    assert_eq!(r.state, CreateState::DispatchConsumed);
    assert_eq!(r.dispatched_at, Some(101));
    assert!(r.cleanup_required);
    assert_eq!(
        s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
    drop(claim);
    assert!(s.claim_create("op-a", 102).is_err());
    assert_eq!(
        s.prepare_create(&intent("a"), 102).unwrap().state,
        CreateState::DispatchConsumed
    );
}
#[test]
fn two_actual_connections_only_one_claim() {
    let (path, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    drop(s);
    let gate = Arc::new(Barrier::new(2));
    let ts: Vec<_> = (0..2)
        .map(|_| {
            let p = path.clone();
            let g = gate.clone();
            std::thread::spawn(move || {
                let s = Store::open(p, "owner").unwrap();
                g.wait();
                s.claim_create("op-a", 101).is_ok()
            })
        })
        .collect();
    assert_eq!(
        ts.into_iter()
            .map(|t| u32::from(t.join().unwrap()))
            .sum::<u32>(),
        1
    );
}
#[test]
fn claim_cancel_race_is_serialized_and_cancel_is_irreversible() {
    let (path, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    drop(s);
    let gate = Arc::new(Barrier::new(2));
    let p = path.clone();
    let g = gate.clone();
    let claim = std::thread::spawn(move || {
        let s = Store::open(p, "owner").unwrap();
        g.wait();
        s.claim_create("op-a", 101).is_ok()
    });
    let p = path.clone();
    let cancel = std::thread::spawn(move || {
        let s = Store::open(p, "owner").unwrap();
        gate.wait();
        s.cancel_create("op-a", 101).unwrap()
    });
    let minted = claim.join().unwrap();
    let r = cancel.join().unwrap();
    assert_eq!(r.state, CreateState::Cancelled);
    assert_eq!(r.dispatched_at.is_some(), minted);
    assert_eq!(r.cleanup_required, minted);
    let s = Store::open(path, "owner").unwrap();
    assert!(s.claim_create("op-a", 102).is_err());
    assert_eq!(
        s.prepare_create(&intent("a"), 102).unwrap().state,
        CreateState::Cancelled
    );
}
#[test]
fn restart_drop_unwind_timeout_and_ambiguous_result_never_reissue() {
    let (path, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _claim = s.claim_create("op-a", 101).unwrap();
        panic!("synthetic effect-boundary unwind");
    }));
    assert!(panic.is_err());
    drop(s);
    let s = Store::open(path, "owner").unwrap();
    assert!(s.claim_create("op-a", 102).is_err());
    s.record_create_result("op-a", &CreateObservation::Ambiguous, 102)
        .unwrap();
    assert!(s.claim_create("op-a", 10000).is_err());
    let r = s.create_operations().unwrap().remove(0);
    assert!(r.uncertainty_observed && r.cleanup_required);
    assert_eq!(r.dispatched_at, Some(101));
}
#[test]
fn cancellation_preclaim_and_existing_down_deletion_preempt_dispatch() {
    for how in 0..4 {
        let (_, s) = fixture();
        s.prepare_create(&intent("a"), 100).unwrap();
        match how {
            0 => {
                s.cancel_create("op-a", 101).unwrap();
            }
            1 => {
                s.down().unwrap();
                s.scale("tier", 1).unwrap();
            }
            2 => {
                s.conn
                    .execute("DELETE FROM gpu_node WHERE uid='node-a'", [])
                    .unwrap();
            }
            _ => {
                s.scale("tier", 0).unwrap();
            }
        }
        assert!(s.claim_create("op-a", 102).is_err());
        assert!(s.create_operations().unwrap()[0].dispatched_at.is_none());
    }
}
#[test]
fn postclaim_cancel_keeps_cleanup_and_accepts_only_exact_recovery_binding() {
    let (_, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    let _claim = s.claim_create("op-a", 101).unwrap();
    let r = s.cancel_create("op-a", 102).unwrap();
    assert!(r.cleanup_required);
    let bound = s
        .record_create_result("op-a", &CreateObservation::Bound(result("a")), 102)
        .unwrap();
    assert_eq!(bound.state, CreateState::Cancelled);
    assert!(bound.cleanup_required);
    assert_eq!(bound.result, Some(result("a")));
    assert!(s.claim_create("op-a", 103).is_err());
    assert_eq!(
        s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
    let mut changed = result("a");
    changed.instance_id = "different".into();
    assert!(
        s.record_create_result("op-a", &CreateObservation::Bound(changed), 103)
            .is_err()
    );
}
#[test]
fn changed_retry_operation_or_launch_fingerprint_cannot_rebind_budget() {
    let (_, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    for kind in 0..5 {
        let mut i = intent("a");
        match kind {
            0 => i.launch_fingerprint[0] ^= 1,
            1 => i.operation_id = "other".into(),
            2 => i.run_id = "other".into(),
            3 => i.node_uid = "other".into(),
            _ => i.owner = "foreign".into(),
        };
        assert!(s.prepare_create(&i, 100).is_err());
    }
    assert_eq!(s.create_operations().unwrap().len(), 1);
}
#[test]
fn missing_corrupt_or_changed_budget_never_mints_claim() {
    for kind in 0..7 {
        let (_, s) = fixture();
        s.prepare_create(&intent("a"), 100).unwrap();
        match kind {
            0 => {
                s.conn
                    .execute("DELETE FROM budget_reservation", [])
                    .unwrap();
            }
            1 => {
                s.conn
                    .execute("UPDATE budget_reservation SET record='not-json'", [])
                    .unwrap();
            }
            2 => {
                let mut b = s.budget_reservations().unwrap().remove(0);
                b.reserved_all_in = 1;
                s.conn
                    .execute(
                        "UPDATE budget_reservation SET record=?",
                        [encode(&b).unwrap()],
                    )
                    .unwrap();
            }
            3 => {
                s.conn.execute("DELETE FROM budget_account", []).unwrap();
            }
            4 => {
                s.conn
                    .execute("UPDATE budget_account SET record='corrupt'", [])
                    .unwrap();
            }
            5 => {
                s.mark_budget_uncertain("a", 101).unwrap();
            }
            _ => {
                let mut b = s.budget_reservations().unwrap().remove(0);
                b.request.node_uid = "foreign".into();
                s.conn
                    .execute(
                        "UPDATE budget_reservation SET record=?",
                        [encode(&b).unwrap()],
                    )
                    .unwrap();
            }
        };
        assert!(s.claim_create("op-a", 101).is_err());
        assert!(s.create_operations().unwrap()[0].dispatched_at.is_none());
    }
}
#[test]
fn result_before_claim_or_wrong_partial_stale_binding_is_refused() {
    let (_, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    assert!(
        s.record_create_result("op-a", &CreateObservation::Bound(result("a")), 102)
            .is_err()
    );
    s.claim_create("op-a", 101).unwrap();
    for kind in 0..6 {
        let mut b = result("a");
        match kind {
            0 => b.owner = "foreign".into(),
            1 => b.status = EvidenceStatus::Partial,
            2 => b.observed_at = 103,
            3 => b.observed_at = 71,
            4 => b.volumes = VolumeBinding::Ids(vec![]),
            _ => b.node_uid = "other".into(),
        };
        assert!(
            s.record_create_result("op-a", &CreateObservation::Bound(b), 102)
                .is_err()
        );
    }
    s.record_create_result("op-a", &CreateObservation::Bound(result("a")), 102)
        .unwrap();
    let n = s.conn.total_changes();
    s.record_create_result("op-a", &CreateObservation::Bound(result("a")), 102)
        .unwrap();
    assert_eq!(s.conn.total_changes(), n);
}
#[test]
fn unknown_other_work_or_late_bill_blocks_already_prepared_dispatch() {
    for settle in [false, true] {
        let (_, s) = fixture();
        seed(&s, "b");
        s.prepare_create(&intent("a"), 100).unwrap();
        s.prepare_create(&intent("b"), 100).unwrap();
        if settle {
            s.bind_budget_resources(
                "b",
                &ResourceBinding {
                    observed_at: 100,
                    ..result("b")
                },
                100,
            )
            .unwrap();
            let e = SettlementEvidence {
                binding: ResourceBinding {
                    observed_at: 100,
                    ..result("b")
                },
                observed_at: 100,
                status: EvidenceStatus::Complete,
                resource: ResourceOutcome::OwnedAbsent,
                volumes: VolumeOutcome::BoundVolumesAbsent,
                billing: FinalBilling::Known {
                    actual_all_in: 10000,
                    provenance: "synthetic-late-overrun".into(),
                },
                provenance: "synthetic-complete-absence".into(),
            };
            s.settle_budget("b", &period(), &e, 100).unwrap();
        } else {
            s.mark_budget_uncertain("b", 100).unwrap();
        }
        assert!(s.claim_create("op-a", 101).is_err());
    }
}
#[test]
fn future_rollback_expired_quote_and_nested_transactions_refuse_before_dispatch() {
    let (_, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    for now in [-1, 99, 131, 1100, 1160] {
        assert!(s.claim_create("op-a", now).is_err());
    }
    s.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(s.claim_create("op-a", 101).is_err());
    s.conn.execute_batch("ROLLBACK").unwrap();
    assert!(s.create_operations().unwrap()[0].dispatched_at.is_none());
}

#[test]
fn held_claim_has_no_transaction_and_other_connection_can_cancel_before_fake_effect() {
    let (path, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    let claim = s.claim_create("op-a", 101).unwrap();
    assert!(s.conn.is_autocommit());
    let other = Store::open(path, "owner").unwrap();
    let r = other.cancel_create("op-a", 102).unwrap();
    assert!(r.cleanup_required && r.dispatched_at.is_some());
    // No fake effect is invoked: outstanding authority cannot be recalled by a
    // DB transaction; cancellation already owes cleanup for possible dispatch.
    assert_eq!(claim.intent(), &intent("a"));
    drop(claim);
    assert!(s.claim_create("op-a", 103).is_err());
}
#[test]
fn deleting_operation_record_does_not_reset_consumed_budget_or_rerent_after_restart() {
    let (path, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    s.claim_create("op-a", 101).unwrap();
    s.conn.execute("DELETE FROM create_operation", []).unwrap();
    drop(s);
    let s = Store::open(path, "owner").unwrap();
    assert!(s.prepare_create(&intent("a"), 102).is_err());
    assert!(s.claim_create("op-a", 102).is_err());
    assert_eq!(
        s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
}
#[test]
fn invalid_arithmetic_or_missing_period_also_refuses_initial_preparation() {
    for kind in 0..5 {
        let (_, s) = fixture();
        match kind {
            0 => {
                s.conn.execute("DELETE FROM budget_period", []).unwrap();
            }
            1 => {
                let mut b = s.budget_reservations().unwrap().remove(0);
                b.reserved_all_in -= 1;
                s.conn
                    .execute(
                        "UPDATE budget_reservation SET record=?",
                        [encode(&b).unwrap()],
                    )
                    .unwrap();
            }
            2 => {
                let mut b = s.budget_reservations().unwrap().remove(0);
                b.request.quote.runtime_seconds = u64::MAX;
                s.conn
                    .execute(
                        "UPDATE budget_reservation SET record=?",
                        [encode(&b).unwrap()],
                    )
                    .unwrap();
            }
            3 => {
                s.conn
                    .execute("UPDATE budget_reservation SET record='corrupt'", [])
                    .unwrap();
            }
            _ => {
                s.conn.execute("DELETE FROM budget_account", []).unwrap();
            }
        }
        assert!(s.prepare_create(&intent("a"), 100).is_err());
        assert!(s.create_operations().unwrap().is_empty());
    }
}
#[test]
fn ambiguous_then_bound_recovery_stays_uncertain_and_never_refunds_or_reissues() {
    let (path, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    s.claim_create("op-a", 101).unwrap();
    s.record_create_result("op-a", &CreateObservation::Ambiguous, 102)
        .unwrap();
    drop(s);
    let s = Store::open(path, "owner").unwrap();
    let r = s
        .record_create_result("op-a", &CreateObservation::Bound(result("a")), 102)
        .unwrap();
    assert!(r.uncertainty_observed && r.cleanup_required);
    assert!(s.claim_create("op-a", 103).is_err());
    seed_second_uncertain_check(&s);
}
fn seed_second_uncertain_check(s: &Store) {
    assert!(
        s.reserve_budget(&policy(), &period(), &request("b"), 103)
            .is_err()
    );
    assert_eq!(
        s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
}
#[test]
fn operation_record_corruption_and_pre_dispatch_observation_fail_closed() {
    let (_, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    s.claim_create("op-a", 101).unwrap();
    let mut observed = result("a");
    observed.observed_at = 100;
    assert!(
        s.record_create_result("op-a", &CreateObservation::Bound(observed), 102)
            .is_err()
    );
    s.conn
        .execute("UPDATE create_operation SET record='corrupt'", [])
        .unwrap();
    assert!(s.claim_create("op-a", 102).is_err());
    assert!(s.create_operations().is_err());
}

#[test]
fn cross_operation_claim_race_mints_only_one_before_global_uncertainty_blocks_other() {
    let (path, s) = fixture();
    seed(&s, "b");
    s.prepare_create(&intent("a"), 100).unwrap();
    s.prepare_create(&intent("b"), 100).unwrap();
    drop(s);
    let gate = Arc::new(Barrier::new(2));
    let ts: Vec<_> = ["op-a", "op-b"]
        .into_iter()
        .map(|id| {
            let path = path.clone();
            let g = gate.clone();
            std::thread::spawn(move || {
                let s = Store::open(path, "owner").unwrap();
                g.wait();
                s.claim_create(id, 101).is_ok()
            })
        })
        .collect();
    assert_eq!(
        ts.into_iter()
            .map(|t| u32::from(t.join().unwrap()))
            .sum::<u32>(),
        1
    );
    let s = Store::open(path, "owner").unwrap();
    assert_eq!(
        s.create_operations()
            .unwrap()
            .iter()
            .filter(|r| r.dispatched_at.is_some())
            .count(),
        1
    );
    assert!(s.claim_create("op-a", 102).is_err());
    assert!(s.claim_create("op-b", 102).is_err());
}
#[test]
fn concurrent_fresh_reserve_and_other_claim_serialize_without_stale_capacity_grant() {
    let (path, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    drop(s);
    let gate = Arc::new(Barrier::new(2));
    let path_a = path.clone();
    let g = gate.clone();
    let claim = std::thread::spawn(move || {
        let s = Store::open(path_a, "owner").unwrap();
        g.wait();
        s.claim_create("op-a", 101).is_ok()
    });
    let path_b = path.clone();
    let reserve = std::thread::spawn(move || {
        let s = Store::open(path_b, "owner").unwrap();
        gate.wait();
        s.reserve_budget(&policy(), &period(), &request("b"), 101)
            .is_ok()
    });
    assert!(claim.join().unwrap());
    let reserved = reserve.join().unwrap();
    let s = Store::open(path, "owner").unwrap();
    let rows = s.budget_reservations().unwrap();
    assert_eq!(rows.len(), if reserved { 2 } else { 1 });
    assert_eq!(
        rows.iter()
            .filter(|r| r.state == BudgetState::Uncertain)
            .count(),
        1
    );
    // If reservation won the lock first it remains only metadata, not permission
    // to dispatch around the first operation's now-durable uncertainty.
    if reserved {
        s.conn.execute("INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES ('node-b','tier','\"Launching\"',100)",[]).unwrap();
        assert!(s.prepare_create(&intent("b"), 102).is_err());
        assert!(s.claim_create("op-b", 102).is_err());
    }
}
#[test]
fn delayed_claim_keeps_positive_original_runtime_and_entire_cleanup_grace() {
    for now in [109, 110] {
        let p = path();
        let s = Store::open(p, "owner").unwrap();
        s.scale("tier", 1).unwrap();
        let mut caps = policy();
        caps.instance_hourly = 100000;
        caps.total_hourly = 200000;
        let mut r = request("a");
        r.quote.runtime_seconds = 10;
        r.quote.transfer_total = 0;
        r.quote.valid_until = 170;
        s.reserve_budget(&caps, &period(), &r, 100).unwrap();
        s.conn.execute("INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES ('node-a','tier','\"Launching\"',100)",[]).unwrap();
        s.prepare_create(&intent("a"), 100).unwrap();
        let claim = s.claim_create("op-a", now);
        if now == 109 {
            let claim = claim.unwrap();
            assert_eq!(claim.serving_deadline() - now, 1);
            assert_eq!(claim.cleanup_deadline() - claim.serving_deadline(), 60);
            assert_eq!(claim.cleanup_deadline(), 170);
        } else {
            assert!(claim.is_err());
            assert!(s.create_operations().unwrap()[0].dispatched_at.is_none());
        }
    }
}
#[test]
fn cancellation_and_later_binding_never_clear_ambiguity_or_unknown_billing() {
    let (_, s) = fixture();
    s.prepare_create(&intent("a"), 100).unwrap();
    s.claim_create("op-a", 101).unwrap();
    s.record_create_result("op-a", &CreateObservation::Ambiguous, 102)
        .unwrap();
    s.cancel_create("op-a", 102).unwrap();
    let r = s
        .record_create_result("op-a", &CreateObservation::Bound(result("a")), 102)
        .unwrap();
    assert_eq!(r.state, CreateState::Cancelled);
    assert!(r.uncertainty_observed && r.cleanup_required);
    let e = SettlementEvidence {
        binding: result("a"),
        observed_at: 103,
        status: EvidenceStatus::Complete,
        resource: ResourceOutcome::OwnedAbsent,
        volumes: VolumeOutcome::BoundVolumesAbsent,
        billing: FinalBilling::Unknown,
        provenance: "synthetic-complete-absence-unknown-bill".into(),
    };
    s.settle_budget("a", &period(), &e, 103).unwrap();
    s.record_create_result("op-a", &CreateObservation::Ambiguous, 104)
        .unwrap();
    assert_eq!(
        s.budget_reservations().unwrap()[0].state,
        BudgetState::BillingUnknown
    );
    assert!(s.claim_create("op-a", 105).is_err());
}
