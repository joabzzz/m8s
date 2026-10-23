//! Public managed LOCAL metadata qualification; synthetic accounting is NOT approval.
//! No SQL fixtures, child execution, fabricated claims, absence or settlement.
#![cfg(all(unix, feature = "local-development"))]
use m8s_provider::local::DevelopmentCommand;
use m8sd::{
    Store,
    budget::*,
    operation::{
        CreateIntent, CreateState,
        local_development::{LocalDevelopmentExecutor, LocalDevelopmentPlan, LocalLaunchRequest},
    },
};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
const OWNER: &str = "synthetic-owner";
static NEXT: AtomicU64 = AtomicU64::new(0);
fn utc() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}
fn request(id: &str) -> LocalLaunchRequest {
    LocalLaunchRequest {
        run_id: "synthetic-run".into(),
        request_id: format!("request-{id}"),
        operation_id: format!("operation-{id}"),
    }
}
struct Fixture {
    dir: PathBuf,
    db: PathBuf,
    at: i64,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "m8sd-managed-public-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        Self {
            db: dir.join("state.db"),
            dir,
            at: utc(),
        }
    }
    fn store(&self) -> Store {
        Store::open(&self.db, OWNER).unwrap()
    }
    fn executor(&self, id: &str, target: &str) -> LocalDevelopmentExecutor {
        let journal = self.dir.join(id);
        if !journal.exists() {
            fs::DirBuilder::new().mode(0o700).create(&journal).unwrap();
        }
        LocalDevelopmentExecutor::open(
            LocalDevelopmentPlan::new(
                DevelopmentCommand::new("/bin/sleep".into(), vec!["60".into()]).unwrap(),
                journal,
                "namespace",
                target,
                "tier",
            )
            .unwrap(),
            OWNER,
        )
        .unwrap()
    }
    fn period(&self) -> CalendarPeriod {
        CalendarPeriod {
            id: "trusted-synthetic-month".into(),
            starts_at: self.at - 86400,
            ends_at: self.at + 28 * 86400,
        }
    }
    fn policy(&self) -> AdminBudget {
        AdminBudget {
            owner: OWNER.into(),
            approval_id: "synthetic-not-real-approval".into(),
            baseline: AccountBaseline {
                prior_lifetime_spent: 0,
                prior_current_period_spent: 0,
                initial_period: self.period(),
                external_liability: ExternalLiability::NoneConfirmed,
                observed_at: self.at,
                status: EvidenceStatus::Complete,
                provenance: "synthetic-explicit-baseline".into(),
            },
            valid_from: self.at - 100,
            valid_until: self.at + 600,
            trial_all_in: 100000,
            monthly_all_in: 100000,
            total_hourly: 10_000_000,
            instance_hourly: 10_000_000,
            max_instances: 8,
            max_runtime_seconds: 300,
            minimum_cleanup_seconds: 3,
            minimum_contingency: 1,
        }
    }
    fn request(&self, id: &str) -> BudgetRequest {
        BudgetRequest {
            owner: OWNER.into(),
            run_id: "synthetic-run".into(),
            request_id: format!("request-{id}"),
            node_uid: format!("node-{id}"),
            quote: CostQuote {
                provenance: "synthetic-normalized-cost-bound-not-billed".into(),
                observed_at: self.at,
                valid_until: self.at + 600,
                status: EvidenceStatus::Complete,
                compute_unit: RateUnit::MicroUsdPerHour,
                storage_unit: RateUnit::MicroUsdPerHour,
                totals_unit: TotalUnit::MicroUsd,
                compute_hourly: 3600,
                storage_hourly: 360,
                transfer_total: 10,
                runtime_seconds: 180,
                cleanup_seconds: 3,
                contingency: 1,
            },
        }
    }
    fn reserve(
        &self,
        s: &Store,
        e: &LocalDevelopmentExecutor,
        r: &LocalLaunchRequest,
    ) -> CreateIntent {
        let intent = e.planned_intent(s, r).unwrap();
        let mut b = self.request("placeholder");
        b.run_id = r.run_id.clone();
        b.request_id = r.request_id.clone();
        b.node_uid = intent.node_uid.clone();
        s.reserve_budget(&self.policy(), &self.period(), &b, utc())
            .unwrap();
        intent
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !thread::panicking() {
            fs::remove_dir_all(&self.dir).unwrap();
        }
    }
}
// Debug is used only for exact snapshot equality, never parsed for variant/state.
fn snapshot(s: &Store) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{}",
        s.nodes().unwrap(),
        s.create_operations().unwrap(),
        s.budget_reservations().unwrap(),
        s.budget_account().unwrap(),
        s.launch_count("tier", utc()).unwrap()
    )
}
fn no_submission(e: &LocalDevelopmentExecutor) {
    assert!(!e.submission_started());
    assert!(e.launch().is_none());
}

