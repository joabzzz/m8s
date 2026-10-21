//! PUBLIC-API Phase B: explicitly controlled /bin/sleep children only.
//! Owned Launching SQL fixtures are not admission, absence or billing proof.
//! No private hooks, PID scans/signals, cloud/network/TLS/credentials/Keychain.
#![cfg(all(unix, feature = "local-development"))]
use m8s_provider_local::{DevelopmentCommand, Local, LocalTerminationState};
use m8sd::{
    Store,
    budget::*,
    operation::{
        CreateIntent, CreateOperation, CreateState,
        development::{DevelopmentExecutor, SyntheticPlan},
        local_development::{
            LocalDevelopmentExecutor, LocalDevelopmentOutcome, LocalDevelopmentPlan,
        },
    },
};
use rusqlite::{Connection, OpenFlags, params};
use std::{
    fs,
    ops::{Deref, DerefMut},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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
fn command() -> DevelopmentCommand {
    DevelopmentCommand::new("/bin/sleep".into(), vec!["60".into()]).unwrap()
}
fn plan(journal: &Path) -> LocalDevelopmentPlan {
    LocalDevelopmentPlan::new(command(), journal.into(), "namespace", "target", "tier").unwrap()
}
fn raw_intent(id: &str) -> CreateIntent {
    CreateIntent {
        owner: OWNER.into(),
        run_id: "synthetic-run".into(),
        request_id: format!("request-{id}"),
        node_uid: format!("node-{id}"),
        operation_id: format!("operation-{id}"),
        launch_fingerprint: [0; 32],
    }
}
fn intent(e: &LocalDevelopmentExecutor, id: &str) -> CreateIntent {
    let mut i = raw_intent(id);
    i.launch_fingerprint = e.fingerprint(&i).unwrap();
    i
}
struct Fixture {
    dir: PathBuf,
    db: PathBuf,
    at: i64,
    runtime: u64,
}
impl Fixture {
    fn new(runtime: u64) -> Self {
        Self::at(runtime, utc())
    }
    fn at(runtime: u64, at: i64) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "m8sd-local-public-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        Self {
            db: dir.join("state.db"),
            dir,
            at,
            runtime,
        }
    }
    fn journal(&self, name: &str) -> PathBuf {
        let p = self.dir.join(name);
        fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        p
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
                provenance: "trusted-synthetic-baseline".into(),
            },
            valid_from: self.at - 100,
            valid_until: self.at + 600,
            trial_all_in: 100000,
            monthly_all_in: 100000,
            total_hourly: 10_000_000,
            instance_hourly: 10_000_000,
            max_instances: 2,
            max_runtime_seconds: 30,
            minimum_cleanup_seconds: 3,
            minimum_contingency: 1,
        }
    }
    fn open(&self) -> Store {
        Store::open(&self.db, OWNER).unwrap()
    }
    fn seed(&self, id: &str) -> Store {
        let s = self.open();
        s.scale("tier", 1).unwrap();
        // ONLY owned fixture setup is direct SQL; never a claim/proof constructor.
        Connection::open(&self.db).unwrap().execute(
            "INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES (?,'tier','\"Launching\"',?)",
            params![format!("node-{id}"), self.at],
        ).unwrap();
        s.reserve_budget(
            &self.policy(),
            &self.period(),
            &BudgetRequest {
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
                    runtime_seconds: self.runtime,
                    cleanup_seconds: 3,
                    contingency: 1,
                },
            },
            self.at,
        )
        .unwrap();
        s
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Preserve a failing fixture for diagnosis rather than unlinking a
        // possibly still-retained worker journal during unwind.
        if !thread::panicking() {
            fs::remove_dir_all(&self.dir).unwrap();
        }
    }
}
fn operation(s: &Store, id: &str) -> CreateOperation {
    s.create_operations()
        .unwrap()
        .into_iter()
        .find(|r| r.intent.operation_id == format!("operation-{id}"))
        .unwrap()
}
fn budget(s: &Store, id: &str) -> BudgetReservation {
    s.budget_reservations()
        .unwrap()
        .into_iter()
        .find(|r| r.request.request_id == format!("request-{id}"))
        .unwrap()
}
fn state(e: &LocalDevelopmentExecutor) -> LocalTerminationState {
    e.launch().unwrap().termination_state()
}
fn wait_reaped(e: &LocalDevelopmentExecutor, end: Instant) -> bool {
    while state(e) != LocalTerminationState::DirectChildReaped {
        if Instant::now() >= end {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    true
}
// Always request stop and boundedly observe retained-child cleanup on every
// exit, including assertion unwind. Executor Drop also independently stops.
// This remains the real kernel/worker contract, not an unconditional guarantee
// against kernel stalls/permission denial or loss of the parent process.
struct ChildScope {
    executor: Option<LocalDevelopmentExecutor>,
    journal: PathBuf,
}
impl ChildScope {
    fn open(journal: &Path) -> Self {
        Self {
            executor: Some(LocalDevelopmentExecutor::open(plan(journal), OWNER).unwrap()),
            journal: journal.into(),
        }
    }
    fn stop_and_reap(&self) {
        self.stop_handle().request_stop();
        assert!(
            wait_reaped(self, Instant::now() + Duration::from_secs(4)),
            "controlled child did not reap after stop"
        );
    }
    fn close(&mut self) -> bool {
        // Dropping the executor requests stop even when no retained launch is
        // available after an ambiguous startup/result. Worker owns exact lock
        // until reap; opening it again is only a LOCAL ownership-release oracle.
        drop(self.executor.take());
        let end = Instant::now() + Duration::from_secs(4);
        loop {
            match Local::open_development(&self.journal, OWNER.into(), command()) {
                Ok(local) => {
                    drop(local);
                    return true;
                }
                Err(m8s_provider::Error::Unavailable) => {} // Expected retained-worker lock.
                Err(_) => return false, // Never hide storage/validation failures behind retry.
            }
            if Instant::now() >= end {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Deref for ChildScope {
    type Target = LocalDevelopmentExecutor;
    fn deref(&self) -> &Self::Target {
        self.executor.as_ref().unwrap()
    }
}
impl DerefMut for ChildScope {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.executor.as_mut().unwrap()
    }
}
impl Drop for ChildScope {
    fn drop(&mut self) {
        if let Some(e) = &self.executor {
            e.stop_handle().request_stop();
            let reaped =
                e.launch().is_none() || wait_reaped(e, Instant::now() + Duration::from_secs(4));
            let released = self.close();
            if !reaped || !released {
                if thread::panicking() {
                    eprintln!("controlled child cleanup remains uncertain");
                } else {
                    panic!("controlled child cleanup remains uncertain");
                }
            }
        }
    }
}
fn no_submission(e: &LocalDevelopmentExecutor, s: &Store) {
    assert!(!e.submission_started());
    assert!(e.launch().is_none());
    assert!(operation(s, "a").invocation_started_at.is_none());
    assert_eq!(budget(s, "a").state, BudgetState::Uncertain);
}
fn assert_hold(s: &Store, id: &str, original: u64) {
    let b = budget(s, id);
    assert_eq!(b.state, BudgetState::Uncertain);
    assert_eq!(b.reserved_all_in, original);
    assert!(b.settlement.is_none());
    assert!(operation(s, id).cleanup_required);
    assert!(
        s.nodes()
            .unwrap()
            .iter()
            .all(|n| n.phase == m8s_core::Phase::Launching)
    );
}
fn launch_bound(e: &mut ChildScope, s: &Store, at: i64, id: &str) -> CreateOperation {
    e.prepare(s, &intent(e, id), at).unwrap();
    let claim = e.claim(s, &format!("operation-{id}"), at).unwrap();
    match e.execute(s, claim).unwrap() {
        LocalDevelopmentOutcome::Bound(op) => *op,
        _ => panic!("expected controlled local child binding"),
    }
}

#[test]
fn store_driver_unbound_and_synthetic_claim_substitution_never_submits_local() {
    for case in [
        "same-db",
        "different-db",
        "reopen",
        "driver",
        "unbound",
        "synthetic",
    ] {
        let f = Fixture::new(20);
        let other = Fixture::at(20, f.at);
        let journal = f.journal("local");
        let mut e = ChildScope::open(&journal);
        let mut source = Some(f.seed("a"));
        let i = intent(&e, "a");
        let claim = if case == "synthetic" {
            let p = SyntheticPlan::new("namespace", "target", "tier").unwrap();
            let mut i = raw_intent("a");
            i.launch_fingerprint = p.fingerprint(&i).unwrap();
            let synthetic = DevelopmentExecutor::new_synthetic(p).unwrap();
            synthetic
                .prepare(source.as_ref().unwrap(), &i, f.at)
                .unwrap();
            let c = synthetic
                .claim(source.as_ref().unwrap(), "operation-a", f.at)
                .unwrap();
            assert_eq!(synthetic.invocations(), 0);
            c
        } else {
            e.prepare(source.as_ref().unwrap(), &i, f.at).unwrap();
            if case == "unbound" {
                source
                    .as_ref()
                    .unwrap()
                    .claim_create("operation-a", f.at)
                    .unwrap()
            } else {
                e.claim(source.as_ref().unwrap(), "operation-a", f.at)
                    .unwrap()
            }
        };
        let before = operation(source.as_ref().unwrap(), "a");
        let target = match case {
            "different-db" => {
                let s = other.seed("a");
                e.prepare(&s, &i, f.at).unwrap();
                drop(e.claim(&s, "operation-a", f.at).unwrap());
                assert_eq!(operation(&s, "a"), before);
                s
            }
            "reopen" => {
                drop(source.take());
                f.open()
            }
            _ => f.open(),
        };
        // Driver/unbound/synthetic cases use ORIGINAL Store; origin tests alone
        // substitute a new handle, so origin rejection cannot mask driver checks.
        if case == "driver" {
            assert!(e.close());
            e = ChildScope::open(&journal);
        }
        let target = if matches!(case, "driver" | "unbound" | "synthetic") {
            source.as_ref().unwrap()
        } else {
            &target
        };
        assert!(e.execute(target, claim).is_err());
        no_submission(&e, target);
    }
}

#[test]
fn canonical_command_journal_target_tier_identity_and_bad_fingerprints_refuse() {
    let f = Fixture::new(20);
    let journal = f.journal("one");
    let other = f.journal("two");
    let raw = raw_intent("a");
    let expected = plan(&journal).fingerprint(&raw).unwrap();
    for p in [
        LocalDevelopmentPlan::new(
            DevelopmentCommand::new("/bin/sleep".into(), vec!["61".into()]).unwrap(),
            journal.clone(),
            "namespace",
            "target",
            "tier",
        )
        .unwrap(),
        plan(&other),
        LocalDevelopmentPlan::new(command(), journal.clone(), "namespace", "target-2", "tier")
            .unwrap(),
        LocalDevelopmentPlan::new(
            command(),
            journal.clone(),
            "namespace",
            "target",
            "other-tier",
        )
        .unwrap(),
    ] {
        assert_ne!(p.fingerprint(&raw).unwrap(), expected);
    }
    let left = LocalDevelopmentPlan::new(command(), journal.clone(), "ab", "c", "tier").unwrap();
    let right = LocalDevelopmentPlan::new(command(), journal.clone(), "a", "bc", "tier").unwrap();
    assert_ne!(
        left.fingerprint(&raw).unwrap(),
        right.fingerprint(&raw).unwrap()
    );
    let args = |values: &[&str]| {
        DevelopmentCommand::new("/bin/sleep".into(), values.iter().map(Into::into).collect())
            .unwrap()
    };
    assert_ne!(
        LocalDevelopmentPlan::new(args(&["1", "23"]), journal.clone(), "n", "t", "tier")
            .unwrap()
            .fingerprint(&raw)
            .unwrap(),
        LocalDevelopmentPlan::new(args(&["12", "3"]), journal.clone(), "n", "t", "tier")
            .unwrap()
            .fingerprint(&raw)
            .unwrap()
    );
    assert!(
        LocalDevelopmentPlan::new(command(), PathBuf::from("relative"), "n", "t", "tier").is_err()
    );
    assert!(LocalDevelopmentPlan::new(command(), journal.join(".."), "n", "t", "tier").is_err());
    let s = f.seed("a");
    let e = ChildScope::open(&journal);
    let mut bad = intent(&e, "a");
    bad.launch_fingerprint = [0; 32];
    assert!(e.prepare(&s, &bad, f.at).is_err());
    assert!(!e.submission_started());
    let wrong =
        LocalDevelopmentPlan::new(command(), other, "namespace", "target", "other-tier").unwrap();
    let mut i = raw;
    i.launch_fingerprint = wrong.fingerprint(&i).unwrap();
    let wrong = LocalDevelopmentExecutor::open(wrong, OWNER).unwrap();
    assert!(wrong.prepare(&s, &i, f.at).is_err());
    assert!(!wrong.submission_started());
    assert!(s.create_operations().unwrap().is_empty());
}

#[test]
fn real_local_submission_has_committed_marker_full_hold_and_no_remint_or_ready() {
    let f = Fixture::new(15);
    let s = f.seed("a");
    let mut e = ChildScope::open(&f.journal("local"));
    let original = budget(&s, "a").reserved_all_in;
    e.prepare(&s, &intent(&e, "a"), f.at).unwrap();
    let claim = e.claim(&s, "operation-a", f.at).unwrap();
    assert_eq!(budget(&s, "a").state, BudgetState::Uncertain);
    assert!(operation(&s, "a").invocation_started_at.is_none());
    assert!(!e.submission_started());
    let bound = match e.execute(&s, claim).unwrap() {
        LocalDevelopmentOutcome::Bound(r) => r,
        _ => panic!("expected retained local launch"),
    };
    assert!(e.submission_started());
    assert!(bound.invocation_started_at.is_some());
    let read = Connection::open_with_flags(&f.db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let json: String = read
        .query_row(
            "SELECT record FROM create_operation WHERE owner=?",
            [OWNER],
            |r| r.get(0),
        )
        .unwrap();
    let persisted: CreateOperation = serde_json::from_str(&json).unwrap();
    assert_eq!(persisted, *bound);
    let json: String = read
        .query_row(
            "SELECT record FROM budget_reservation WHERE owner=?",
            [OWNER],
            |r| r.get(0),
        )
        .unwrap();
    let held: BudgetReservation = serde_json::from_str(&json).unwrap();
    assert_eq!(held.state, BudgetState::Uncertain);
    assert_eq!(held.reserved_all_in, original);
    assert_eq!(
        bound.result.as_ref().unwrap().instance_id,
        e.launch().unwrap().instance().id
    );
    assert_eq!(
        bound.result.as_ref().unwrap().volumes,
        VolumeBinding::NoneAllocated
    );
    assert!(e.claim(&s, "operation-a", utc()).is_err());
    assert!(s.claim_create("operation-a", utc()).is_err());
    assert_hold(&s, "a", original);
    e.stop_and_reap();
    assert_hold(&s, "a", original);
    // Public return-time visibility, not an independent before-callback hook.
}

#[test]
fn stop_requested_before_execute_refuses_without_a_child_or_submission() {
    let f = Fixture::new(15);
    let s = f.seed("a");
    let mut e = ChildScope::open(&f.journal("local"));
    e.prepare(&s, &intent(&e, "a"), f.at).unwrap();
    let claim = e.claim(&s, "operation-a", f.at).unwrap();
    e.stop_handle().request_stop();
    assert!(e.execute(&s, claim).is_err());
    no_submission(&e, &s);
    assert!(e.claim(&s, "operation-a", utc()).is_err());
}

#[test]
fn durable_cancel_paired_with_stop_reaps_retained_child_without_settlement() {
    let f = Fixture::new(15);
    let s = f.seed("a");
    let mut e = ChildScope::open(&f.journal("local"));
    let original = budget(&s, "a").reserved_all_in;
    launch_bound(&mut e, &s, f.at, "a");
    let stop = e.stop_handle();
    let cancelled = s.cancel_create("operation-a", utc()).unwrap();
    stop.request_stop();
    assert_eq!(cancelled.state, CreateState::Cancelled);
    e.stop_and_reap();
    assert_eq!(state(&e), LocalTerminationState::DirectChildReaped);
    assert_hold(&s, "a", original);
    assert!(e.claim(&s, "operation-a", utc()).is_err());
}

#[test]
fn database_only_cancel_has_no_observer_and_test_always_requests_stop_after_assertion() {
    let f = Fixture::new(20);
    let s = f.seed("a");
    let mut e = ChildScope::open(&f.journal("local"));
    let original = budget(&s, "a").reserved_all_in;
    launch_bound(&mut e, &s, f.at, "a");
    assert!(
        f.at + 20 - utc() >= 3,
        "insufficient deadline headroom for no-observer assertion"
    );
    assert_eq!(state(&e), LocalTerminationState::Running);
    s.cancel_create("operation-a", utc()).unwrap();
    assert_eq!(operation(&s, "a").state, CreateState::Cancelled);
    assert_eq!(
        state(&e),
        LocalTerminationState::Running,
        "DB cleanup intent is not a stop-handle observer"
    );
    // Assertion unwind also runs ChildScope Drop, requests stop and polls reap.
    e.stop_and_reap();
    assert_hold(&s, "a", original);
}

#[test]
fn absolute_utc_deadline_is_not_rebased_to_execute_and_worker_reaps_with_bounded_poll() {
    let f = Fixture::new(6);
    let s = f.seed("a");
    let mut e = ChildScope::open(&f.journal("local"));
    let original = budget(&s, "a").reserved_all_in;
    e.prepare(&s, &intent(&e, "a"), f.at).unwrap();
    let delay_end = Instant::now() + Duration::from_secs(4);
    while utc() < f.at + 3 {
        assert!(
            Instant::now() < delay_end,
            "real UTC did not reach delayed dispatch point"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let claim = e.claim(&s, "operation-a", utc()).unwrap();
    assert_eq!(claim.serving_deadline(), f.at + 6);
    assert_eq!(claim.cleanup_deadline(), f.at + 9);
    assert!(
        f.at + 6 - utc() >= 2,
        "insufficient remaining real-clock horizon"
    );
    assert!(matches!(
        e.execute(&s, claim).unwrap(),
        LocalDevelopmentOutcome::Bound(_)
    ));
    // Two-second OS/poll grace after ORIGINAL UTC deadline. Rebased six seconds
    // from delayed execute would exceed this bound. No fractional equality test.
    let upper = UNIX_EPOCH + Duration::from_secs(u64::try_from(f.at + 8).unwrap());
    let allowance = upper.duration_since(SystemTime::now()).unwrap();
    assert!(
        wait_reaped(&e, Instant::now() + allowance),
        "absolute-deadline worker did not reap within observed bound"
    );
    assert_eq!(state(&e), LocalTerminationState::DirectChildReaped);
    assert_hold(&s, "a", original);
}

#[test]
fn drop_releases_retained_worker_only_after_local_cleanup_but_restart_blocks_fresh_uid() {
    let f = Fixture::new(15);
    let s = f.seed("a");
    let journal = f.journal("local");
    let mut e = ChildScope::open(&journal);
    let original = budget(&s, "a").reserved_all_in;
    launch_bound(&mut e, &s, f.at, "a");
    // Exercise actual executor Drop, not explicit pre-drop stop/reap. Reopening
    // requires worker's exact journal lock release after cleanup; not absence.
    assert!(
        e.close(),
        "dropped executor's worker did not release journal within cleanup bound"
    );
    assert_hold(&s, "a", original);
    let fresh = Fixture::new(15);
    let other_store = fresh.seed("b");
    let mut reopened = ChildScope::open(&journal);
    let other_hold = budget(&other_store, "b").reserved_all_in;
    let i = intent(&reopened, "b");
    reopened.prepare(&other_store, &i, fresh.at).unwrap();
    let claim = reopened
        .claim(&other_store, "operation-b", fresh.at)
        .unwrap();
    assert!(matches!(
        reopened.execute(&other_store, claim).unwrap(),
        LocalDevelopmentOutcome::Ambiguous
    ));
    assert!(reopened.submission_started());
    assert!(
        reopened.launch().is_none(),
        "no fresh child handle after unresolved journal restart"
    );
    assert!(operation(&other_store, "b").invocation_started_at.is_some());
    assert_hold(&other_store, "b", other_hold);
    assert!(reopened.claim(&other_store, "operation-b", utc()).is_err());
}
