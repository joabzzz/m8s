//! Actual SQLite tests; all Admin/provider/billing observations are synthetic
//! trusted assertions. No paid create, credential, watcher or provider call.
use super::*;
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
        "m8sd-budget-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
    p.join("state.db")
}
fn policy() -> AdminBudget {
    AdminBudget {
        owner: "owner".into(),
        valid_from: 0,
        valid_until: 90 * 86400,
        baseline: AccountBaseline {
            prior_lifetime_spent: 0,
            prior_current_period_spent: 0,
            initial_period: period(),
            external_liability: ExternalLiability::NoneConfirmed,
            observed_at: 100,
            status: EvidenceStatus::Complete,
            provenance: "synthetic-explicit-initial-cost-reconciliation".into(),
        },
        approval_id: "synthetic-explicit-admin-envelope-not-real-approval".into(),
        trial_all_in: 2_000_000,
        monthly_all_in: 1_000_000,
        total_hourly: 20_000,
        instance_hourly: 10_000,
        max_instances: 1,
        max_runtime_seconds: 3600,
        minimum_cleanup_seconds: 60,
        minimum_contingency: 100,
    }
}
fn period() -> CalendarPeriod {
    CalendarPeriod {
        id: "synthetic-month-1".into(),
        starts_at: 0,
        ends_at: 28 * 86400,
    }
}
fn request(id: &str) -> BudgetRequest {
    BudgetRequest {
        owner: "owner".into(),
        run_id: "synthetic-run".into(),
        request_id: id.into(),
        node_uid: format!("node-{id}"),
        quote: CostQuote {
            provenance: "synthetic-all-components-quote".into(),
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
fn binding(id: &str) -> ResourceBinding {
    ResourceBinding {
        owner: "owner".into(),
        run_id: "synthetic-run".into(),
        node_uid: format!("node-{id}"),
        instance_id: format!("instance-{id}"),
        volumes: VolumeBinding::Ids(vec![format!("volume-{id}")]),
        provenance: "synthetic-exact-owned-resource-binding".into(),
        observed_at: 101,
        status: EvidenceStatus::Complete,
    }
}
fn evidence(id: &str, actual: MicroUsd) -> SettlementEvidence {
    SettlementEvidence {
        binding: binding(id),
        observed_at: 102,
        status: EvidenceStatus::Complete,
        resource: ResourceOutcome::OwnedAbsent,
        volumes: VolumeOutcome::BoundVolumesAbsent,
        billing: FinalBilling::Known {
            actual_all_in: actual,
            provenance: "synthetic-final-bill-all-components".into(),
        },
        provenance: "synthetic-complete-owned-instance-and-volume-absence".into(),
    }
}
fn fixture() -> (PathBuf, Store) {
    let p = path();
    let s = Store::open(&p, "owner").unwrap();
    (p, s)
}
#[test]
fn all_in_ceil_cleanup_transfer_contingency_and_checked_overflow() {
    let p = policy();
    let q = request("a").quote;
    assert_eq!(costs(&q, &p, 100).unwrap(), (1766, 6358));
    assert_eq!(ceil_hourly(1, 1).unwrap(), 1);
    assert_eq!(ceil_hourly(u64::MAX, 3600).unwrap(), u64::MAX);
    assert!(ceil_hourly(u64::MAX, u64::MAX).is_err());
    let mut p = p;
    p.instance_hourly = u64::MAX;
    p.max_runtime_seconds = u64::MAX;
    let mut q = q;
    q.runtime_seconds = u64::MAX;
    assert!(costs(&q, &p, 100).is_err());
    q.runtime_seconds = 1000;
    q.transfer_total = u64::MAX;
    assert!(costs(&q, &p, 100).is_err());
    q.transfer_total = 0;
    q.compute_hourly = u64::MAX;
    q.storage_hourly = 1;
    assert!(costs(&q, &p, 100).is_err());
}
#[test]
fn exact_retry_no_mutation_changed_fingerprint_and_uid_refused_after_restart() {
    let (p, s) = fixture();
    let r = request("a");
    let reserved = s.reserve_budget(&policy(), &period(), &r, 100).unwrap();
    let changes = s.conn.total_changes();
    assert_eq!(
        s.reserve_budget(&policy(), &period(), &r, 100).unwrap(),
        reserved
    );
    assert_eq!(s.conn.total_changes(), changes);
    let mut changed = r.clone();
    changed.quote.transfer_total += 1;
    assert!(
        s.reserve_budget(&policy(), &period(), &changed, 100)
            .is_err()
    );
    changed = r.clone();
    changed.request_id = "other".into();
    assert!(
        s.reserve_budget(&policy(), &period(), &changed, 100)
            .is_err()
    );
    s.down().unwrap();
    drop(s);
    let s = Store::open(&p, "owner").unwrap();
    s.scale("tier", 1).unwrap();
    assert_eq!(s.budget_reservations().unwrap(), vec![reserved.clone()]);
    assert_eq!(
        s.reserve_budget(&policy(), &period(), &r, 100).unwrap(),
        reserved
    );
    let mut other = policy();
    other.approval_id = "reset-attempt".into();
    assert!(
        s.reserve_budget(&other, &period(), &request("b"), 100)
            .is_err()
    );
}
#[test]
fn concurrency_two_connections_never_overreserve() {
    let (p, s) = fixture();
    drop(s);
    let gate = Arc::new(Barrier::new(2));
    let threads: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|id| {
            let p = p.clone();
            let gate = gate.clone();
            std::thread::spawn(move || {
                let s = Store::open(p, "owner").unwrap();
                gate.wait();
                s.reserve_budget(&policy(), &period(), &request(id), 100)
                    .is_ok()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| u32::from(t.join().unwrap()))
            .sum::<u32>(),
        1
    );
    assert_eq!(
        Store::open(p, "owner")
            .unwrap()
            .budget_reservations()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn unknown_partial_error_stale_future_quotes_and_clock_rollback_refused() {
    let (_, s) = fixture();
    for status in [
        EvidenceStatus::Partial,
        EvidenceStatus::Error,
        EvidenceStatus::Unknown,
    ] {
        let mut r = request("a");
        r.quote.status = status;
        assert!(s.reserve_budget(&policy(), &period(), &r, 100).is_err());
    }
    for observed in [-1, 69, 101] {
        let mut r = request("a");
        r.quote.observed_at = observed;
        assert!(s.reserve_budget(&policy(), &period(), &r, 100).is_err());
    }
    assert!(
        s.reserve_budget(&policy(), &period(), &request("a"), -1)
            .is_err()
    );
    s.reserve_budget(&policy(), &period(), &request("a"), 100)
        .unwrap();
    assert!(s.mark_budget_uncertain("a", 99).is_err());
    assert!(
        s.reserve_budget(&policy(), &period(), &request("a"), 99)
            .is_err()
    );
    let mut r = request("b");
    r.quote.observed_at = 101;
    r.quote.valid_until = 101;
    assert!(s.reserve_budget(&policy(), &period(), &r, 101).is_err());
}
#[test]
fn destroy_ack_empty_error_stopped_processdeath_are_not_absence_or_refund() {
    let (_, s) = fixture();
    s.reserve_budget(&policy(), &period(), &request("a"), 100)
        .unwrap();
    s.bind_budget_resources("a", &binding("a"), 101).unwrap();
    for outcome in [
        ResourceOutcome::DestroyAcknowledged,
        ResourceOutcome::EmptyList,
        ResourceOutcome::Stopped,
        ResourceOutcome::ProcessDead,
        ResourceOutcome::Unknown,
        ResourceOutcome::Partial,
        ResourceOutcome::Error,
    ] {
        let mut e = evidence("a", 0);
        e.resource = outcome;
        assert!(s.settle_budget("a", &period(), &e, 102).is_err());
    }
    for outcome in [
        VolumeOutcome::DeleteAcknowledged,
        VolumeOutcome::EmptyList,
        VolumeOutcome::Unknown,
        VolumeOutcome::Partial,
        VolumeOutcome::Error,
    ] {
        let mut e = evidence("a", 0);
        e.volumes = outcome;
        assert!(s.settle_budget("a", &period(), &e, 102).is_err());
    }
    for status in [
        EvidenceStatus::Partial,
        EvidenceStatus::Unknown,
        EvidenceStatus::Error,
    ] {
        let mut e = evidence("a", 0);
        e.status = status;
        assert!(s.settle_budget("a", &period(), &e, 102).is_err());
    }
    let mut e = evidence("a", 0);
    e.observed_at = 103;
    assert!(s.settle_budget("a", &period(), &e, 102).is_err());
    e.observed_at = 70;
    assert!(s.settle_budget("a", &period(), &e, 102).is_err());
    e = evidence("a", 0);
    e.binding.instance_id = "foreign".into();
    assert!(s.settle_budget("a", &period(), &e, 102).is_err());
    assert_eq!(
        s.budget_reservations().unwrap()[0].state,
        BudgetState::Reserved
    );
}
#[test]
fn final_known_billing_exact_retry_refunds_only_unused_and_retains_tombstones() {
    let (p, s) = fixture();
    let mut caps = policy();
    caps.trial_all_in = 2000;
    s.reserve_budget(&caps, &period(), &request("a"), 100)
        .unwrap();
    s.bind_budget_resources("a", &binding("a"), 101).unwrap();
    let e = evidence("a", 1000);
    s.settle_budget("a", &period(), &e, 102).unwrap();
    let changes = s.conn.total_changes();
    s.settle_budget("a", &period(), &e, 102).unwrap();
    assert_eq!(s.conn.total_changes(), changes);
    let mut changed = e.clone();
    changed.billing = FinalBilling::Known {
        actual_all_in: 0,
        provenance: "changed".into(),
    };
    assert!(s.settle_budget("a", &period(), &changed, 102).is_err());
    assert!(
        s.reserve_budget(&caps, &period(), &request("b"), 102)
            .is_err()
    );
    s.down().unwrap();
    drop(s);
    let s = Store::open(&p, "owner").unwrap();
    assert_eq!(s.budget_reservations().unwrap().len(), 1);
    assert!(
        s.reserve_budget(&caps, &period(), &request("b"), 102)
            .is_err()
    );
}
#[test]
fn unknown_actual_retains_full_reservation_and_blocks_fresh_spend() {
    let (p, s) = fixture();
    s.reserve_budget(&policy(), &period(), &request("a"), 100)
        .unwrap();
    s.bind_budget_resources("a", &binding("a"), 101).unwrap();
    let mut e = evidence("a", 0);
    e.billing = FinalBilling::Unknown;
    let row = s.settle_budget("a", &period(), &e, 102).unwrap();
    assert_eq!(row.state, BudgetState::BillingUnknown);
    assert_eq!(row.reserved_all_in, 1766);
    drop(s);
    let s = Store::open(&p, "owner").unwrap();
    assert!(
        s.reserve_budget(&policy(), &period(), &request("b"), 103)
            .is_err()
    );
    s.settle_budget("a", &period(), &evidence("a", 900), 103)
        .unwrap();
    s.reserve_budget(&policy(), &period(), &request("b"), 103)
        .unwrap();
}
#[test]
fn overrun_debt_is_recorded_even_above_approved_caps_and_blocks_new_spend() {
    let (_, s) = fixture();
    s.reserve_budget(&policy(), &period(), &request("a"), 100)
        .unwrap();
    s.bind_budget_resources("a", &binding("a"), 101).unwrap();
    s.settle_budget("a", &period(), &evidence("a", u64::MAX), 102)
        .unwrap();
    assert!(
        s.reserve_budget(&policy(), &period(), &request("b"), 102)
            .is_err()
    );
}
#[test]
fn explicit_period_rollover_carries_unresolved_and_never_resets_lifetime() {
    let (p, s) = fixture();
    let mut caps = policy();
    caps.max_instances = 2;
    caps.monthly_all_in = 3000;
    caps.trial_all_in = 5000;
    s.reserve_budget(&caps, &period(), &request("a"), 100)
        .unwrap();
    let next = CalendarPeriod {
        id: "synthetic-month-2".into(),
        starts_at: period().ends_at,
        ends_at: period().ends_at + 28 * 86400,
    };
    let mut b = request("b");
    b.quote.observed_at = next.starts_at;
    b.quote.valid_until = next.starts_at + 2000;
    assert!(
        s.reserve_budget(&caps, &next, &b, next.starts_at - 1)
            .is_err()
    );
    assert!(s.reserve_budget(&caps, &next, &b, next.starts_at).is_err()); // old unresolved counts against new month's cap
    s.bind_budget_resources("a", &binding("a"), 101).unwrap();
    s.settle_budget("a", &period(), &evidence("a", 2500), 102)
        .unwrap();
    drop(s);
    let s = Store::open(p, "owner").unwrap();
    s.reserve_budget(&caps, &next, &b, next.starts_at).unwrap();
    let mut old = request("c");
    old.quote.observed_at = 100;
    assert!(s.reserve_budget(&caps, &period(), &old, 103).is_err());
    let mut overlap = next.clone();
    overlap.id = "overlap".into();
    overlap.starts_at -= 1;
    assert!(
        s.reserve_budget(&caps, &overlap, &b, next.starts_at)
            .is_err()
    );
}
#[test]
fn immutable_resource_binding_and_ambiguous_create_are_recoverable_not_refundable() {
    let (_, s) = fixture();
    s.reserve_budget(&policy(), &period(), &request("a"), 100)
        .unwrap();
    s.mark_budget_uncertain("a", 101).unwrap();
    assert!(
        s.settle_budget("a", &period(), &evidence("a", 0), 102)
            .is_err()
    );
    s.bind_budget_resources("a", &binding("a"), 102).unwrap();
    let changes = s.conn.total_changes();
    s.bind_budget_resources("a", &binding("a"), 102).unwrap();
    assert_eq!(s.conn.total_changes(), changes);
    let mut changed = binding("a");
    changed.volumes = VolumeBinding::NoneAllocated;
    assert!(s.bind_budget_resources("a", &changed, 102).is_err());
    assert_eq!(
        s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
    assert!(
        s.reserve_budget(&policy(), &period(), &request("b"), 102)
            .is_err()
    );
}

#[test]
fn instance_hourly_total_hourly_and_lifetime_caps_are_all_in() {
    for kind in 0..4 {
        let (_, s) = fixture();
        let mut p = policy();
        p.max_instances = 3;
        match kind {
            0 => p.instance_hourly = 6357,
            1 => p.total_hourly = 6357,
            2 => p.monthly_all_in = 1765,
            _ => p.trial_all_in = 1765,
        }
        assert!(s.reserve_budget(&p, &period(), &request("a"), 100).is_err());
        assert!(s.budget_reservations().unwrap().is_empty());
    }
    let (_, s) = fixture();
    let mut p = policy();
    p.max_instances = 3;
    p.total_hourly = 12_715;
    s.reserve_budget(&p, &period(), &request("a"), 100).unwrap();
    assert!(s.reserve_budget(&p, &period(), &request("b"), 100).is_err());
}
#[test]
fn uncertainty_or_missed_deadline_freezes_new_spend_even_with_headroom() {
    for uncertain in [false, true] {
        let (_, s) = fixture();
        let mut p = policy();
        p.max_instances = 10;
        p.total_hourly = 100_000;
        s.reserve_budget(&p, &period(), &request("a"), 100).unwrap();
        let now = if uncertain {
            s.mark_budget_uncertain("a", 101).unwrap();
            102
        } else {
            1160
        };
        let mut b = request("b");
        b.quote.observed_at = now;
        b.quote.valid_until = now + 2000;
        assert!(s.reserve_budget(&p, &period(), &b, now).is_err());
    }
}
#[test]
fn explicit_approval_window_and_nested_transaction_fail_before_admission() {
    let (_, s) = fixture();
    let mut p = policy();
    p.valid_from = 101;
    assert!(s.reserve_budget(&p, &period(), &request("a"), 100).is_err());
    p.valid_from = 0;
    p.valid_until = 1159; // runtime+cleanup would extend beyond approval
    assert!(s.reserve_budget(&p, &period(), &request("a"), 100).is_err());
    p.valid_until = 100;
    assert!(s.reserve_budget(&p, &period(), &request("a"), 100).is_err());
    s.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(
        s.reserve_budget(&policy(), &period(), &request("a"), 100)
            .is_err()
    );
    s.conn.execute_batch("ROLLBACK").unwrap();
    assert!(s.budget_reservations().unwrap().is_empty());
}
#[test]
fn binding_needs_complete_fresh_owned_exact_resource_scope() {
    let (_, s) = fixture();
    s.reserve_budget(&policy(), &period(), &request("a"), 100)
        .unwrap();
    for kind in 0..6 {
        let mut b = binding("a");
        match kind {
            0 => b.status = EvidenceStatus::Partial,
            1 => b.observed_at = 102,
            2 => b.owner = "foreign".into(),
            3 => b.node_uid = "other".into(),
            4 => b.volumes = VolumeBinding::Ids(vec![]),
            _ => b.volumes = VolumeBinding::Ids(vec!["duplicate".into(), "duplicate".into()]),
        }
        assert!(s.bind_budget_resources("a", &b, 101).is_err());
    }
    assert!(s.budget_reservations().unwrap()[0].binding.is_none());
}

#[test]
fn explicit_final_cost_after_rollover_is_charged_to_settlement_month() {
    let (_, s) = fixture();
    let mut caps = policy();
    caps.max_instances = 2;
    caps.monthly_all_in = 4000;
    s.reserve_budget(&caps, &period(), &request("a"), 100)
        .unwrap();
    s.bind_budget_resources("a", &binding("a"), 101).unwrap();
    let next = CalendarPeriod {
        id: "synthetic-month-2".into(),
        starts_at: period().ends_at,
        ends_at: period().ends_at + 28 * 86400,
    };
    let mut e = evidence("a", 2500);
    e.observed_at = next.starts_at;
    s.settle_budget("a", &next, &e, next.starts_at).unwrap();
    let mut r = request("b");
    r.quote.observed_at = next.starts_at;
    r.quote.valid_until = next.starts_at + 2000;
    assert!(s.reserve_budget(&caps, &next, &r, next.starts_at).is_err());
}
#[test]
fn period_identity_cannot_be_reused_with_new_bounds_and_settlement_cannot_be_future() {
    let (_, s) = fixture();
    s.reserve_budget(&policy(), &period(), &request("a"), 100)
        .unwrap();
    s.bind_budget_resources("a", &binding("a"), 101).unwrap();
    let next = CalendarPeriod {
        id: "synthetic-month-2".into(),
        starts_at: period().ends_at,
        ends_at: period().ends_at + 28 * 86400,
    };
    assert!(s.settle_budget("a", &next, &evidence("a", 0), 102).is_err());
    s.settle_budget("a", &period(), &evidence("a", 0), 102)
        .unwrap();
    let mut r = request("b");
    r.quote.observed_at = next.starts_at;
    r.quote.valid_until = next.starts_at + 2000;
    s.reserve_budget(&policy(), &next, &r, next.starts_at)
        .unwrap();
    let mut b = binding("b");
    b.observed_at = next.starts_at;
    s.bind_budget_resources("b", &b, next.starts_at).unwrap();
    let mut e = evidence("b", 0);
    e.binding = b;
    e.observed_at = next.starts_at;
    s.settle_budget("b", &next, &e, next.starts_at).unwrap();
    let reused = CalendarPeriod {
        id: period().id,
        starts_at: next.ends_at,
        ends_at: next.ends_at + 28 * 86400,
    };
    let mut r = request("c");
    r.quote.observed_at = reused.starts_at;
    r.quote.valid_until = reused.starts_at + 2000;
    assert!(
        s.reserve_budget(&policy(), &reused, &r, reused.starts_at)
            .is_err()
    );
}
#[test]
fn settlement_race_restart_never_double_debits_or_changes_final_cost() {
    let (p, s) = fixture();
    s.reserve_budget(&policy(), &period(), &request("a"), 100)
        .unwrap();
    s.bind_budget_resources("a", &binding("a"), 101).unwrap();
    drop(s);
    let gate = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [1000, 1100]
        .into_iter()
        .map(|actual| {
            let p = p.clone();
            let gate = gate.clone();
            std::thread::spawn(move || {
                let s = Store::open(p, "owner").unwrap();
                gate.wait();
                s.settle_budget("a", &period(), &evidence("a", actual), 102)
                    .is_ok()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| u32::from(t.join().unwrap()))
            .sum::<u32>(),
        1
    );
    let s = Store::open(p, "owner").unwrap();
    let row = s.budget_reservations().unwrap().remove(0);
    let changes = s.conn.total_changes();
    assert_eq!(
        s.settle_budget("a", &period(), row.settlement.as_ref().unwrap(), 102)
            .unwrap(),
        row
    );
    assert_eq!(s.conn.total_changes(), changes);
}
#[test]
fn mock_node_cleanup_cannot_delete_or_reset_budget_liability() {
    let (_, mut s) = fixture();
    let mut mock = m8s_provider::Mock::default();
    s.scale("tier", 1).unwrap();
    s.reconcile(&mut mock, 0).unwrap();
    let mut r = request("a");
    r.node_uid = s.nodes().unwrap().remove(0).uid;
    s.reserve_budget(&policy(), &period(), &r, 100).unwrap();
    s.down().unwrap();
    s.reconcile(&mut mock, 101).unwrap();
    s.reconcile(&mut mock, 102).unwrap();
    assert!(s.nodes().unwrap().is_empty());
    assert_eq!(s.budget_reservations().unwrap().len(), 1);
    s.scale("tier", 1).unwrap();
    assert!(
        s.reserve_budget(&policy(), &period(), &request("b"), 103)
            .is_err()
    );
}

#[test]
fn explicit_initial_baseline_persists_once_and_blocks_unknown_or_external_debt() {
    for liability in [
        ExternalLiability::Unknown,
        ExternalLiability::KnownReserved(500),
    ] {
        let (path, s) = fixture();
        let mut p = policy();
        p.baseline.external_liability = liability;
        s.initialize_budget(&p, &period(), 100).unwrap();
        assert!(s.reserve_budget(&p, &period(), &request("a"), 100).is_err());
        drop(s);
        let s = Store::open(path, "owner").unwrap();
        assert_eq!(s.budget_account().unwrap().unwrap().policy, p);
        assert!(s.initialize_budget(&policy(), &period(), 100).is_err());
        assert!(
            s.reserve_budget(&policy(), &period(), &request("a"), 100)
                .is_err()
        );
    }
    let (_, s) = fixture();
    let mut p = policy();
    p.baseline.prior_lifetime_spent = u64::MAX;
    s.initialize_budget(&p, &period(), 100).unwrap();
    assert!(s.reserve_budget(&p, &period(), &request("a"), 100).is_err());
    assert_eq!(
        s.budget_account()
            .unwrap()
            .unwrap()
            .policy
            .baseline
            .prior_lifetime_spent,
        u64::MAX
    );
}
#[test]
fn initial_spend_counts_exact_initial_period_and_lifetime_without_reset() {
    let (_, s) = fixture();
    let mut p = policy();
    p.baseline.prior_lifetime_spent = 500;
    p.baseline.prior_current_period_spent = 500;
    p.monthly_all_in = 2000;
    assert!(s.reserve_budget(&p, &period(), &request("a"), 100).is_err());
    let next = CalendarPeriod {
        id: "synthetic-month-2".into(),
        starts_at: period().ends_at,
        ends_at: period().ends_at + 28 * 86400,
    };
    let mut r = request("b");
    r.quote.observed_at = next.starts_at;
    r.quote.valid_until = next.starts_at + 2000;
    assert!(s.reserve_budget(&p, &next, &r, next.starts_at).is_ok());
    assert_eq!(
        s.budget_account()
            .unwrap()
            .unwrap()
            .policy
            .baseline
            .prior_lifetime_spent,
        500
    );
    let (_, s) = fixture();
    let mut p = policy();
    p.baseline.prior_lifetime_spent = 1000;
    p.trial_all_in = 2766;
    s.reserve_budget(&p, &period(), &request("a"), 100).unwrap();
    s.bind_budget_resources("a", &binding("a"), 101).unwrap();
    s.settle_budget("a", &period(), &evidence("a", 1000), 102)
        .unwrap();
    assert!(s.reserve_budget(&p, &period(), &request("b"), 103).is_err());
}
#[test]
fn baseline_must_be_explicit_complete_fresh_and_never_coerced_from_missing() {
    for kind in 0..5 {
        let (_, s) = fixture();
        let mut p = policy();
        match kind {
            0 => p.baseline.status = EvidenceStatus::Partial,
            1 => p.baseline.status = EvidenceStatus::Error,
            2 => p.baseline.observed_at = 101,
            3 => p.baseline.observed_at = 69,
            _ => p.baseline.external_liability = ExternalLiability::KnownReserved(0),
        }
        assert!(s.initialize_budget(&p, &period(), 100).is_err());
        assert!(s.budget_account().unwrap().is_none());
    }
    let mut missing = serde_json::to_value(policy()).unwrap();
    missing.as_object_mut().unwrap().remove("baseline");
    assert!(serde_json::from_value::<AdminBudget>(missing).is_err());
}
#[test]
fn unknown_or_unsupported_normalized_cost_units_are_never_coerced() {
    for kind in 0..6 {
        let (_, s) = fixture();
        let mut r = request("a");
        match kind {
            0 => r.quote.compute_unit = RateUnit::Unknown,
            1 => r.quote.compute_unit = RateUnit::Unsupported,
            2 => r.quote.storage_unit = RateUnit::Unknown,
            3 => r.quote.storage_unit = RateUnit::Unsupported,
            4 => r.quote.totals_unit = TotalUnit::Unknown,
            _ => r.quote.totals_unit = TotalUnit::Unsupported,
        }
        assert!(s.reserve_budget(&policy(), &period(), &r, 100).is_err());
        assert!(s.budget_reservations().unwrap().is_empty());
    }
    let mut missing = serde_json::to_value(request("a").quote).unwrap();
    missing.as_object_mut().unwrap().remove("storage_unit");
    assert!(serde_json::from_value::<CostQuote>(missing).is_err());
}
#[test]
fn late_overrun_is_retained_and_charged_against_current_month_not_original() {
    let (_, s) = fixture();
    let mut caps = policy();
    caps.monthly_all_in = 5000;
    s.reserve_budget(&caps, &period(), &request("a"), 100)
        .unwrap();
    s.bind_budget_resources("a", &binding("a"), 101).unwrap();
    let next = CalendarPeriod {
        id: "synthetic-month-2".into(),
        starts_at: period().ends_at,
        ends_at: period().ends_at + 28 * 86400,
    };
    let mut e = evidence("a", 5001);
    e.observed_at = next.starts_at;
    let row = s.settle_budget("a", &next, &e, next.starts_at).unwrap();
    assert_eq!(row.state, BudgetState::Settled);
    assert_eq!(row.billing_period, Some(next.clone()));
    assert_eq!(row.settlement.unwrap().billing, e.billing);
    let mut r = request("b");
    r.quote.observed_at = next.starts_at;
    r.quote.valid_until = next.starts_at + 2000;
    assert!(s.reserve_budget(&caps, &next, &r, next.starts_at).is_err());
}

#[test]
fn normalized_cost_bound_must_cover_cleanup_deadline_including_equality() {
    let (_, s) = fixture();
    let mut r = request("a");
    r.quote.valid_until = 1160;
    assert!(s.reserve_budget(&policy(), &period(), &r, 100).is_ok());
    for until in [101, 1159] {
        let (_, s) = fixture();
        let mut r = request("a");
        r.quote.valid_until = until;
        assert!(s.reserve_budget(&policy(), &period(), &r, 100).is_err());
        assert!(s.budget_reservations().unwrap().is_empty());
    }
    let mut q = request("a").quote;
    q.observed_at = i64::MAX - 10;
    q.valid_until = i64::MAX;
    assert!(costs(&q, &policy(), i64::MAX - 10).is_err());
    let mut p = policy();
    p.max_runtime_seconds = u64::MAX;
    q = request("a").quote;
    q.valid_until = i64::MAX;
    q.runtime_seconds = i64::MAX as u64 + 1;
    assert!(costs(&q, &p, 100).is_err());
    q.runtime_seconds = u64::MAX;
    assert!(costs(&q, &p, 100).is_err());
}
#[test]
fn baseline_requires_exact_initial_period_and_consistent_explicit_spend() {
    for kind in 0..3 {
        let (_, s) = fixture();
        let mut p = policy();
        match kind {
            0 => p.baseline.initial_period.id = "different-period".into(),
            1 => p.baseline.prior_current_period_spent = 1,
            _ => p.baseline.status = EvidenceStatus::Unknown,
        }
        assert!(s.initialize_budget(&p, &period(), 100).is_err());
        assert!(s.budget_account().unwrap().is_none());
    }
    for field in [
        "prior_lifetime_spent",
        "prior_current_period_spent",
        "initial_period",
    ] {
        let mut missing = serde_json::to_value(policy()).unwrap();
        missing["baseline"].as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<AdminBudget>(missing).is_err());
    }
    let (_, s) = fixture();
    s.initialize_budget(&policy(), &period(), 100).unwrap();
    let mut p = policy();
    p.baseline.prior_lifetime_spent = 1;
    assert!(s.initialize_budget(&p, &period(), 100).is_err());
}
#[test]
fn distinct_initial_monthly_spend_and_lifetime_spend_survive_rollover() {
    let (path, s) = fixture();
    let mut p = policy();
    p.baseline.prior_lifetime_spent = 5000;
    p.baseline.prior_current_period_spent = 100;
    p.trial_all_in = 9000;
    p.monthly_all_in = 2000;
    // Monthly includes100, NOT the entire5000 lifetime baseline.
    s.reserve_budget(&p, &period(), &request("a"), 100).unwrap();
    s.bind_budget_resources("a", &binding("a"), 101).unwrap();
    s.settle_budget("a", &period(), &evidence("a", 3000), 102)
        .unwrap();
    drop(s);
    let s = Store::open(path, "owner").unwrap();
    let next = CalendarPeriod {
        id: "synthetic-month-2".into(),
        starts_at: period().ends_at,
        ends_at: period().ends_at + 28 * 86400,
    };
    let mut r = request("b");
    r.quote.observed_at = next.starts_at;
    r.quote.valid_until = next.starts_at + 2000;
    // New monthly cost fits, but retained lifetime5000+actual3000+new1766 does not.
    assert!(s.reserve_budget(&p, &next, &r, next.starts_at).is_err());
}