#[test]
fn preview_is_stable_effect_free_and_rotation_changes_fingerprint_not_uid() {
    let f = Fixture::new();
    let s = f.store();
    let e = f.executor("journal", "target");
    s.initialize_budget(&f.policy(), &f.period(), f.at).unwrap();
    let r = request("a");
    let before = snapshot(&s);
    let a = e.planned_intent(&s, &r).unwrap();
    assert_eq!(a, e.planned_intent(&s, &r).unwrap());
    let mut rotated = r.clone();
    rotated.operation_id = "different-operation".into();
    let b = e.planned_intent(&s, &rotated).unwrap();
    assert_eq!(a.node_uid, b.node_uid);
    assert_ne!(a.launch_fingerprint, b.launch_fingerprint);
    let e2 = f.executor("other-journal", "other-target");
    let c = e2.planned_intent(&s, &r).unwrap();
    assert_eq!(a.node_uid, c.node_uid);
    assert_ne!(a.launch_fingerprint, c.launch_fingerprint);
    let mut left = request("unused");
    left.run_id = "a".into();
    left.request_id = "bc".into();
    let mut right = left.clone();
    right.run_id = "ab".into();
    right.request_id = "c".into();
    assert_ne!(
        e.planned_intent(&s, &left).unwrap().node_uid,
        e.planned_intent(&s, &right).unwrap().node_uid
    );
    assert_eq!(before, snapshot(&s));
    no_submission(&e);
    no_submission(&e2);
}

#[test]
fn public_prepare_persists_one_intent_without_dispatch_and_rejects_rotation() {
    let f = Fixture::new();
    let s = f.store();
    s.scale("tier", 1).unwrap();
    let e = f.executor("journal", "target");
    let r = request("a");
    let intent = f.reserve(&s, &e, &r);
    let held = s.budget_reservations().unwrap();
    let op = e.prepare_managed(&s, &r).unwrap();
    assert_eq!(op.intent, intent);
    assert_eq!(op.state, CreateState::Prepared);
    assert!(op.dispatched_at.is_none());
    assert!(op.invocation_started_at.is_none());
    assert!(op.result.is_none());
    let nodes = s.nodes().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].uid, intent.node_uid);
    assert_eq!(nodes[0].tier, "tier");
    assert_eq!(nodes[0].phase, m8s_core::Phase::Launching);
    assert_eq!(s.launch_count("tier", utc()).unwrap(), 1);
    assert_eq!(s.create_operations().unwrap().len(), 1);
    assert_eq!(s.budget_reservations().unwrap(), held);
    let before = snapshot(&s);
    assert_eq!(e.prepare_managed(&s, &r).unwrap(), op);
    let mut rotated = r.clone();
    rotated.operation_id = "other-op".into();
    assert!(e.prepare_managed(&s, &rotated).is_err());
    let other = f.executor("other-journal", "other-target");
    assert!(other.prepare_managed(&s, &r).is_err());
    assert_eq!(before, snapshot(&s));
    no_submission(&e);
    no_submission(&other);
}

#[test]
fn cancelled_or_consumed_exact_retry_and_reopen_never_remint_or_renew_history() {
    for cancel in [true, false] {
        let f = Fixture::new();
        let s = f.store();
        s.scale("tier", 1).unwrap();
        let e = f.executor("journal", "target");
        let r = request("a");
        f.reserve(&s, &e, &r);
        e.prepare_managed(&s, &r).unwrap();
        if cancel {
            s.cancel_create(&r.operation_id, utc()).unwrap();
        } else {
            drop(e.claim(&s, &r.operation_id, utc()).unwrap());
        }
        let terminal = s.create_operations().unwrap().remove(0);
        assert_eq!(
            terminal.state,
            if cancel {
                CreateState::Cancelled
            } else {
                CreateState::DispatchConsumed
            }
        );
        let hold = s.budget_reservations().unwrap().remove(0);
        assert_eq!(hold.state, BudgetState::Uncertain);
        assert!(hold.reserved_all_in > 0);
        assert!(hold.settlement.is_none());
        let before = snapshot(&s);
        e.stop_handle().request_stop();
        assert_eq!(e.prepare_managed(&s, &r).unwrap(), terminal);
        assert_eq!(snapshot(&s), before);
        no_submission(&e);
        drop(e);
        drop(s);
        let s = f.store();
        let e = f.executor("journal", "target");
        assert_eq!(e.prepare_managed(&s, &r).unwrap(), terminal);
        assert!(e.claim(&s, &r.operation_id, utc()).is_err());
        assert_eq!(snapshot(&s), before);
        no_submission(&e);
    }
}

