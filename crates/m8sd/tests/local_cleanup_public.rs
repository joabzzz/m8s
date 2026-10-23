//! Independent PUBLIC cleanup diagnostics for explicitly owned sleep children.
//! Not provider absence, billing, admission, settlement or parent-loss proof.
#![cfg(all(unix, feature = "local-development"))]
use m8s_provider_local::{DevelopmentCommand, LocalTerminationState};
use m8sd::{
    Store,
    budget::*,
    operation::{
        CreateIntent, CreateOperation,
        local_development::{
            LocalCleanupStatus, LocalDevelopmentExecutor, LocalDevelopmentOutcome,
            LocalDevelopmentPlan,
        },
    },
};
use rusqlite::{Connection, params};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    ops::{Deref, DerefMut},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
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
fn plan(journal: &Path) -> LocalDevelopmentPlan {
    LocalDevelopmentPlan::new(
        DevelopmentCommand::new("/bin/sleep".into(), vec!["60".into()]).unwrap(),
        journal.into(),
        "namespace",
        "target",
        "tier",
    )
    .unwrap()
}
struct Fixture {
    dir: PathBuf,
    db: PathBuf,
    at: i64,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "m8sd-cleanup-public-{}-{}",
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
    fn journal(&self) -> PathBuf {
        let p = self.dir.join("journal");
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
                provenance: "synthetic-explicit-baseline".into(),
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
                runtime_seconds: 20,
                cleanup_seconds: 3,
                contingency: 1,
            },
        }
    }
    fn seed(&self, id: &str) -> Store {
        let s = Store::open(&self.db, OWNER).unwrap();
        s.initialize_budget(&self.policy(), &self.period(), self.at)
            .unwrap();
        s.scale("tier", 1).unwrap();
        // Fixture setup only. All authority comes from public claim APIs; never
        // mint a private claim, opaque proof or fabricated absence/billing grant.
        Connection::open(&self.db).unwrap().execute(
            "INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES (?,'tier','\"Launching\"',?)",
            params![format!("node-{id}"), self.at],
        ).unwrap();
        s.reserve_budget(&self.policy(), &self.period(), &self.request(id), self.at)
            .unwrap();
        s
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !thread::panicking() {
            fs::remove_dir_all(&self.dir).unwrap();
        }
    }
}
fn snapshot(s: &Store, id: &str) -> (BudgetReservation, CreateOperation) {
    (
        s.budget_reservations()
            .unwrap()
            .into_iter()
            .find(|r| r.request.request_id == format!("request-{id}"))
            .unwrap(),
        s.create_operations()
            .unwrap()
            .into_iter()
            .find(|r| r.intent.operation_id == format!("operation-{id}"))
            .unwrap(),
    )
}
fn assert_unchanged(s: &Store, id: &str, before: &(BudgetReservation, CreateOperation)) {
    assert_eq!(&snapshot(s, id), before);
    assert_eq!(before.0.state, BudgetState::Uncertain);
    assert!(before.0.reserved_all_in > 0);
    assert!(before.0.settlement.is_none());
    assert!(before.1.cleanup_required);
    assert!(
        s.nodes()
            .unwrap()
            .iter()
            .all(|n| n.phase == m8s_core::Phase::Launching)
    );
}
fn reaped(e: &LocalDevelopmentExecutor) -> bool {
    e.launch()
        .is_some_and(|l| l.termination_state() == LocalTerminationState::DirectChildReaped)
}
fn wait_reaped(e: &LocalDevelopmentExecutor) -> bool {
    let end = Instant::now() + Duration::from_secs(4);
    while !reaped(e) {
        if Instant::now() >= end {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    true
}
struct ChildScope(Option<LocalDevelopmentExecutor>);
impl ChildScope {
    fn open(journal: &Path) -> Self {
        let end = Instant::now() + Duration::from_secs(4);
        loop {
            match LocalDevelopmentExecutor::open(plan(journal), OWNER) {
                Ok(e) => return Self(Some(e)),
                Err(m8sd::Error::Provider(m8s_core::provider::Error::Unavailable))
                    if Instant::now() < end =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => panic!("local executor open failed; non-lock errors are not retried"),
            }
        }
    }
    fn close(&mut self) -> bool {
        let reaped = if let Some(e) = &self.0 {
            e.stop_handle().request_stop();
            e.launch().is_none() || wait_reaped(e)
        } else {
            true
        };
        drop(self.0.take());
        reaped
    }
}
impl Deref for ChildScope {
    type Target = LocalDevelopmentExecutor;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap()
    }
}
impl DerefMut for ChildScope {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().unwrap()
    }
}
impl Drop for ChildScope {
    fn drop(&mut self) {
        // No journal reopening here: cleanup must not write a displaced anchor's
        // replacement directory. Stop/reap uses the retained public handle only.
        if !self.close() {
            if thread::panicking() {
                eprintln!("controlled cleanup child remains uncertain");
            } else {
                panic!("controlled cleanup child remains uncertain");
            }
        }
    }
}
fn launch(e: &mut ChildScope, s: &Store, id: &str) {
    let mut i = CreateIntent {
        owner: OWNER.into(),
        run_id: "synthetic-run".into(),
        request_id: format!("request-{id}"),
        node_uid: format!("node-{id}"),
        operation_id: format!("operation-{id}"),
        launch_fingerprint: [0; 32],
    };
    i.launch_fingerprint = e.fingerprint(&i).unwrap();
    e.prepare(s, &i, utc()).unwrap();
    let claim = e.claim(s, &i.operation_id, utc()).unwrap();
    assert!(matches!(
        e.execute(s, claim).unwrap(),
        LocalDevelopmentOutcome::Bound(_)
    ));
    assert!(e.submission_started());
    assert!(e.launch().is_some());
}
fn finish(e: &mut LocalDevelopmentExecutor) {
    e.request_local_cleanup();
    let end = Instant::now() + Duration::from_secs(4);
    loop {
        match e.poll_local_cleanup() {
            LocalCleanupStatus::LocalOnlyAbsent => {
                assert!(reaped(e));
                return;
            }
            LocalCleanupStatus::Pending { direct_child } => {
                assert_ne!(direct_child, LocalTerminationState::Uncertain);
                assert!(
                    Instant::now() < end,
                    "local finalization exceeded observed test bound"
                );
                thread::sleep(Duration::from_millis(10));
            }
            other => panic!("unexpected retained-owned cleanup diagnostic: {other:?}"),
        }
    }
}

