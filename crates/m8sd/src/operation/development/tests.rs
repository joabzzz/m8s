//! Actual private SQLite + sealed virtual effects; no provider/process/network.
use super::*;
use crate::budget::*;
use std::{
    path::PathBuf,
    sync::{
        Barrier,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn path() -> PathBuf {
    use std::os::unix::fs::DirBuilderExt;
    let p = std::env::temp_dir().join(format!(
        "m8sd-development-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
    p.join("state.db")
}
fn period() -> CalendarPeriod {
    CalendarPeriod {
        id: "month".into(),
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
            provenance: "synthetic-bounded-costs".into(),
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
fn plan() -> SyntheticPlan {
    SyntheticPlan::new("namespace", "target", "tier").unwrap()
}
fn executor() -> DevelopmentExecutor {
    DevelopmentExecutor::new_synthetic(plan()).unwrap()
}
fn intent(id: &str) -> CreateIntent {
    let mut i = CreateIntent {
        owner: "owner".into(),
        run_id: "run".into(),
        request_id: id.into(),
        node_uid: format!("node-{id}"),
        operation_id: format!("op-{id}"),
        launch_fingerprint: [0; 32],
    };
    i.launch_fingerprint = plan().fingerprint(&i).unwrap();
    i
}
fn seed(s: &Store, id: &str, p: &AdminBudget) {
    s.scale("tier", 1).unwrap();
    s.conn
        .execute(
            "INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES (?,'tier','\"Launching\"',100)",
            [format!("node-{id}")],
        )
        .unwrap();
    s.reserve_budget(p, &period(), &request(id), 100).unwrap();
}
fn fixture() -> (PathBuf, Store, DevelopmentExecutor) {
    let path = path();
    let s = Store::open(&path, "owner").unwrap();
    seed(&s, "a", &policy());
    (path, s, executor())
}
fn claim(s: &Store, e: &DevelopmentExecutor) -> CreateClaim {
    e.prepare(s, &intent("a"), 100).unwrap();
    e.claim(s, "op-a", 101).unwrap()
}
fn pause_pair() -> (Pause, mpsc::Receiver<u64>, mpsc::SyncSender<()>) {
    let (entered, seen) = mpsc::sync_channel(1);
    let (resume, wait) = mpsc::sync_channel(1);
    (
        Pause {
            entered,
            resume: wait,
        },
        seen,
        resume,
    )
}
#[test]
fn exact_canonical_plan_and_bound_claim_invokes_once_with_anchored_virtual_cleanup() {
    let (_, s, mut e) = fixture();
    let c = claim(&s, &e);
    let r = match e.execute(&s, c, 102).unwrap() {
        DevelopmentOutcome::Bound(r) => r,
        _ => panic!("expected synthetic bound result"),
    };
    assert_eq!(e.invocations(), 1);
    assert_eq!(r.invocation_started_at, Some(102));
    assert!(r.cleanup_required);
    let fake = &e.resources()[0];
    assert_eq!(r.result.as_ref(), Some(fake.binding()));
    assert!(fake.service_eligible(1099));
    assert!(!fake.service_eligible(1100));
    assert_eq!(fake.cleanup_deadline(), 1160);
    e.advance(1100).unwrap();
    assert!(!e.resources()[0].is_removed());
    e.advance(1160).unwrap();
    assert!(e.resources()[0].is_removed());
    assert!(e.advance(1159).is_err());
    assert_eq!(
        s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
    assert!(e.claim(&s, "op-a", 103).is_err());
}
#[test]
fn canonical_hash_is_versioned_length_prefixed_and_binds_every_identity_and_plan_field() {
    let i = intent("a");
    let expected = plan().fingerprint(&i).unwrap();
    let mut h = Sha256::new();
    h.update(b"m8s-development-plan\0");
    h.update(1u16.to_be_bytes());
    for value in [
        "SyntheticV1",
        "namespace",
        "target",
        "tier",
        "owner",
        "run",
        "a",
        "node-a",
        "op-a",
    ] {
        h.update((value.len() as u32).to_be_bytes());
        h.update(value.as_bytes());
    }
    assert_eq!(expected, <[u8; 32]>::from(h.finalize()));
    for index in 0..5 {
        let mut changed = i.clone();
        match index {
            0 => changed.owner.push('x'),
            1 => changed.run_id.push('x'),
            2 => changed.request_id.push('x'),
            3 => changed.node_uid.push('x'),
            _ => changed.operation_id.push('x'),
        };
        assert_ne!(expected, plan().fingerprint(&changed).unwrap());
    }
    for p in [
        SyntheticPlan::new("different", "target", "tier").unwrap(),
        SyntheticPlan::new("namespace", "different", "tier").unwrap(),
        SyntheticPlan::new("namespace", "target", "different").unwrap(),
    ] {
        assert_ne!(expected, p.fingerprint(&i).unwrap());
    }
    assert_ne!(
        SyntheticPlan::new("ab", "c", "tier")
            .unwrap()
            .fingerprint(&i)
            .unwrap(),
        SyntheticPlan::new("a", "bc", "tier")
            .unwrap()
            .fingerprint(&i)
            .unwrap()
    );
    for text in ["".to_owned(), "x".repeat(65), "bad\nlabel".into()] {
        assert!(SyntheticPlan::new(&text, "target", "tier").is_err());
    }
}
#[test]
fn changed_plan_fingerprint_or_live_tier_never_dispatches() {
    for case in 0..4 {
        let (_, s, e) = fixture();
        let mut i = intent("a");
        if case == 0 {
            i.launch_fingerprint[0] ^= 1;
            assert!(e.prepare(&s, &i, 100).is_err());
            continue;
        }
        e.prepare(&s, &i, 100).unwrap();
        let other = match case {
            1 => DevelopmentExecutor::new_synthetic(
                SyntheticPlan::new("changed", "target", "tier").unwrap(),
            )
            .unwrap(),
            2 => DevelopmentExecutor::new_synthetic(
                SyntheticPlan::new("namespace", "changed", "tier").unwrap(),
            )
            .unwrap(),
            _ => DevelopmentExecutor::new_synthetic(
                SyntheticPlan::new("namespace", "target", "changed").unwrap(),
            )
            .unwrap(),
        };
        assert!(other.claim(&s, "op-a", 101).is_err());
        assert_eq!(other.invocations(), 0);
    }
    let (_, s, e) = fixture();
    e.prepare(&s, &intent("a"), 100).unwrap();
    s.conn
        .execute("UPDATE gpu_node SET tier='other'", [])
        .unwrap();
    assert!(e.claim(&s, "op-a", 101).is_err());
}
#[test]
fn exact_issuing_store_allocation_not_owner_strings_path_or_reopened_database() {
    for case in 0..3 {
        let (path, s, mut e) = fixture();
        let c = claim(&s, &e);
        let other = if case == 0 {
            let (_, other, _) = fixture();
            other
        } else {
            Store::open(&path, "owner").unwrap()
        };
        if case == 2 {
            drop(s);
            assert!(e.execute(&other, c, 102).is_err());
        } else {
            assert!(e.execute(&other, c, 102).is_err());
            assert!(e.claim(&s, "op-a", 102).is_err());
        }
        assert_eq!(e.invocations(), 0);
    }
}
#[test]
fn identical_executor_plan_is_not_actual_issuer_and_ordinary_claim_is_unbound() {
    let (_, s, e) = fixture();
    let c = claim(&s, &e);
    let mut other = executor();
    assert!(other.execute(&s, c, 102).is_err());
    assert_eq!(other.invocations(), 0);
    assert!(e.claim(&s, "op-a", 103).is_err());
    let (_, s, mut e) = fixture();
    e.prepare(&s, &intent("a"), 100).unwrap();
    let c = s.claim_create("op-a", 101).unwrap();
    assert!(e.execute(&s, c, 102).is_err());
    assert_eq!(e.invocations(), 0);
}
#[test]
fn prepared_metadata_can_be_claimed_after_reopen_but_consumed_metadata_never_remints() {
    let (path, s, e) = fixture();
    e.prepare(&s, &intent("a"), 100).unwrap();
    drop(s);
    drop(e);
    let s = Store::open(&path, "owner").unwrap();
    let mut e = executor();
    let c = e.claim(&s, "op-a", 101).unwrap();
    assert!(matches!(
        e.execute(&s, c, 102).unwrap(),
        DevelopmentOutcome::Bound(_)
    ));
    drop(e);
    drop(s);
    let s = Store::open(path, "owner").unwrap();
    assert!(executor().claim(&s, "op-a", 103).is_err());
}
#[test]
fn own_exact_uncertain_hold_is_permitted_charged_once_and_not_omitted() {
    for too_small in [false, true] {
        let path = path();
        let s = Store::open(path, "owner").unwrap();
        let mut p = policy();
        p.trial_all_in = 1766;
        p.monthly_all_in = 1766;
        p.total_hourly = 6358;
        p.instance_hourly = 6358;
        p.max_instances = 1;
        seed(&s, "a", &p);
        let mut e = executor();
        let c = claim(&s, &e);
        if too_small {
            let mut a = s.budget_account().unwrap().unwrap();
            a.policy.trial_all_in = 1765;
            s.conn
                .execute("UPDATE budget_account SET record=?", [encode(&a).unwrap()])
                .unwrap();
            assert!(e.execute(&s, c, 102).is_err());
            assert_eq!(e.invocations(), 0);
        } else {
            assert!(matches!(
                e.execute(&s, c, 102).unwrap(),
                DevelopmentOutcome::Bound(_)
            ));
            assert_eq!(e.invocations(), 1);
        }
        assert_eq!(s.budget_reservations().unwrap()[0].reserved_all_in, 1766);
        assert_eq!(
            s.budget_reservations().unwrap()[0].state,
            BudgetState::Uncertain
        );
    }
}
#[test]
fn other_uncertainty_unknown_bill_or_late_overrun_blocks_an_issued_claim() {
    for kind in 0..3 {
        let (_, s, mut e) = fixture();
        seed(&s, "b", &policy());
        let c = claim(&s, &e);
        if kind == 0 {
            s.mark_budget_uncertain("b", 102).unwrap();
        } else {
            let b = ResourceBinding {
                owner: "owner".into(),
                run_id: "run".into(),
                node_uid: "node-b".into(),
                instance_id: "synthetic-other".into(),
                volumes: VolumeBinding::NoneAllocated,
                observed_at: 102,
                status: EvidenceStatus::Complete,
                provenance: "synthetic-other-binding".into(),
            };
            s.bind_budget_resources("b", &b, 102).unwrap();
            let ev = SettlementEvidence {
                binding: b,
                observed_at: 102,
                status: EvidenceStatus::Complete,
                resource: ResourceOutcome::OwnedAbsent,
                volumes: VolumeOutcome::BoundVolumesAbsent,
                billing: if kind == 1 {
                    FinalBilling::Unknown
                } else {
                    FinalBilling::Known {
                        actual_all_in: 10000,
                        provenance: "synthetic-overrun".into(),
                    }
                },
                provenance: "synthetic-complete-absence".into(),
            };
            s.settle_budget("b", &period(), &ev, 102).unwrap();
        }
        assert!(e.execute(&s, c, 102).is_err());
        assert_eq!(e.invocations(), 0);
        assert!(
            s.create_operations().unwrap()[0]
                .invocation_started_at
                .is_none()
        );
    }
}
#[test]
fn missing_changed_or_corrupt_exact_budget_refuses_before_fake_call() {
    for kind in 0..4 {
        let (_, s, mut e) = fixture();
        let c = claim(&s, &e);
        match kind {
            0 => {
                s.conn
                    .execute("DELETE FROM budget_reservation", [])
                    .unwrap();
            }
            1 => {
                s.conn
                    .execute("UPDATE budget_reservation SET record='broken'", [])
                    .unwrap();
            }
            2 => {
                let mut b = s.budget_reservations().unwrap().remove(0);
                b.reserved_all_in -= 1;
                s.conn
                    .execute(
                        "UPDATE budget_reservation SET record=?",
                        [encode(&b).unwrap()],
                    )
                    .unwrap();
            }
            _ => {
                s.conn.execute("DELETE FROM budget_account", []).unwrap();
            }
        }
        assert!(e.execute(&s, c, 102).is_err());
        assert_eq!(e.invocations(), 0);
    }
}
#[test]
fn marker_commits_before_backend_no_transaction_and_cancel_after_commit_is_possible_one() {
    let (path, s, mut e) = fixture();
    let c = claim(&s, &e);
    let (pause, seen, resume) = pause_pair();
    e.hooks.after_commit = Some(pause);
    let writer = Store::open(path, "owner").unwrap();
    let run = std::thread::spawn(move || {
        let outcome = e.execute(&s, c, 102);
        (s, e, outcome)
    });
    assert_eq!(
        seen.recv_timeout(std::time::Duration::from_secs(3))
            .unwrap(),
        0
    );
    let r = writer.create_operations().unwrap().remove(0);
    assert_eq!(r.invocation_started_at, Some(102));
    writer
        .conn
        .execute_batch("BEGIN IMMEDIATE;ROLLBACK;")
        .unwrap();
    writer.cancel_create("op-a", 102).unwrap();
    resume.send(()).unwrap();
    let (s, e, outcome) = run.join().unwrap();
    let r = match outcome.unwrap() {
        DevelopmentOutcome::Bound(r) => r,
        _ => panic!("expected cleanup-only late binding"),
    };
    assert_eq!(r.state, CreateState::Cancelled);
    assert!(r.cleanup_required);
    assert_eq!(e.invocations(), 1);
    assert_eq!(e.resources().len(), 1);
    assert_eq!(
        s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
}
#[test]
fn cancellation_before_claim_or_before_invocation_and_deleting_down_yields_zero() {
    for kind in 0..4 {
        let (_, s, mut e) = fixture();
        e.prepare(&s, &intent("a"), 100).unwrap();
        if kind == 0 {
            s.cancel_create("op-a", 101).unwrap();
            assert!(e.claim(&s, "op-a", 101).is_err());
            continue;
        }
        let c = e.claim(&s, "op-a", 101).unwrap();
        if kind == 1 {
            s.cancel_create("op-a", 102).unwrap();
            assert!(matches!(
                e.execute(&s, c, 102).unwrap(),
                DevelopmentOutcome::NotInvoked
            ));
        } else {
            if kind == 2 {
                s.down().unwrap();
                s.scale("tier", 1).unwrap();
            } else {
                s.conn.execute("DELETE FROM gpu_node", []).unwrap();
            }
            assert!(e.execute(&s, c, 102).is_err());
        }
        assert_eq!(e.invocations(), 0);
    }
}
#[test]
fn before_commit_failure_and_after_commit_unwind_burn_without_retry() {
    for after in [false, true] {
        let (_, s, mut e) = fixture();
        let c = claim(&s, &e);
        if after {
            e.hooks.after_commit_unwind = true;
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| e.execute(&s, c, 102)))
                    .is_err()
            );
        } else {
            e.hooks.before_commit_fail = true;
            assert!(e.execute(&s, c, 102).is_err());
        }
        assert_eq!(e.invocations(), 0);
        assert_eq!(
            s.create_operations().unwrap()[0]
                .invocation_started_at
                .is_some(),
            after
        );
        assert!(e.claim(&s, "op-a", 103).is_err());
        assert_eq!(
            s.budget_reservations().unwrap()[0].state,
            BudgetState::Uncertain
        );
    }
}
#[test]
fn lost_error_or_malformed_reply_retains_resource_when_present_and_never_retries() {
    for fault in [
        ResponseFault::LostResponse,
        ResponseFault::BackendError,
        ResponseFault::MalformedResponse,
    ] {
        let (_, s, mut e) = fixture();
        e.hooks.response = fault;
        let c = claim(&s, &e);
        assert!(matches!(
            e.execute(&s, c, 102).unwrap(),
            DevelopmentOutcome::Ambiguous
        ));
        assert_eq!(e.invocations(), 1);
        assert_eq!(
            e.resources().len(),
            usize::from(fault != ResponseFault::BackendError)
        );
        let r = s.create_operations().unwrap().remove(0);
        assert!(r.cleanup_required && r.uncertainty_observed && r.invocation_started_at.is_some());
        assert!(r.result.is_none());
        assert!(e.claim(&s, "op-a", 103).is_err());
    }
}
#[test]
fn result_write_error_after_effect_keeps_resource_and_durable_possible_effect_marker() {
    let (path, s, mut e) = fixture();
    let c = claim(&s, &e);
    let (pause, seen, resume) = pause_pair();
    e.hooks.after_effect = Some(pause);
    let writer = Store::open(path, "owner").unwrap();
    let run = std::thread::spawn(move || {
        let outcome = e.execute(&s, c, 102);
        (s, e, outcome)
    });
    assert_eq!(
        seen.recv_timeout(std::time::Duration::from_secs(3))
            .unwrap(),
        1
    );
    writer
        .conn
        .execute("UPDATE budget_account SET record='broken'", [])
        .unwrap();
    resume.send(()).unwrap();
    let (s, e, outcome) = run.join().unwrap();
    assert!(matches!(outcome.unwrap(), DevelopmentOutcome::Ambiguous));
    assert_eq!(e.resources().len(), 1);
    let r = s.create_operations().unwrap().remove(0);
    assert_eq!(r.invocation_started_at, Some(102));
    assert!(r.cleanup_required && r.result.is_none());
    assert!(executor().claim(&s, "op-a", 103).is_err());
}
#[test]
fn legacy_marker_defaults_none_but_unbound_or_consumed_rows_never_gain_capability() {
    let (_, s, mut e) = fixture();
    e.prepare(&s, &intent("a"), 100).unwrap();
    let c = s.claim_create("op-a", 101).unwrap();
    let json: String = s
        .conn
        .query_row("SELECT record FROM create_operation", [], |r| r.get(0))
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("invocation_started_at");
    s.conn
        .execute("UPDATE create_operation SET record=?", [value.to_string()])
        .unwrap();
    assert!(
        s.create_operations().unwrap()[0]
            .invocation_started_at
            .is_none()
    );
    assert!(e.execute(&s, c, 102).is_err());
    assert!(e.claim(&s, "op-a", 102).is_err());
}
#[test]
fn duplicate_or_inconsistent_marker_refuses_no_renewed_runtime_or_faked_result() {
    for marker in [100, 102, 9999] {
        let (_, s, mut e) = fixture();
        let c = claim(&s, &e);
        let mut r = s.create_operations().unwrap().remove(0);
        r.invocation_started_at = Some(marker);
        if marker == 102 {
            r.last_at = 102;
        }
        s.conn
            .execute(
                "UPDATE create_operation SET record=?",
                [encode(&r).unwrap()],
            )
            .unwrap();
        assert!(e.execute(&s, c, 102).is_err());
        assert_eq!(e.invocations(), 0);
    }
}
#[test]
fn competing_executor_claims_use_same_single_dispatch_transition() {
    let (path, s, _) = fixture();
    seed(&s, "b", &policy());
    let e = executor();
    e.prepare(&s, &intent("a"), 100).unwrap();
    e.prepare(&s, &intent("b"), 100).unwrap();
    let s1 = Store::open(&path, "owner").unwrap();
    let s2 = Store::open(&path, "owner").unwrap();
    let gate = Arc::new(Barrier::new(2));
    let runs: Vec<_> = [(s1, "op-a"), (s2, "op-b")]
        .into_iter()
        .map(|(s, id)| {
            let g = gate.clone();
            std::thread::spawn(move || {
                let mut e = executor();
                g.wait();
                match e.claim(&s, id, 101) {
                    Ok(c) => {
                        let outcome = e.execute(&s, c, 102);
                        (e.invocations(), outcome.is_ok())
                    }
                    Err(_) => (0, false),
                }
            })
        })
        .collect();
    let outcomes: Vec<_> = runs.into_iter().map(|r| r.join().unwrap()).collect();
    assert_eq!(outcomes.iter().map(|(n, _)| *n).sum::<u64>(), 1);
    assert_eq!(outcomes.iter().filter(|(_, ok)| *ok).count(), 1);
}

#[test]
fn original_serving_deadline_minus_one_succeeds_exact_deadline_or_rollback_does_not() {
    for now in [100, 109, 110] {
        let path = path();
        let s = Store::open(path, "owner").unwrap();
        s.scale("tier", 1).unwrap();
        s.conn.execute("INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES ('node-a','tier','\"Launching\"',100)",[]).unwrap();
        let mut p = policy();
        p.instance_hourly = 100000;
        p.total_hourly = 200000;
        let mut r = request("a");
        r.quote.runtime_seconds = 10;
        r.quote.transfer_total = 0;
        r.quote.valid_until = 170;
        s.reserve_budget(&p, &period(), &r, 100).unwrap();
        let mut e = executor();
        let c = claim(&s, &e);
        let outcome = e.execute(&s, c, now);
        if now == 109 {
            assert!(matches!(outcome.unwrap(), DevelopmentOutcome::Bound(_)));
            assert_eq!(e.resources()[0].serving_deadline(), 110);
            assert_eq!(e.resources()[0].cleanup_deadline(), 170);
            assert!(e.resources()[0].service_eligible(109));
            assert!(!e.resources()[0].service_eligible(110));
        } else {
            assert!(outcome.is_err());
            assert_eq!(e.invocations(), 0);
        }
    }
}
#[test]
fn driver_drop_claim_drop_or_rejected_execute_never_offers_new_authority() {
    let (_, s, e) = fixture();
    let c = claim(&s, &e);
    drop(e);
    let mut replacement = executor();
    assert!(replacement.execute(&s, c, 102).is_err());
    assert!(replacement.claim(&s, "op-a", 103).is_err());
    assert_eq!(replacement.invocations(), 0);
    let (_, s, e) = fixture();
    let c = claim(&s, &e);
    drop(c);
    assert!(e.claim(&s, "op-a", 102).is_err());
    assert!(
        s.create_operations().unwrap()[0]
            .invocation_started_at
            .is_none()
    );
    assert_eq!(
        s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
}
#[test]
fn cancellation_after_response_does_not_clear_marker_or_allow_rebind_retry() {
    let (_, s, mut e) = fixture();
    let c = claim(&s, &e);
    assert!(matches!(
        e.execute(&s, c, 102).unwrap(),
        DevelopmentOutcome::Bound(_)
    ));
    let r = s.cancel_create("op-a", 103).unwrap();
    assert!(r.cleanup_required);
    assert_eq!(r.invocation_started_at, Some(102));
    let mut wrong = e.resources()[0].binding().clone();
    wrong.volumes = VolumeBinding::NoneAllocated;
    assert!(
        s.record_create_result("op-a", &CreateObservation::Bound(wrong), 104)
            .is_err()
    );
    assert!(e.claim(&s, "op-a", 104).is_err());
    assert_eq!(e.invocations(), 1);
}
#[test]
fn invocation_write_failure_is_zero_calls_but_dispatch_stays_consumed() {
    let (_, s, mut e) = fixture();
    let c = claim(&s, &e);
    s.conn.execute_batch("CREATE TRIGGER synthetic_refuse_invocation BEFORE UPDATE ON create_operation BEGIN SELECT RAISE(ABORT,'synthetic storage fault'); END;").unwrap();
    assert!(e.execute(&s, c, 102).is_err());
    assert_eq!(e.invocations(), 0);
    assert!(
        s.create_operations().unwrap()[0]
            .invocation_started_at
            .is_none()
    );
    s.conn
        .execute_batch("DROP TRIGGER synthetic_refuse_invocation;")
        .unwrap();
    assert!(e.claim(&s, "op-a", 103).is_err());
    assert_eq!(
        s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
}
#[test]
fn stale_quote_approval_period_change_and_mismatched_or_overflowing_bounds_refuse() {
    for case in 0..4 {
        let (_, s, mut e) = fixture();
        let mut c = claim(&s, &e);
        match case {
            0 => {}
            1 => {
                let mut a = s.budget_account().unwrap().unwrap();
                a.policy.valid_until = 102;
                s.conn
                    .execute("UPDATE budget_account SET record=?", [encode(&a).unwrap()])
                    .unwrap();
            }
            2 => {
                let mut a = s.budget_account().unwrap().unwrap();
                a.period.starts_at = a.period.ends_at;
                a.period.ends_at += 28 * 86400;
                s.conn
                    .execute("UPDATE budget_account SET record=?", [encode(&a).unwrap()])
                    .unwrap();
            }
            _ => c.cleanup_deadline = i64::MAX,
        }
        let now = if case == 0 { 131 } else { 102 };
        assert!(e.execute(&s, c, now).is_err());
        assert_eq!(e.invocations(), 0);
    }
    let (_, s, mut e) = fixture();
    let c = claim(&s, &e);
    let mut r = s.create_operations().unwrap().remove(0);
    r.budget.request.quote.runtime_seconds = u64::MAX;
    let mut b = r.budget.clone();
    b.state = BudgetState::Uncertain;
    s.conn
        .execute(
            "UPDATE create_operation SET record=?",
            [encode(&r).unwrap()],
        )
        .unwrap();
    s.conn
        .execute(
            "UPDATE budget_reservation SET record=?",
            [encode(&b).unwrap()],
        )
        .unwrap();
    assert!(e.execute(&s, c, 102).is_err());
    assert_eq!(e.invocations(), 0);
}
#[test]
fn another_expired_reserved_cleanup_blocks_own_uncertain_exception() {
    let path = path();
    let s = Store::open(path, "owner").unwrap();
    let mut p = policy();
    p.minimum_cleanup_seconds = 1;
    p.instance_hourly = 1000000;
    p.total_hourly = 2000000;
    seed(&s, "a", &p);
    s.conn.execute("INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES ('node-b','tier','\"Launching\"',100)",[]).unwrap();
    let mut b = request("b");
    b.quote.runtime_seconds = 1;
    b.quote.cleanup_seconds = 1;
    b.quote.transfer_total = 0;
    s.reserve_budget(&p, &period(), &b, 100).unwrap();
    let mut e = executor();
    let c = claim(&s, &e);
    assert!(e.execute(&s, c, 103).is_err());
    assert_eq!(e.invocations(), 0);
}

#[test]
fn duplicate_payload_cannot_turn_a_second_uncertain_sql_row_into_own_exception() {
    let (_, s, mut e) = fixture();
    let c = claim(&s, &e);
    let b = s.budget_reservations().unwrap().remove(0);
    s.conn.execute("INSERT INTO budget_reservation(owner,request_id,node_uid,record)VALUES('owner','other','other-node',?)",[encode(&b).unwrap()]).unwrap();
    assert!(e.execute(&s, c, 102).is_err());
    assert_eq!(e.invocations(), 0);
}

#[test]
fn preexisting_same_operation_ambiguity_is_not_the_fresh_consumed_hold_exception() {
    let (_, s, mut e) = fixture();
    let c = claim(&s, &e);
    s.record_create_result("op-a", &CreateObservation::Ambiguous, 102)
        .unwrap();
    assert!(e.execute(&s, c, 103).is_err());
    assert_eq!(e.invocations(), 0);
    assert!(s.create_operations().unwrap()[0].uncertainty_observed);
}