#[test]
fn two_sqlite_connections_same_request_or_competing_capacity_commit_only_one_history() {
    // Distinct journals are genuinely distinct fingerprints. Same-request race
    // must have one refusal, NOT two successful identical-plan recoveries.
    for same in [true, false] {
        let f = Fixture::new();
        let s = f.store();
        s.scale("tier", 1).unwrap();
        let e = f.executor("journal", "target");
        let e2 = f.executor("journal2", "target");
        let r = request("a");
        let r2 = if same { r.clone() } else { request("b") };
        f.reserve(&s, &e, &r);
        if !same {
            f.reserve(&s, &e2, &r2);
        }
        let s2 = f.store();
        let barrier = Arc::new(Barrier::new(2));
        let b = barrier.clone();
        let a = thread::spawn(move || {
            b.wait();
            let result = e.prepare_managed(&s, &r);
            no_submission(&e);
            result.is_ok()
        });
        let b = thread::spawn(move || {
            barrier.wait();
            let result = e2.prepare_managed(&s2, &r2);
            no_submission(&e2);
            result.is_ok()
        });
        assert_eq!(u8::from(a.join().unwrap()) + u8::from(b.join().unwrap()), 1);
        let s = f.store();
        assert_eq!(s.nodes().unwrap().len(), 1);
        assert_eq!(s.create_operations().unwrap().len(), 1);
        assert_eq!(s.launch_count("tier", utc()).unwrap(), 1);
        let holds = s.budget_reservations().unwrap();
        assert_eq!(holds.len(), if same { 1 } else { 2 });
        assert!(
            holds
                .iter()
                .all(|h| h.state == BudgetState::Reserved && h.settlement.is_none())
        );
    }
}

#[test]
fn missing_mismatched_or_other_unknown_budget_refuses_without_manager_writes() {
    for variant in 0..4 {
        let f = Fixture::new();
        let s = f.store();
        s.scale("tier", 2).unwrap();
        let e = f.executor("journal", "target");
        let r = request("a");
        s.initialize_budget(&f.policy(), &f.period(), f.at).unwrap();
        match variant {
            0 => {} // Missing reservation.
            1 => {
                let mut b = f.request("a");
                b.node_uid = "wrong-node".into();
                s.reserve_budget(&f.policy(), &f.period(), &b, utc())
                    .unwrap();
            }
            2 => {
                f.reserve(&s, &e, &r);
                s.mark_budget_uncertain(&r.request_id, utc()).unwrap();
            }
            3 => {
                let other = request("other");
                f.reserve(&s, &e, &other);
                f.reserve(&s, &e, &r);
                s.mark_budget_uncertain(&other.request_id, utc()).unwrap();
            }
            _ => unreachable!(),
        }
        let before = snapshot(&s);
        assert!(e.prepare_managed(&s, &r).is_err());
        assert_eq!(before, snapshot(&s));
        assert!(s.nodes().unwrap().is_empty());
        assert!(s.create_operations().unwrap().is_empty());
        no_submission(&e);
    }
}

#[test]
fn down_then_up_cannot_revive_old_node_or_ignore_retained_capacity() {
    for global in [false, true] {
        let f = Fixture::new();
        let s = f.store();
        s.scale("tier", 1).unwrap();
        let e = f.executor("journal", "target");
        let r = request("a");
        let next = request("b");
        f.reserve(&s, &e, &r);
        f.reserve(&s, &e, &next);
        if global {
            s.down().unwrap();
        } else {
            s.scale("tier", 0).unwrap();
        }
        let before = snapshot(&s);
        assert!(e.prepare_managed(&s, &r).is_err());
        assert_eq!(snapshot(&s), before);
        s.scale("tier", 1).unwrap();
        let original = e.prepare_managed(&s, &r).unwrap();
        if global {
            s.down().unwrap();
        } else {
            s.scale("tier", 0).unwrap();
        }
        s.scale("tier", 1).unwrap();
        let before = snapshot(&s);
        assert_eq!(e.prepare_managed(&s, &r).unwrap(), original);
        assert!(e.claim(&s, &r.operation_id, utc()).is_err());
        assert!(e.prepare_managed(&s, &next).is_err());
        assert_eq!(snapshot(&s), before);
        // Raise only desired: proves refusal above was retained capacity, not
        // invalid accounting or malformed second request. No resource is freed.
        s.scale("tier", 2).unwrap();
        e.prepare_managed(&s, &next).unwrap();
        assert_eq!(s.nodes().unwrap().len(), 2);
        assert_eq!(s.launch_count("tier", utc()).unwrap(), 2);
        no_submission(&e);
    }
}