#[test]
fn never_submitted_is_per_object_diagnostic_and_failed_restart_submission_stays_uncertain() {
    let f = Fixture::new();
    let journal = f.journal();
    let s = f.seed("a");
    let mut e = ChildScope::open(&journal);
    assert_eq!(
        e.poll_local_cleanup(),
        LocalCleanupStatus::NoLocalSubmission
    );
    launch(&mut e, &s, "a");
    let before = snapshot(&s, "a");
    e.stop_handle().request_stop();
    assert!(wait_reaped(&e));
    assert_eq!(
        e.poll_local_cleanup(),
        LocalCleanupStatus::NotRequested,
        "stop/reap alone is not journal finalization"
    );
    assert!(e.close());
    assert_unchanged(&s, "a", &before);
    let fresh = Fixture::new();
    let other_store = fresh.seed("b");
    let mut reopened = ChildScope::open(&journal);
    assert_eq!(
        reopened.poll_local_cleanup(),
        LocalCleanupStatus::NoLocalSubmission
    );
    // That diagnostic cannot prove absence of the unresolved old allocation.
    let mut i = CreateIntent {
        owner: OWNER.into(),
        run_id: "synthetic-run".into(),
        request_id: "request-b".into(),
        node_uid: "node-b".into(),
        operation_id: "operation-b".into(),
        launch_fingerprint: [0; 32],
    };
    i.launch_fingerprint = reopened.fingerprint(&i).unwrap();
    reopened.prepare(&other_store, &i, utc()).unwrap();
    let claim = reopened.claim(&other_store, "operation-b", utc()).unwrap();
    assert!(matches!(
        reopened.execute(&other_store, claim).unwrap(),
        LocalDevelopmentOutcome::Ambiguous
    ));
    assert!(reopened.submission_started());
    assert!(reopened.launch().is_none());
    let failed = snapshot(&other_store, "b");
    assert_eq!(reopened.poll_local_cleanup(), LocalCleanupStatus::Uncertain);
    reopened.request_local_cleanup();
    assert_eq!(reopened.poll_local_cleanup(), LocalCleanupStatus::Uncertain);
    assert_unchanged(&other_store, "b", &failed);
    assert_unchanged(&s, "a", &before);
}

#[test]
fn explicit_retained_cleanup_is_local_only_exact_journal_and_never_changes_accounting() {
    let f = Fixture::new();
    let journal = f.journal();
    let s = f.seed("a");
    let mut e = ChildScope::open(&journal);
    launch(&mut e, &s, "a");
    let before = snapshot(&s, "a");
    let id = e.launch().unwrap().instance().id.clone();
    assert_eq!(e.poll_local_cleanup(), LocalCleanupStatus::NotRequested);
    finish(&mut e);
    assert_eq!(e.launch().unwrap().instance().id, id);
    assert_eq!(e.poll_local_cleanup(), LocalCleanupStatus::LocalOnlyAbsent);
    assert_eq!(e.poll_local_cleanup(), LocalCleanupStatus::LocalOnlyAbsent);
    assert_unchanged(&s, "a", &before);
    assert!(e.claim(&s, "operation-a", utc()).is_err());
    // No SettlementEvidence/FinalBilling conversion or self-settlement is used.
}

#[test]
fn repeated_terminal_poll_is_fresh_and_displaced_anchor_is_uncertain_without_decoy_mutation() {
    let f = Fixture::new();
    let journal = f.journal();
    let s = f.seed("a");
    let mut e = ChildScope::open(&journal);
    launch(&mut e, &s, "a");
    let before = snapshot(&s, "a");
    finish(&mut e);
    assert_eq!(e.poll_local_cleanup(), LocalCleanupStatus::LocalOnlyAbsent);
    let displaced = f.dir.join("displaced");
    fs::rename(&journal, &displaced).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&journal).unwrap();
    let decoy = journal.join("owned-decoy");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&decoy)
        .unwrap();
    file.write_all(b"synthetic-decoy-untouched").unwrap();
    drop(file);
    let original = fs::read(displaced.join("state.json")).unwrap();
    for _ in 0..2 {
        assert_eq!(e.poll_local_cleanup(), LocalCleanupStatus::Uncertain);
    }
    assert_eq!(fs::read(&decoy).unwrap(), b"synthetic-decoy-untouched");
    assert_eq!(
        fs::read_dir(&journal).unwrap().count(),
        1,
        "poll must not repair replacement anchor"
    );
    assert_eq!(fs::read(displaced.join("state.json")).unwrap(), original);
    assert!(reaped(&e));
    assert_unchanged(&s, "a", &before);
    // ChildScope Drop never opens/repairs the replacement journal.
}

#[test]
fn retained_cleanup_needs_no_live_store_and_reopen_preserves_original_liability() {
    let f = Fixture::new();
    let s = f.seed("a");
    let mut e = ChildScope::open(&f.journal());
    launch(&mut e, &s, "a");
    let before = snapshot(&s, "a");
    drop(s);
    finish(&mut e);
    assert_eq!(e.poll_local_cleanup(), LocalCleanupStatus::LocalOnlyAbsent);
    let s = Store::open(&f.db, OWNER).unwrap();
    assert_unchanged(&s, "a", &before);
    assert!(s.claim_create("operation-a", utc()).is_err());
}

#[test]
fn journal_finalization_allows_new_uid_only_in_fresh_budget_store_not_original_account() {
    let f = Fixture::new();
    let journal = f.journal();
    let s = f.seed("a");
    let mut e = ChildScope::open(&journal);
    launch(&mut e, &s, "a");
    let before = snapshot(&s, "a");
    let old_id = e.launch().unwrap().instance().id.clone();
    finish(&mut e);
    assert!(e.close());
    assert!(
        s.reserve_budget(&f.policy(), &f.period(), &f.request("b"), utc())
            .is_err(),
        "local tombstone never refunds the original Uncertain account"
    );
    assert_unchanged(&s, "a", &before);
    // Separate synthetic accounting fixture isolates journal-layer permission.
    // It is NOT permission to reset/reconcile real outstanding owner liabilities.
    let fresh = Fixture::new();
    let other_store = fresh.seed("b");
    let mut next = ChildScope::open(&journal);
    assert_eq!(
        next.poll_local_cleanup(),
        LocalCleanupStatus::NoLocalSubmission
    );
    launch(&mut next, &other_store, "b");
    assert_ne!(next.launch().unwrap().instance().id, old_id);
    assert_eq!(next.launch().unwrap().instance().node_uid, "node-b");
    let other_hold = snapshot(&other_store, "b");
    finish(&mut next);
    assert_unchanged(&other_store, "b", &other_hold);
    assert_unchanged(&s, "a", &before);
}
