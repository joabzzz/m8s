//! Actual private SQLite and explicitly authorized harmless local sleep children.
//! No cloud/network/credentials, PID scans, fake proof or billing settlement.
use super::*;
use crate::budget::*;
use std::{
    fs,
    os::unix::{ffi::OsStringExt, fs::DirBuilderExt},
    sync::{
        atomic::{AtomicU64, Ordering as AtomicOrdering},
        mpsc,
    },
    thread,
    time::Instant,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "m8sd-local-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn journal(&self) -> PathBuf {
        let path = self.0.join("journal");
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        path
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn command() -> DevelopmentCommand {
    DevelopmentCommand::new("/bin/sleep".into(), vec!["60".into()]).unwrap()
}
fn plan(journal: PathBuf) -> LocalDevelopmentPlan {
    LocalDevelopmentPlan::new(command(), journal, "namespace", "target", "tier").unwrap()
}
fn period(now: i64) -> CalendarPeriod {
    CalendarPeriod {
        id: "trusted-test-month".into(),
        starts_at: now - 86400,
        ends_at: now + 28 * 86400,
    }
}
fn policy(now: i64) -> AdminBudget {
    AdminBudget {
        owner: "owner".into(),
        approval_id: "local-test-not-product-approval".into(),
        baseline: AccountBaseline {
            prior_lifetime_spent: 0,
            prior_current_period_spent: 0,
            initial_period: period(now),
            external_liability: ExternalLiability::NoneConfirmed,
            observed_at: now,
            status: EvidenceStatus::Complete,
            provenance: "trusted synthetic accounting assertion".into(),
        },
        valid_from: now - 100,
        valid_until: now + 600,
        trial_all_in: 100000,
        monthly_all_in: 100000,
        total_hourly: 10000000,
        instance_hourly: 10000000,
        max_instances: 2,
        max_runtime_seconds: 30,
        minimum_cleanup_seconds: 2,
        minimum_contingency: 1,
    }
}
fn request(id: &str, now: i64, runtime: u64) -> BudgetRequest {
    BudgetRequest {
        owner: "owner".into(),
        run_id: "run".into(),
        request_id: id.into(),
        node_uid: format!("node-{id}"),
        quote: CostQuote {
            provenance: "local-test conservative cost assertion, not billed".into(),
            observed_at: now,
            valid_until: now + 600,
            status: EvidenceStatus::Complete,
            compute_unit: RateUnit::MicroUsdPerHour,
            storage_unit: RateUnit::MicroUsdPerHour,
            totals_unit: TotalUnit::MicroUsd,
            compute_hourly: 3600,
            storage_hourly: 360,
            transfer_total: 10,
            runtime_seconds: runtime,
            cleanup_seconds: 2,
            contingency: 1,
        },
    }
}
fn intent(e: &LocalDevelopmentExecutor, id: &str) -> CreateIntent {
    let mut i = CreateIntent {
        owner: "owner".into(),
        run_id: "run".into(),
        request_id: id.into(),
        node_uid: format!("node-{id}"),
        operation_id: format!("op-{id}"),
        launch_fingerprint: [0; 32],
    };
    i.launch_fingerprint = e.fingerprint(&i).unwrap();
    i
}
fn seed(s: &Store, id: &str, now: i64, runtime: u64, p: &AdminBudget) {
    s.scale("tier", 1).unwrap();
    s.conn
        .execute(
            "INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES (?,'tier','\"Launching\"',?)",
            params![format!("node-{id}"), now],
        )
        .unwrap();
    s.reserve_budget(p, &period(now), &request(id, now, runtime), now)
        .unwrap();
}
struct Fixture {
    _dir: Directory,
    db: PathBuf,
    s: Store,
    e: LocalDevelopmentExecutor,
    now: i64,
}
impl Fixture {
    fn new(runtime: u64) -> Self {
        let d = Directory::new();
        let db = d.0.join("state.db");
        let s = Store::open(&db, "owner").unwrap();
        let e = LocalDevelopmentExecutor::open(plan(d.journal()), "owner").unwrap();
        let now = utc_seconds().unwrap();
        seed(&s, "a", now, runtime, &policy(now));
        Self {
            _dir: d,
            db,
            s,
            e,
            now,
        }
    }
    fn claim(&self) -> CreateClaim {
        self.e
            .prepare(&self.s, &intent(&self.e, "a"), self.now)
            .unwrap();
        self.e.claim(&self.s, "op-a", self.now).unwrap()
    }
}
fn reaped(e: &LocalDevelopmentExecutor) -> bool {
    e.launch()
        .is_some_and(|l| format!("{:?}", l.termination_state()) == "DirectChildReaped")
}
fn wait_reaped(e: &LocalDevelopmentExecutor) {
    let end = Instant::now() + Duration::from_secs(5);
    while !reaped(e) {
        assert!(
            Instant::now() < end,
            "owned direct child did not reap within bound"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn pause_pair() -> (Pause, mpsc::Receiver<()>, mpsc::SyncSender<()>) {
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
fn actual_sleep_result_is_cleanup_metadata_and_stop_is_independent_not_refund() {
    let mut f = Fixture::new(20);
    let c = f.claim();
    let r = match f.e.execute(&f.s, c).unwrap() {
        LocalDevelopmentOutcome::Bound(r) => r,
        _ => panic!("local child should bind"),
    };
    assert!(r.invocation_started_at.is_some());
    assert!(r.cleanup_required);
    assert_eq!(
        r.result.as_ref().unwrap().volumes,
        VolumeBinding::NoneAllocated
    );
    assert_eq!(
        r.result.as_ref().unwrap().instance_id,
        f.e.launch().unwrap().instance().id
    );
    assert_eq!(
        f.s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
    f.s.cancel_create("op-a", utc_seconds().unwrap()).unwrap();
    f.e.stop_handle().request_stop();
    wait_reaped(&f.e);
    assert_eq!(
        f.s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
    assert!(f.e.claim(&f.s, "op-a", utc_seconds().unwrap()).is_err());
}
#[test]
fn canonical_identity_binds_raw_command_path_namespace_target_tier_and_all_intent_fields() {
    let f = Fixture::new(20);
    let i = intent(&f.e, "a");
    let expected = f.e.fingerprint(&i).unwrap();
    let mut hash = Sha256::new();
    hash.update(b"m8s-local-development-plan\0");
    hash.update(1u16.to_be_bytes());
    for b in [
        b"LocalDevelopmentV1".as_slice(),
        f.e.identity.command.as_slice(),
        f.e.identity.journal.as_os_str().as_bytes(),
        b"namespace",
        b"target",
        b"tier",
        b"owner",
        b"run",
        b"a",
        b"node-a",
        b"op-a",
    ] {
        hash.update((b.len() as u32).to_be_bytes());
        hash.update(b);
    }
    assert_eq!(expected, <[u8; 32]>::from(hash.finalize()));
    for n in 0..5 {
        let mut altered = i.clone();
        *match n {
            0 => &mut altered.owner,
            1 => &mut altered.run_id,
            2 => &mut altered.request_id,
            3 => &mut altered.node_uid,
            _ => &mut altered.operation_id,
        } = "different".into();
        assert_ne!(f.e.fingerprint(&altered).unwrap(), expected);
    }
    for n in 0..5 {
        let mut p = plan(f.e.identity.journal.clone());
        match n {
            0 => p.identity.namespace.push('x'),
            1 => p.identity.target.push('x'),
            2 => p.identity.tier.push('x'),
            3 => p.identity.journal.push("other"),
            _ => p.identity.command[0] ^= 1,
        };
        assert_ne!(p.fingerprint(&i).unwrap(), expected);
    }
    let raw = DevelopmentCommand::new(
        "/bin/sleep".into(),
        vec![std::ffi::OsString::from_vec(vec![0xff])],
    )
    .unwrap();
    assert_ne!(raw.fingerprint(), command().fingerprint());
    assert_ne!(
        DevelopmentCommand::new("/bin/sleep".into(), vec!["a".into(), "b".into()])
            .unwrap()
            .fingerprint(),
        DevelopmentCommand::new("/bin/sleep".into(), vec!["ab".into()])
            .unwrap()
            .fingerprint()
    );
    for p in [
        PathBuf::from("relative"),
        PathBuf::from("/tmp/../test"),
        PathBuf::from("/tmp//test"),
        PathBuf::from("/tmp/./test"),
    ] {
        assert!(LocalDevelopmentPlan::new(command(), p, "n", "t", "tier").is_err());
    }
    assert!(
        LocalDevelopmentPlan::new(
            command(),
            f.e.identity.journal.clone(),
            &"n".repeat(65),
            "t",
            "tier"
        )
        .is_err()
    );
}
#[test]
fn actual_store_allocation_second_connection_reopen_and_executor_identity_are_not_strings() {
    for kind in 0..4 {
        let mut f = Fixture::new(20);
        let c = f.claim();
        match kind {
            0 => {
                let second = Store::open(&f.db, "owner").unwrap();
                assert!(f.e.execute(&second, c).is_err());
            }
            1 => {
                let path = f.db.clone();
                drop(f.s);
                let reopened = Store::open(path, "owner").unwrap();
                assert!(f.e.execute(&reopened, c).is_err());
            }
            2 => {
                let other = Directory::new();
                let s = Store::open(other.0.join("state.db"), "owner").unwrap();
                seed(&s, "a", f.now, 20, &policy(f.now));
                assert!(f.e.execute(&s, c).is_err());
            }
            _ => {
                let other = Directory::new();
                let mut e = LocalDevelopmentExecutor::open(plan(other.journal()), "owner").unwrap();
                assert!(e.execute(&f.s, c).is_err());
            }
        }
        assert!(!f.e.submission_started());
    }
}
#[test]
fn unbound_synthetic_wrong_target_changed_command_and_live_tier_refuse() {
    let mut f = Fixture::new(20);
    f.e.prepare(&f.s, &intent(&f.e, "a"), f.now).unwrap();
    let c = f.s.claim_create("op-a", f.now).unwrap();
    assert!(f.e.execute(&f.s, c).is_err());
    assert!(!f.e.submitted);
    let mut f = Fixture::new(20);
    let p = super::super::development::SyntheticPlan::new("n", "t", "tier").unwrap();
    let e = super::super::development::DevelopmentExecutor::new_synthetic(p).unwrap();
    let mut i = intent(&f.e, "a");
    i.launch_fingerprint = super::super::development::SyntheticPlan::new("n", "t", "tier")
        .unwrap()
        .fingerprint(&i)
        .unwrap();
    e.prepare(&f.s, &i, f.now).unwrap();
    let c = e.claim(&f.s, "op-a", f.now).unwrap();
    assert!(f.e.execute(&f.s, c).is_err());
    for kind in 0..3 {
        let mut f = Fixture::new(20);
        let c = f.claim();
        match kind {
            0 => f.e.identity.target.push('x'),
            1 => f.e.identity.command[0] ^= 1,
            _ => {
                f.s.conn
                    .execute("UPDATE gpu_node SET tier='other'", [])
                    .unwrap();
            }
        }
        assert!(f.e.execute(&f.s, c).is_err());
        assert!(!f.e.submitted);
    }
}
#[test]
fn current_caps_own_hold_once_and_other_uncertainty_revalidated_before_child() {
    let mut f = Fixture::new(20);
    let c = f.claim();
    let b = f.s.budget_reservations().unwrap().remove(0);
    let mut a: BudgetAccount = decode(
        &f.s.conn
            .query_row::<String, _, _>("SELECT record FROM budget_account", [], |r| r.get(0))
            .unwrap(),
    )
    .unwrap();
    a.policy.trial_all_in = b.reserved_all_in;
    a.policy.monthly_all_in = b.reserved_all_in;
    a.policy.total_hourly = b.reserved_hourly;
    a.policy.max_instances = 1;
    f.s.conn
        .execute("UPDATE budget_account SET record=?", [encode(&a).unwrap()])
        .unwrap();
    assert!(matches!(
        f.e.execute(&f.s, c).unwrap(),
        LocalDevelopmentOutcome::Bound(_)
    ));
    f.e.stop_handle().request_stop();
    wait_reaped(&f.e);
    for kind in 0..3 {
        let mut f = Fixture::new(20);
        let c = f.claim();
        let mut a: BudgetAccount = decode(
            &f.s.conn
                .query_row::<String, _, _>("SELECT record FROM budget_account", [], |r| r.get(0))
                .unwrap(),
        )
        .unwrap();
        match kind {
            0 => a.policy.trial_all_in = 1,
            1 => a.policy.baseline.external_liability = ExternalLiability::Unknown,
            _ => a.last_at = f.now + 100,
        };
        f.s.conn
            .execute("UPDATE budget_account SET record=?", [encode(&a).unwrap()])
            .unwrap();
        assert!(f.e.execute(&f.s, c).is_err());
        assert!(!f.e.submitted);
    }
    let mut f = Fixture::new(20);
    seed(&f.s, "b", f.now, 20, &policy(f.now));
    let c = f.claim();
    f.s.mark_budget_uncertain("b", f.now).unwrap();
    assert!(f.e.execute(&f.s, c).is_err());
    assert!(!f.e.submitted);
}
#[test]
fn missing_changed_or_corrupt_budget_refuses_and_never_submits() {
    for kind in 0..3 {
        let mut f = Fixture::new(20);
        let c = f.claim();
        match kind {
            0 => {
                f.s.conn
                    .execute("DELETE FROM budget_reservation", [])
                    .unwrap();
            }
            1 => {
                f.s.conn
                    .execute("UPDATE budget_reservation SET record='{'", [])
                    .unwrap();
            }
            _ => {
                let mut b = f.s.budget_reservations().unwrap().remove(0);
                b.request.quote.transfer_total += 1;
                f.s.conn
                    .execute(
                        "UPDATE budget_reservation SET record=?",
                        [encode(&b).unwrap()],
                    )
                    .unwrap();
            }
        }
        assert!(f.e.execute(&f.s, c).is_err());
        assert!(!f.e.submitted);
    }
}
#[test]
fn cancellation_before_invocation_or_down_wins_without_local_submission() {
    for kind in 0..3 {
        let mut f = Fixture::new(20);
        let c = f.claim();
        match kind {
            0 => {
                f.s.cancel_create("op-a", f.now).unwrap();
            }
            1 => {
                f.s.scale("tier", 0).unwrap();
            }
            _ => {
                f.s.conn
                    .execute("UPDATE gpu_node SET deleting=1", [])
                    .unwrap();
            }
        }
        let r = f.e.execute(&f.s, c);
        if kind == 0 {
            assert!(matches!(r.unwrap(), LocalDevelopmentOutcome::NotInvoked));
        } else {
            assert!(r.is_err());
        }
        assert!(!f.e.submitted);
    }
    let mut f = Fixture::new(20);
    let c = f.claim();
    f.e.stop_handle().request_stop();
    assert!(f.e.execute(&f.s, c).is_err());
    assert!(!f.e.submitted);
}
#[test]
fn commit_marker_precedes_child_no_transaction_spans_launch_and_cancel_stop_gap_is_latched() {
    let mut f = Fixture::new(20);
    let c = f.claim();
    let stop = f.e.stop_handle();
    let other = Store::open(&f.db, "owner").unwrap();
    let (h, seen, resume) = pause_pair();
    f.e.hooks.after_commit = Some(h);
    let t = thread::spawn(move || {
        let r = f.e.execute(&f.s, c);
        (f.e, r)
    });
    seen.recv_timeout(Duration::from_secs(5)).unwrap();
    let r = other.create_operations().unwrap().remove(0);
    assert!(r.invocation_started_at.is_some());
    other
        .conn
        .execute_batch("BEGIN IMMEDIATE; COMMIT;")
        .unwrap();
    other.cancel_create("op-a", utc_seconds().unwrap()).unwrap();
    stop.request_stop();
    resume.send(()).unwrap();
    let (e, r) = t.join().unwrap();
    assert!(matches!(r.unwrap(), LocalDevelopmentOutcome::NotInvoked));
    assert!(!e.submitted);
    assert!(
        other
            .create_operations()
            .unwrap()
            .remove(0)
            .cleanup_required
    );
}
#[test]
fn watchdog_deadline_and_stop_progress_while_result_sqlite_writer_is_blocked() {
    let mut f = Fixture::new(3);
    let c = f.claim();
    f.s.conn.busy_timeout(Duration::from_secs(4)).unwrap();
    let writer = Store::open(&f.db, "owner").unwrap();
    let (h, seen, resume) = pause_pair();
    f.e.hooks.after_launch = Some(h);
    let t = thread::spawn(move || {
        let r = f.e.execute(&f.s, c);
        (f.e, r)
    });
    seen.recv_timeout(Duration::from_secs(5)).unwrap();
    writer.conn.execute_batch("BEGIN IMMEDIATE;").unwrap();
    resume.send(()).unwrap();
    let (e, r) = t.join().unwrap(); // Store busy timeout4s, anchored stop<=3s, writer STILL holds TX
    assert!(matches!(r.unwrap(), LocalDevelopmentOutcome::Ambiguous));
    assert!(reaped(&e));
    writer.conn.execute_batch("ROLLBACK;").unwrap();
    assert_eq!(
        writer.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
    assert!(
        writer
            .create_operations()
            .unwrap()
            .remove(0)
            .invocation_started_at
            .is_some()
    );
}
#[test]
fn explicit_stop_during_after_launch_cancel_binds_cancelled_cleanup_only() {
    let mut f = Fixture::new(20);
    let c = f.claim();
    let stop = f.e.stop_handle();
    let other = Store::open(&f.db, "owner").unwrap();
    let (h, seen, resume) = pause_pair();
    f.e.hooks.after_launch = Some(h);
    let t = thread::spawn(move || {
        let r = f.e.execute(&f.s, c);
        (f.e, r)
    });
    seen.recv_timeout(Duration::from_secs(5)).unwrap();
    other.cancel_create("op-a", utc_seconds().unwrap()).unwrap();
    stop.request_stop();
    resume.send(()).unwrap();
    let (e, r) = t.join().unwrap();
    let r = match r.unwrap() {
        LocalDevelopmentOutcome::Bound(r) => r,
        _ => panic!("exact late local result"),
    };
    assert_eq!(r.state, CreateState::Cancelled);
    assert!(r.cleanup_required);
    wait_reaped(&e);
}
#[test]
fn precommit_failure_aftercommit_unwind_drop_and_restart_never_remint() {
    for kind in 0..3 {
        let mut f = Fixture::new(20);
        let c = f.claim();
        if kind == 0 {
            f.e.hooks.before_commit_fail = true;
            assert!(f.e.execute(&f.s, c).is_err());
        } else if kind == 1 {
            f.e.hooks.after_commit_unwind = true;
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.e.execute(&f.s, c)))
                    .is_err()
            );
        } else {
            drop(c);
        }
        assert!(!f.e.submitted);
        assert!(f.e.claim(&f.s, "op-a", utc_seconds().unwrap()).is_err());
        assert_eq!(
            f.s.budget_reservations().unwrap()[0].state,
            BudgetState::Uncertain
        );
        let r = f.s.create_operations().unwrap().remove(0);
        assert_eq!(r.invocation_started_at.is_some(), kind == 1);
        let reopened = Store::open(&f.db, "owner").unwrap();
        assert!(
            reopened
                .claim_create("op-a", utc_seconds().unwrap())
                .is_err()
        );
    }
}
#[test]
fn lost_result_or_store_write_error_retains_owned_launch_stops_and_never_refunds() {
    let mut f = Fixture::new(20);
    let c = f.claim();
    f.e.hooks.lose_response = true;
    assert!(matches!(
        f.e.execute(&f.s, c).unwrap(),
        LocalDevelopmentOutcome::Ambiguous
    ));
    wait_reaped(&f.e);
    assert_eq!(
        f.s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
    assert!(
        f.s.create_operations()
            .unwrap()
            .remove(0)
            .uncertainty_observed
    );
    let mut f = Fixture::new(20);
    let c = f.claim();
    let other = Store::open(&f.db, "owner").unwrap();
    let (h, seen, resume) = pause_pair();
    f.e.hooks.after_launch = Some(h);
    let t = thread::spawn(move || {
        let r = f.e.execute(&f.s, c);
        (f.e, r)
    });
    seen.recv_timeout(Duration::from_secs(5)).unwrap();
    other
        .conn
        .execute("UPDATE budget_account SET record='{'", [])
        .unwrap();
    resume.send(()).unwrap();
    let (e, r) = t.join().unwrap();
    assert!(matches!(r.unwrap(), LocalDevelopmentOutcome::Ambiguous));
    wait_reaped(&e);
    assert!(e.launch().is_some());
    assert!(
        other
            .create_operations()
            .unwrap()
            .remove(0)
            .invocation_started_at
            .is_some()
    );
}
#[test]
fn spawn_failure_burns_and_keeps_full_uncertain_hold_without_retry() {
    let d = Directory::new();
    let bad = DevelopmentCommand::new(d.0.join("missing-trusted-fixture"), vec![]).unwrap();
    let p = LocalDevelopmentPlan::new(bad, d.journal(), "n", "t", "tier").unwrap();
    let mut e = LocalDevelopmentExecutor::open(p, "owner").unwrap();
    let s = Store::open(d.0.join("state.db"), "owner").unwrap();
    let now = utc_seconds().unwrap();
    seed(&s, "a", now, 20, &policy(now));
    let i = intent(&e, "a");
    e.prepare(&s, &i, now).unwrap();
    let c = e.claim(&s, "op-a", now).unwrap();
    assert!(matches!(
        e.execute(&s, c).unwrap(),
        LocalDevelopmentOutcome::Ambiguous
    ));
    assert!(e.submitted);
    assert!(e.local.is_none());
    assert!(e.claim(&s, "op-a", now).is_err());
    assert_eq!(
        s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
}
#[test]
fn original_utc_deadline_expires_without_rebase_and_clock_future_refuses() {
    for kind in 0..3 {
        let mut f = Fixture::new(20);
        let mut c = f.claim();
        match kind {
            0 => {
                c.serving_deadline = f.now - 1;
            }
            1 => {
                c.cleanup_deadline = i64::MAX;
            }
            _ => {
                f.s.conn
                    .execute(
                        "UPDATE budget_account SET record=json_set(record,'$.last_at',?)",
                        // Remain strictly future even if scheduling advances real UTC.
                        [i64::MAX],
                    )
                    .unwrap();
            }
        }
        assert!(f.e.execute(&f.s, c).is_err());
        assert!(!f.e.submitted);
    }
    assert!(absolute(-1).is_err());
    let now = SystemTime::now();
    assert!(LocalDeadlines::new(now, now).is_err());
}
#[test]
fn drop_requests_stop_and_worker_retains_lock_until_owned_reap_without_pid_adoption() {
    let mut f = Fixture::new(20);
    let c = f.claim();
    assert!(matches!(
        f.e.execute(&f.s, c).unwrap(),
        LocalDevelopmentOutcome::Bound(_)
    ));
    let journal = f.e.identity.journal.clone();
    drop(f.e);
    let end = Instant::now() + Duration::from_secs(5);
    let local = loop {
        match Local::open_development(&journal, "owner".into(), command()) {
            Ok(l) => break l,
            Err(m8s_provider::Error::Unavailable) if Instant::now() < end => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("bounded lock/reap reopen failed:{e:?}"),
        }
    };
    drop(local);
    assert_eq!(
        f.s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
}

#[test]
fn stop_request_before_publication_is_latched_then_applied_and_cannot_renew() {
    let mut f = Fixture::new(20);
    let c = f.claim();
    let stop = f.e.stop_handle();
    let (h, seen, resume) = pause_pair();
    f.e.hooks.before_attach = Some(h);
    let t = thread::spawn(move || {
        let r = f.e.execute(&f.s, c);
        (f.e, r)
    });
    seen.recv_timeout(Duration::from_secs(5)).unwrap();
    // Actual Local launch returned, but wrapper has not published its real stop
    // handle yet. Request is only latched here; no prompt-signal claim.
    assert!(stop.0.actual.get().is_none());
    stop.request_stop();
    stop.clone().request_stop();
    assert!(stop.requested());
    resume.send(()).unwrap();
    let (e, r) = t.join().unwrap();
    assert!(matches!(r.unwrap(), LocalDevelopmentOutcome::Bound(_)));
    assert!(stop.0.actual.get().is_some());
    wait_reaped(&e);
    // Opposite order (publish then request) exercised by the explicit stop tests.
}

#[test]
fn two_sqlite_connections_competing_local_executors_mint_only_one_invocation() {
    let f = Fixture::new(20);
    let now = f.now;
    seed(&f.s, "b", now, 20, &policy(now));
    let d = Directory::new();
    let e2 = LocalDevelopmentExecutor::open(plan(d.journal()), "owner").unwrap();
    let s2 = Store::open(&f.db, "owner").unwrap();
    f.e.prepare(&f.s, &intent(&f.e, "a"), now).unwrap();
    e2.prepare(&s2, &intent(&e2, "b"), now).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let b = barrier.clone();
    let a = thread::spawn(move || {
        let mut e = f.e;
        b.wait();
        let result = e.claim(&f.s, "op-a", now).and_then(|c| e.execute(&f.s, c));
        if result.is_ok() {
            e.stop_handle().request_stop();
            wait_reaped(&e);
        }
        result.is_ok()
    });
    let b = thread::spawn(move || {
        let mut e = e2;
        barrier.wait();
        let result = e.claim(&s2, "op-b", now).and_then(|c| e.execute(&s2, c));
        if result.is_ok() {
            e.stop_handle().request_stop();
            wait_reaped(&e);
        }
        result.is_ok()
    });
    assert_eq!(u8::from(a.join().unwrap()) + u8::from(b.join().unwrap()), 1);
}

#[test]
fn database_only_cancel_after_marker_is_cleanup_intent_not_an_automatic_stop_observer() {
    let mut f = Fixture::new(20);
    let c = f.claim();
    let stop = f.e.stop_handle();
    let other = Store::open(&f.db, "owner").unwrap();
    let (h, seen, resume) = pause_pair();
    f.e.hooks.after_commit = Some(h);
    let t = thread::spawn(move || {
        let r = f.e.execute(&f.s, c);
        (f.e, r)
    });
    seen.recv_timeout(Duration::from_secs(5)).unwrap();
    other.cancel_create("op-a", utc_seconds().unwrap()).unwrap();
    // Deliberately no stop request here: this slice has no DB observer. Marker
    // committed first, so this may still create exactly one owned local child.
    resume.send(()).unwrap();
    let (e, r) = t.join().unwrap();
    let r = match r.unwrap() {
        LocalDevelopmentOutcome::Bound(r) => r,
        _ => panic!("post-marker cancellation permits one cleanup-bound result"),
    };
    assert_eq!(r.state, CreateState::Cancelled);
    assert!(r.cleanup_required);
    assert!(e.submitted);
    assert!(!stop.requested());
    stop.request_stop();
    wait_reaped(&e);
    assert_eq!(
        other.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
}

#[test]
fn production_rmw_helpers_cover_both_modification_orders_without_cross_variable_assumptions() {
    let request_first = AtomicU8::new(0);
    assert!(!mark_requested(&request_first));
    assert!(mark_published(&request_first)); // second RMW sees Requested
    assert_eq!(request_first.load(Ordering::Acquire), REQUESTED | PUBLISHED);
    let publish_first = AtomicU8::new(0);
    assert!(!mark_published(&publish_first));
    assert!(mark_requested(&publish_first)); // second RMW sees Published
    assert_eq!(publish_first.load(Ordering::Acquire), REQUESTED | PUBLISHED);
    assert!(mark_requested(&publish_first)); // repeated stop cannot clear bits
    // These are the exact helpers called by production request/attach, not a
    // lookalike model. Source proof is their shared atomic modification order.
}

#[test]
fn concurrent_overlap_uses_same_production_rmw_and_exactly_one_second_operation_signals() {
    let state = Arc::new(AtomicU8::new(0));
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let s = state.clone();
    let b = barrier.clone();
    let request = thread::spawn(move || {
        b.wait();
        mark_requested(&s)
    });
    let s = state.clone();
    let publication = thread::spawn(move || {
        barrier.wait();
        mark_published(&s)
    });
    let a = request.join().unwrap();
    let b = publication.join().unwrap();
    assert_ne!(a, b);
    assert_eq!(state.load(Ordering::Acquire), REQUESTED | PUBLISHED);
    // One overlap fixture is not a stress/memory-model proof: the source-level
    // same-atomic RMW order above covers either possible first operation.
}

#[test]
fn duplicate_private_attach_stops_both_real_handles_and_keeps_both_launches_owned() {
    let mut a = Fixture::new(20);
    let c = a.claim();
    assert!(matches!(
        a.e.execute(&a.s, c).unwrap(),
        LocalDevelopmentOutcome::Bound(_)
    ));
    let mut b = Fixture::new(20);
    let c = b.claim();
    assert!(matches!(
        b.e.execute(&b.s, c).unwrap(),
        LocalDevelopmentOutcome::Bound(_)
    ));
    let stop = a.e.stop_handle();
    assert_eq!(stop.0.state.load(Ordering::Acquire), PUBLISHED);
    let rejected = b.e.launch().unwrap().stop_handle();
    assert!(stop.attach(rejected).is_err());
    assert!(a.e.launch().is_some());
    assert!(b.e.launch().is_some());
    assert_eq!(stop.0.state.load(Ordering::Acquire), REQUESTED | PUBLISHED);
    wait_reaped(&a.e);
    wait_reaped(&b.e);
    assert_eq!(
        a.s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
    assert_eq!(
        b.s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
}

fn finish_local_cleanup(e: &mut LocalDevelopmentExecutor) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        // Test driver only: production poll never loops/sleeps internally.
        match e.poll_local_cleanup() {
            LocalCleanupStatus::LocalOnlyAbsent => return,
            LocalCleanupStatus::Pending { .. } | LocalCleanupStatus::Uncertain => {
                assert!(Instant::now() < end, "owned cleanup did not finalize")
            }
            other => panic!("unexpected requested cleanup diagnostic:{other:?}"),
        }
        thread::sleep(Duration::from_millis(5));
    }
}
fn cleanup_fixture_command(command: DevelopmentCommand) -> Fixture {
    let d = Directory::new();
    let db = d.0.join("state.db");
    let s = Store::open(&db, "owner").unwrap();
    let p = LocalDevelopmentPlan::new(command, d.journal(), "namespace", "target", "tier").unwrap();
    let e = LocalDevelopmentExecutor::open(p, "owner").unwrap();
    let now = utc_seconds().unwrap();
    seed(&s, "a", now, 20, &policy(now));
    Fixture {
        _dir: d,
        db,
        s,
        e,
        now,
    }
}
#[test]
fn cleanup_no_submission_not_requested_and_stop_only_reap_remain_distinct_no_io_request() {
    let mut f = Fixture::new(20);
    assert_eq!(
        f.e.poll_local_cleanup(),
        LocalCleanupStatus::NoLocalSubmission
    );
    let c = f.claim();
    assert!(matches!(
        f.e.execute(&f.s, c).unwrap(),
        LocalDevelopmentOutcome::Bound(_)
    ));
    let journal = f.e.identity.journal.join("state.json");
    let original = fs::read(&journal).unwrap();
    assert_eq!(f.e.poll_local_cleanup(), LocalCleanupStatus::NotRequested);
    f.e.stop_handle().request_stop();
    wait_reaped(&f.e);
    assert_eq!(f.e.poll_local_cleanup(), LocalCleanupStatus::NotRequested);
    assert_eq!(fs::read(&journal).unwrap(), original);
    f.e.request_local_cleanup();
    assert!(f.e.stop.requested());
    assert!(f.e.cleanup_requested);
    assert_eq!(fs::read(&journal).unwrap(), original); // request has no journal mutation
    finish_local_cleanup(&mut f.e);
    let mut never = Fixture::new(20);
    never.e.request_local_cleanup();
    assert_eq!(
        never.e.poll_local_cleanup(),
        LocalCleanupStatus::NoLocalSubmission
    );
    assert!(never.e.claim(&never.s, "op-a", never.now).is_err());
}
#[test]
fn cleanup_success_fresh_tombstone_leaves_exact_store_budget_operation_and_one_shot_unchanged() {
    let mut f = Fixture::new(20);
    let c = f.claim();
    f.e.execute(&f.s, c).unwrap();
    let budgets = f.s.budget_reservations().unwrap();
    let operations = f.s.create_operations().unwrap();
    f.e.request_local_cleanup();
    finish_local_cleanup(&mut f.e);
    assert_eq!(
        f.e.poll_local_cleanup(),
        LocalCleanupStatus::LocalOnlyAbsent
    );
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(f.e.identity.journal.join("state.json")).unwrap())
            .unwrap();
    let id = &f.e.launch().unwrap().instance().id;
    assert_eq!(json["records"][id]["status"], "Absent");
    assert_eq!(f.s.budget_reservations().unwrap(), budgets);
    assert_eq!(f.s.create_operations().unwrap(), operations);
    assert_eq!(budgets[0].state, BudgetState::Uncertain);
    assert!(operations[0].cleanup_required);
    assert!(f.e.submitted);
    assert!(f.e.claim(&f.s, "op-a", utc_seconds().unwrap()).is_err());
    for _ in 0..3 {
        f.e.request_local_cleanup();
        assert_eq!(
            f.e.poll_local_cleanup(),
            LocalCleanupStatus::LocalOnlyAbsent
        );
        assert_eq!(f.e.executed_intent.as_ref().unwrap(), &operations[0].intent);
    }
}
#[test]
fn cleanup_natural_exit_and_ack_or_present_diagnostics_are_not_automatic_absence() {
    let mut f = cleanup_fixture_command(
        DevelopmentCommand::new(
            "/bin/sh".into(),
            vec!["-c".into(), "/bin/sleep 60 & exit 0".into()],
        )
        .unwrap(),
    );
    let c = f.claim();
    f.e.execute(&f.s, c).unwrap();
    wait_reaped(&f.e);
    assert_eq!(f.e.poll_local_cleanup(), LocalCleanupStatus::NotRequested);
    let expected = f.e.executed_instance.as_ref().unwrap().clone();
    let launch = f.e.launch().unwrap();
    assert!(matches!(
        cleanup_observation(
            &expected,
            launch,
            m8s_core::Observation::Present(expected.clone())
        ),
        LocalCleanupStatus::Pending {
            direct_child: m8s_provider_local::LocalTerminationState::DirectChildReaped
        }
    ));
    for kind in 0..5 {
        let mut wrong = expected.clone();
        match kind {
            0 => wrong.id.push('x'),
            1 => wrong.owner.push('x'),
            2 => wrong.node_uid.push('x'),
            3 => wrong.tier.push('x'),
            _ => wrong.state = m8s_core::InstanceState::Exited,
        };
        assert_eq!(
            cleanup_observation(&expected, launch, m8s_core::Observation::Present(wrong)),
            LocalCleanupStatus::Uncertain
        );
    }
    f.e.request_local_cleanup();
    f.e.hooks.cleanup_destroy_error = true;
    assert_eq!(f.e.poll_local_cleanup(), LocalCleanupStatus::Uncertain);
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(f.e.identity.journal.join("state.json")).unwrap())
            .unwrap();
    assert_ne!(json["records"][&expected.id]["status"], "Absent");
    f.e.hooks.cleanup_destroy_error = false;
    finish_local_cleanup(&mut f.e);
}
#[test]
fn cleanup_cancelled_lost_response_and_missing_or_corrupt_store_do_not_block_owned_teardown() {
    for kind in 0..3 {
        let mut f = Fixture::new(20);
        let c = f.claim();
        if kind == 1 {
            f.e.hooks.lose_response = true;
        }
        f.e.execute(&f.s, c).unwrap();
        if kind == 0 {
            f.s.cancel_create("op-a", utc_seconds().unwrap()).unwrap();
        } else if kind == 1 {
            f.s.conn
                .execute("DELETE FROM create_operation", [])
                .unwrap();
        } else {
            f.s.conn
                .execute("UPDATE budget_account SET record='{'", [])
                .unwrap();
        }
        let before: String =
            f.s.conn
                .query_row("SELECT record FROM budget_reservation", [], |r| r.get(0))
                .unwrap();
        f.e.request_local_cleanup();
        finish_local_cleanup(&mut f.e);
        let after: String =
            f.s.conn
                .query_row("SELECT record FROM budget_reservation", [], |r| r.get(0))
                .unwrap();
        assert_eq!(after, before);
        assert_eq!(
            decode::<BudgetReservation>(&after).unwrap().state,
            BudgetState::Uncertain
        );
    }
    let mut f = Fixture::new(20);
    let c = f.claim();
    let other = Store::open(&f.db, "owner").unwrap();
    let (h, seen, resume) = pause_pair();
    f.e.hooks.after_launch = Some(h);
    let t = thread::spawn(move || {
        let r = f.e.execute(&f.s, c);
        (f.e, r)
    });
    seen.recv_timeout(Duration::from_secs(5)).unwrap();
    other.conn.execute("DROP TABLE budget_account", []).unwrap();
    resume.send(()).unwrap();
    let (mut e, r) = t.join().unwrap();
    assert!(matches!(r.unwrap(), LocalDevelopmentOutcome::Ambiguous));
    e.request_local_cleanup();
    finish_local_cleanup(&mut e);
    assert_eq!(
        other.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
}
#[test]
fn cleanup_anchor_replacement_before_and_after_terminal_is_uncertain_never_cached_or_repaired() {
    for terminal in [false, true] {
        let mut f = Fixture::new(20);
        let c = f.claim();
        f.e.execute(&f.s, c).unwrap();
        f.e.request_local_cleanup();
        if terminal {
            finish_local_cleanup(&mut f.e);
        } else {
            wait_reaped(&f.e);
        }
        let journal = f.e.identity.journal.clone();
        let displaced = f._dir.0.join("displaced");
        fs::rename(&journal, &displaced).unwrap();
        fs::DirBuilder::new().mode(0o700).create(&journal).unwrap();
        let original = fs::read(displaced.join("state.json")).unwrap();
        assert_eq!(f.e.poll_local_cleanup(), LocalCleanupStatus::Uncertain);
        assert_eq!(f.e.poll_local_cleanup(), LocalCleanupStatus::Uncertain);
        assert!(fs::read_dir(&journal).unwrap().next().is_none());
        assert_eq!(fs::read(displaced.join("state.json")).unwrap(), original);
        assert!(reaped(&f.e));
    }
}
#[test]
fn cleanup_submitted_failure_and_reopened_unresolved_journal_never_adopt_missing_handles() {
    let d = Directory::new();
    let mut f = cleanup_fixture_command(
        DevelopmentCommand::new(d.0.join("missing-local-fixture"), vec![]).unwrap(),
    );
    let c = f.claim();
    assert!(matches!(
        f.e.execute(&f.s, c).unwrap(),
        LocalDevelopmentOutcome::Ambiguous
    ));
    assert!(f.e.executed_intent.is_some());
    assert_eq!(f.e.poll_local_cleanup(), LocalCleanupStatus::Uncertain);
    f.e.request_local_cleanup();
    assert_eq!(f.e.poll_local_cleanup(), LocalCleanupStatus::Uncertain);
    let mut f = Fixture::new(20);
    let c = f.claim();
    f.e.execute(&f.s, c).unwrap();
    f.e.stop_handle().request_stop();
    wait_reaped(&f.e);
    let journal = f.e.identity.journal.clone();
    drop(f.e);
    let end = Instant::now() + Duration::from_secs(5);
    let mut e = loop {
        match LocalDevelopmentExecutor::open(plan(journal.clone()), "owner") {
            Ok(e) => break e,
            Err(Error::Provider(m8s_provider::Error::Unavailable)) if Instant::now() < end => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("reopen:{e:?}"),
        }
    };
    // A new wrapper has no submission; it does NOT claim global absence or adopt
    // old handles. Concrete unresolved journal still refuses even a fresh UID.
    assert_eq!(
        e.poll_local_cleanup(),
        LocalCleanupStatus::NoLocalSubmission
    );
    e.request_local_cleanup();
    assert_eq!(
        e.poll_local_cleanup(),
        LocalCleanupStatus::NoLocalSubmission
    );
    let result = e.local.as_mut().unwrap().launch_development_until(
        &LaunchSpec {
            owner: "owner".into(),
            node_uid: "new-uid".into(),
            tier: "tier".into(),
        },
        LocalDeadlines::new(
            SystemTime::now() + Duration::from_secs(10),
            SystemTime::now() + Duration::from_secs(12),
        )
        .unwrap(),
    );
    assert!(result.is_err());
    assert!(e.launch.is_none());
}
#[test]
fn cleanup_coordinator_barrier_cannot_block_independent_stop_and_reap_progress() {
    let mut f = Fixture::new(20);
    let c = f.claim();
    f.e.execute(&f.s, c).unwrap();
    f.e.request_local_cleanup();
    let (h, seen, resume) = pause_pair();
    f.e.hooks.before_cleanup_io = Some(h);
    let observer = f.e.launch.as_ref().unwrap().clone();
    let t = thread::spawn(move || {
        let status = f.e.poll_local_cleanup();
        (f.e, status)
    });
    seen.recv_timeout(Duration::from_secs(5)).unwrap();
    // Test driver holds coordinator before any journal call, never deliberately
    // stalls a kernel primitive. Worker receives stop BEFORE this barrier.
    let end = Instant::now() + Duration::from_secs(5);
    while observer.termination_state()
        != m8s_provider_local::LocalTerminationState::DirectChildReaped
    {
        assert!(Instant::now() < end, "worker stalled behind coordinator");
        thread::sleep(Duration::from_millis(5));
    }
    resume.send(()).unwrap();
    let (mut e, status) = t.join().unwrap();
    assert!(reaped(&e));
    assert!(matches!(
        status,
        LocalCleanupStatus::LocalOnlyAbsent | LocalCleanupStatus::Pending { .. }
    ));
    e.hooks.before_cleanup_io = None;
    finish_local_cleanup(&mut e);
}
#[test]
fn cleanup_foreign_identity_target_or_decoy_refuses_before_journal_mutation() {
    for kind in 0..5 {
        let mut a = Fixture::new(20);
        let c = a.claim();
        a.e.execute(&a.s, c).unwrap();
        let mut b = Fixture::new(20);
        let c = b.claim();
        b.e.execute(&b.s, c).unwrap();
        let before = fs::read(a.e.identity.journal.join("state.json")).unwrap();
        let decoy = fs::read(b.e.identity.journal.join("state.json")).unwrap();
        match kind {
            0 => a.e.executed_intent.as_mut().unwrap().owner.push('x'),
            1 => a.e.executed_intent.as_mut().unwrap().node_uid.push('x'),
            2 => a.e.identity.target.push('x'),
            3 => {
                a.e.executed_instance.as_mut().unwrap().id =
                    b.e.launch().unwrap().instance().id.clone()
            }
            _ => std::mem::swap(&mut a.e.launch, &mut b.e.launch),
        }
        a.e.request_local_cleanup();
        assert_eq!(a.e.poll_local_cleanup(), LocalCleanupStatus::Uncertain);
        assert_eq!(
            fs::read(a.e.identity.journal.join("state.json")).unwrap(),
            before
        );
        assert_eq!(
            fs::read(b.e.identity.journal.join("state.json")).unwrap(),
            decoy
        );
        b.e.stop_handle().request_stop();
        wait_reaped(&a.e);
        wait_reaped(&b.e); // exact retained ownership only
    }
}
#[test]
fn cleanup_valid_tombstone_reopens_for_new_uid_journal_only_not_old_uid_or_store_budget() {
    let mut f = Fixture::new(20);
    let c = f.claim();
    f.e.execute(&f.s, c).unwrap();
    f.e.request_local_cleanup();
    finish_local_cleanup(&mut f.e);
    let journal = f.e.identity.journal.clone();
    drop(f.e);
    let end = Instant::now() + Duration::from_secs(5);
    let mut e = loop {
        match LocalDevelopmentExecutor::open(plan(journal.clone()), "owner") {
            Ok(e) => break e,
            Err(Error::Provider(m8s_provider::Error::Unavailable)) if Instant::now() < end => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("reopen:{e:?}"),
        }
    };
    let deadline = || {
        LocalDeadlines::new(
            SystemTime::now() + Duration::from_secs(10),
            SystemTime::now() + Duration::from_secs(12),
        )
        .unwrap()
    };
    let old = LaunchSpec {
        owner: "owner".into(),
        node_uid: "node-a".into(),
        tier: "tier".into(),
    };
    assert!(
        e.local
            .as_mut()
            .unwrap()
            .launch_development_until(&old, deadline())
            .is_err()
    );
    // Direct typed Local use is a TEST of journal reuse, never Store authority.
    let new = LaunchSpec {
        node_uid: "new-local-only-uid".into(),
        ..old
    };
    let launch = e
        .local
        .as_mut()
        .unwrap()
        .launch_development_until(&new, deadline())
        .unwrap();
    launch.stop_handle().request_stop();
    let end = Instant::now() + Duration::from_secs(5);
    while launch.termination_state() != m8s_provider_local::LocalTerminationState::DirectChildReaped
    {
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        f.s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
    assert!(
        f.s.reserve_budget(
            &policy(f.now),
            &period(f.now),
            &request("b", f.now, 20),
            f.now
        )
        .is_err()
    );
    assert!(f.s.claim_create("op-a", f.now).is_err());
    // Test-only exact owned journal finalization of this direct Local launch.
    m8s_provider::Provider::destroy(e.local.as_mut().unwrap(), &launch.instance().id).unwrap();
    assert_eq!(
        m8s_provider::Provider::describe(e.local.as_mut().unwrap(), &launch.instance().id).unwrap(),
        m8s_core::Observation::Absent
    );
}

#[test]
fn cleanup_state_file_replacement_refuses_even_after_reap_and_never_repairs_or_uses_cached_absence()
{
    let mut f = Fixture::new(20);
    let c = f.claim();
    f.e.execute(&f.s, c).unwrap();
    let expected = f.e.executed_instance.as_ref().unwrap();
    assert_eq!(
        cleanup_observation(
            expected,
            f.e.launch().unwrap(),
            m8s_core::Observation::Absent
        ),
        LocalCleanupStatus::Uncertain
    ); // no direct reap yet, even private diagnostic refuses
    f.e.request_local_cleanup();
    wait_reaped(&f.e);
    let journal = f.e.identity.journal.clone();
    fs::rename(journal.join("state.json"), journal.join("displaced.json")).unwrap();
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut replacement = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(journal.join("state.json"))
        .unwrap();
    replacement
        .write_all(b"synthetic replacement; do not repair")
        .unwrap();
    drop(replacement);
    for _ in 0..2 {
        assert_eq!(f.e.poll_local_cleanup(), LocalCleanupStatus::Uncertain);
    }
    assert_eq!(
        fs::read(journal.join("state.json")).unwrap(),
        b"synthetic replacement; do not repair"
    );
    assert!(journal.join("displaced.json").is_file());
    assert!(reaped(&f.e));
    assert_eq!(
        f.s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
}

struct ManagedFixture {
    _dir: Directory,
    db: PathBuf,
    s: Store,
    e: LocalDevelopmentExecutor,
    request: LocalLaunchRequest,
}
fn managed_request(id: &str) -> LocalLaunchRequest {
    LocalLaunchRequest {
        run_id: "run".into(),
        request_id: id.into(),
        operation_id: format!("managed-op-{id}"),
    }
}
fn managed_reserve(
    s: &Store,
    e: &LocalDevelopmentExecutor,
    r: &LocalLaunchRequest,
    at: i64,
    p: &AdminBudget,
) {
    let preview = e.planned_intent(s, r).unwrap();
    let mut b = request(&r.request_id, at, 20);
    b.run_id = r.run_id.clone();
    b.node_uid = preview.node_uid;
    s.reserve_budget(p, &period(at), &b, at).unwrap();
}
fn managed_fixture(at: i64) -> ManagedFixture {
    managed_fixture_policy(at, &policy(at))
}
fn managed_fixture_policy(at: i64, p: &AdminBudget) -> ManagedFixture {
    let d = Directory::new();
    let db = d.0.join("state.db");
    let s = Store::open(&db, "owner").unwrap();
    s.scale("tier", 1).unwrap();
    let e = LocalDevelopmentExecutor::open(plan(d.journal()), "owner").unwrap();
    let r = managed_request("managed-a");
    managed_reserve(&s, &e, &r, at, p);
    ManagedFixture {
        _dir: d,
        db,
        s,
        e,
        request: r,
    }
}
fn managed_prepare(f: &ManagedFixture, at: i64) -> Result<CreateOperation> {
    f.e.prepare_managed_inner(&f.s, &f.request, InvocationClock::Explicit(at))
}
fn managed_counts(s: &Store) -> (i64, i64, i64) {
    s.conn.query_row("SELECT (SELECT count(*) FROM gpu_node),(SELECT count(*) FROM launch_attempt),(SELECT count(*) FROM create_operation)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap()
}
#[test]
fn managed_preview_is_pure_stable_domain_bound_no_uid_override_or_target_rotation() {
    let mut f = managed_fixture(100000);
    let before = managed_counts(&f.s);
    let budget = f.s.budget_reservations().unwrap();
    let i = f.e.planned_intent(&f.s, &f.request).unwrap();
    assert_eq!(i, f.e.planned_intent(&f.s, &f.request).unwrap());
    assert!(i.node_uid.starts_with("local-dev-"));
    let mut h = Sha256::new();
    h.update(b"m8s-managed-local-node\0");
    h.update(1u16.to_be_bytes());
    for value in [&i.owner, &i.run_id, &i.request_id] {
        h.update((value.len() as u32).to_be_bytes());
        h.update(value.as_bytes());
    }
    let uid = format!("local-dev-{:x}", h.finalize());
    assert_eq!(uid, i.node_uid);
    let mut changed = f.request.clone();
    changed.operation_id.push('x');
    let j = f.e.planned_intent(&f.s, &changed).unwrap();
    assert_eq!(j.node_uid, i.node_uid);
    assert_ne!(j.launch_fingerprint, i.launch_fingerprint);
    f.e.identity.target.push('x');
    let j = f.e.planned_intent(&f.s, &f.request).unwrap();
    assert_eq!(j.node_uid, i.node_uid);
    assert_ne!(j.launch_fingerprint, i.launch_fingerprint);
    let other = Directory::new();
    let foreign = Store::open(other.0.join("state.db"), "different-owner").unwrap();
    assert!(f.e.planned_intent(&foreign, &f.request).is_err());
    changed.request_id = "".into();
    assert!(f.e.planned_intent(&f.s, &changed).is_err());
    assert_eq!(managed_counts(&f.s), before);
    assert_eq!(f.s.budget_reservations().unwrap(), budget);
    assert!(!f.e.submitted);
    f.s.conn
        .execute_batch("DROP TABLE create_operation; DROP TABLE gpu_node;")
        .unwrap();
    assert_eq!(
        f.e.planned_intent(&f.s, &f.request).unwrap().node_uid,
        i.node_uid
    ); // no DB reads even when tables unavailable
}
#[test]
fn managed_one_atomic_preparation_exact_retry_does_not_debit_or_renew_clock_or_history() {
    let f = managed_fixture(100000);
    let b = f.s.budget_reservations().unwrap();
    let r = managed_prepare(&f, 100001).unwrap();
    assert_eq!(r.state, CreateState::Prepared);
    assert_eq!(r.intent, f.e.planned_intent(&f.s, &f.request).unwrap());
    assert_eq!(managed_counts(&f.s), (1, 1, 1));
    assert_eq!(f.s.budget_reservations().unwrap(), b);
    assert_eq!(r.prepared_at, 100001);
    let account: String =
        f.s.conn
            .query_row("SELECT record FROM budget_account", [], |r| r.get(0))
            .unwrap();
    assert_eq!(managed_prepare(&f, 100002).unwrap(), r);
    assert_eq!(managed_counts(&f.s), (1, 1, 1));
    assert_eq!(
        f.s.conn
            .query_row::<String, _, _>("SELECT record FROM budget_account", [], |r| r.get(0))
            .unwrap(),
        account
    );
    let node: (String, String, i64, i64) =
        f.s.conn
            .query_row("SELECT tier,phase,orphan,deleting FROM gpu_node", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap();
    assert_eq!(node, ("tier".into(), "\"Launching\"".into(), 0, 0));
}
#[test]
fn managed_retries_cancel_dispatch_reopen_missing_history_are_metadata_never_recreated() {
    for kind in 0..4 {
        let f = managed_fixture(100000);
        let r = managed_prepare(&f, 100000).unwrap();
        match kind {
            0 => {
                f.s.cancel_create(&r.intent.operation_id, 100001).unwrap();
                f.e.stop_handle().request_stop();
            }
            1 => {
                drop(f.e.claim(&f.s, &r.intent.operation_id, 100001).unwrap());
            }
            2 => {
                f.s.conn.execute("DELETE FROM gpu_node", []).unwrap();
            }
            _ => {
                f.s.conn.execute("DELETE FROM launch_attempt", []).unwrap();
            }
        }
        let current = f.s.create_operations().unwrap().remove(0);
        let counts = managed_counts(&f.s);
        let account: String =
            f.s.conn
                .query_row("SELECT record FROM budget_account", [], |r| r.get(0))
                .unwrap();
        assert_eq!(managed_prepare(&f, 100002).unwrap(), current);
        assert_eq!(managed_counts(&f.s), counts);
        assert_eq!(
            f.s.conn
                .query_row::<String, _, _>("SELECT record FROM budget_account", [], |r| r.get(0))
                .unwrap(),
            account
        );
        let journal = f.e.identity.journal.clone();
        drop(f.e);
        let e = LocalDevelopmentExecutor::open(plan(journal), "owner").unwrap();
        let reopened = Store::open(&f.db, "owner").unwrap();
        assert_eq!(
            e.prepare_managed_inner(&reopened, &f.request, InvocationClock::Explicit(100002))
                .unwrap(),
            current
        );
        assert_eq!(managed_counts(&reopened), counts);
        let mut changed = f.request.clone();
        changed.operation_id.push('x');
        assert!(
            e.prepare_managed_inner(&reopened, &changed, InvocationClock::Explicit(100002))
                .is_err()
        );
    }
}
#[test]
fn managed_rotation_wrong_reserved_identity_period_and_unknown_accounting_fail_atomically() {
    for kind in 0..9 {
        let mut f = managed_fixture(100000);
        match kind {
            0 => {
                f.s.conn
                    .execute("DELETE FROM budget_reservation", [])
                    .unwrap();
            }
            1 => {
                f.s.conn
                    .execute("UPDATE budget_reservation SET record='{'", [])
                    .unwrap();
            }
            2 => {
                f.s.mark_budget_uncertain(&f.request.request_id, 100000)
                    .unwrap();
            }
            3..=6 => {
                let mut b = f.s.budget_reservations().unwrap().remove(0);
                match kind {
                    3 => b.request.node_uid.push('x'),
                    4 => b.request.run_id.push('x'),
                    5 => b.period.id.push('x'),
                    _ => b.state = BudgetState::Settled,
                };
                f.s.conn
                    .execute(
                        "UPDATE budget_reservation SET record=?",
                        [encode(&b).unwrap()],
                    )
                    .unwrap();
            }
            7 => {
                let mut a: BudgetAccount = decode(
                    &f.s.conn
                        .query_row::<String, _, _>("SELECT record FROM budget_account", [], |r| {
                            r.get(0)
                        })
                        .unwrap(),
                )
                .unwrap();
                a.policy.baseline.external_liability = ExternalLiability::Unknown;
                f.s.conn
                    .execute("UPDATE budget_account SET record=?", [encode(&a).unwrap()])
                    .unwrap();
            }
            _ => f.request.run_id.push('x'),
        }
        assert!(managed_prepare(&f, 100001).is_err());
        assert_eq!(managed_counts(&f.s), (0, 0, 0));
        assert!(!f.e.submitted);
    }
    for kind in 0..3 {
        let mut f = managed_fixture(100000);
        managed_prepare(&f, 100000).unwrap();
        match kind {
            0 => f.e.identity.target.push('x'),
            1 => f.e.identity.tier.push('x'),
            _ => f.e.identity.command[0] ^= 1,
        }
        assert!(managed_prepare(&f, 100001).is_err());
        assert_eq!(managed_counts(&f.s), (1, 1, 1));
    }
}
#[test]
fn managed_desired_missing_zero_and_all_retained_including_deleting_unknown_consume_capacity() {
    for kind in 0..5 {
        let f = managed_fixture(100000);
        match kind {
            0 => {
                f.s.conn.execute("DELETE FROM tier", []).unwrap();
            }
            1 => {
                f.s.scale("tier", 0).unwrap();
            }
            _ => {
                let phase = match kind {
                    2 => "\"Destroying\"",
                    3 => "unparseable-retained-phase",
                    _ => "\"Booting\"",
                };
                f.s.conn.execute("INSERT INTO gpu_node(uid,tier,phase,created_at,orphan,deleting)VALUES('retained','tier',?,100000,1,1)",[phase]).unwrap();
            }
        }
        assert!(managed_prepare(&f, 100000).is_err());
        assert_eq!(managed_counts(&f.s).1, 0);
        assert_eq!(managed_counts(&f.s).2, 0);
    }
    let f = managed_fixture(100000);
    let r = managed_prepare(&f, 100000).unwrap();
    f.s.scale("tier", 0).unwrap();
    f.s.scale("tier", 1).unwrap();
    assert_eq!(managed_prepare(&f, 100001).unwrap(), r);
    assert!(f.e.claim(&f.s, &r.intent.operation_id, 100001).is_err());
    let deleting: bool =
        f.s.conn
            .query_row("SELECT deleting FROM gpu_node", [], |r| r.get(0))
            .unwrap();
    assert!(deleting);
}
#[test]
fn managed_same_request_and_distinct_request_desired_one_sqlite_races_create_only_one_history() {
    for same in [false, true] {
        let f = managed_fixture(100000);
        let second_dir = Directory::new();
        let e2 = LocalDevelopmentExecutor::open(plan(second_dir.journal()), "owner").unwrap();
        let s2 = Store::open(&f.db, "owner").unwrap();
        let request2 = if same {
            f.request.clone()
        } else {
            managed_request("managed-b")
        };
        if !same {
            managed_reserve(&s2, &e2, &request2, 100000, &policy(100000));
        }
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let b = barrier.clone();
        let db = f.db.clone();
        let a = thread::spawn(move || {
            b.wait();
            f.e.prepare_managed_inner(&f.s, &f.request, InvocationClock::Explicit(100000))
                .is_ok()
        });
        let b = thread::spawn(move || {
            barrier.wait();
            e2.prepare_managed_inner(&s2, &request2, InvocationClock::Explicit(100000))
                .is_ok()
        });
        assert_eq!(u8::from(a.join().unwrap()) + u8::from(b.join().unwrap()), 1);
        let s = Store::open(db, "owner").unwrap();
        assert_eq!(managed_counts(&s), (1, 1, 1));
    }
}
#[test]
fn managed_exact_shared_three_per_hour_gate_cutoff_future_and_history_retention() {
    for offset in [0, 1, 3601] {
        let f = managed_fixture(200000);
        for n in 0..3 {
            f.s.conn
                .execute(
                    "INSERT INTO launch_attempt(uid,tier,at)VALUES(?,'tier',?)",
                    params![format!("history-{n}"), 196400 + offset],
                )
                .unwrap();
        }
        let available = crate::launch_rate_available(&f.s.conn, "tier", 200000).unwrap();
        assert_eq!(available, offset == 0);
        assert_eq!(managed_prepare(&f, 200000).is_ok(), available);
        assert_eq!(managed_counts(&f.s).1, if available { 4 } else { 3 });
    }
    let mut p = policy(200000);
    p.max_instances = 16;
    let f = managed_fixture_policy(200000, &p);
    f.s.scale("tier", 16).unwrap();
    for n in 0..4 {
        let r = managed_request(&format!("rate-{n}"));
        managed_reserve(&f.s, &f.e, &r, 200000, &p);
        let result =
            f.e.prepare_managed_inner(&f.s, &r, InvocationClock::Explicit(200000));
        assert_eq!(result.is_ok(), n < 3);
    }
    assert_eq!(managed_counts(&f.s), (3, 3, 3));
}
#[test]
fn managed_sql_abort_each_insert_account_save_and_nested_transaction_leave_no_partial_state() {
    for stage in [
        "gpu_node",
        "launch_attempt",
        "create_operation",
        "budget_account",
    ] {
        let f = managed_fixture(100000);
        let before = f.s.budget_reservations().unwrap();
        let account: String =
            f.s.conn
                .query_row("SELECT record FROM budget_account", [], |r| r.get(0))
                .unwrap();
        let action = if stage == "budget_account" {
            "UPDATE"
        } else {
            "INSERT"
        };
        f.s.conn.execute_batch(&format!("CREATE TRIGGER abort_managed BEFORE {action} ON {stage} BEGIN SELECT RAISE(ABORT,'synthetic stage failure'); END;")).unwrap();
        assert!(managed_prepare(&f, 100001).is_err());
        assert_eq!(managed_counts(&f.s), (0, 0, 0));
        assert_eq!(f.s.budget_reservations().unwrap(), before);
        assert_eq!(
            f.s.conn
                .query_row::<String, _, _>("SELECT record FROM budget_account", [], |r| r.get(0))
                .unwrap(),
            account
        );
        f.s.conn
            .execute_batch("DROP TRIGGER abort_managed")
            .unwrap();
        managed_prepare(&f, 100001).unwrap();
        assert_eq!(managed_counts(&f.s), (1, 1, 1));
    }
    let f = managed_fixture(100000);
    f.s.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(managed_prepare(&f, 100000).is_err());
    assert_eq!(managed_counts(&f.s), (0, 0, 0));
    f.s.conn.execute_batch("ROLLBACK").unwrap();
}
#[test]
fn managed_current_budget_deadline_and_claim_rechecks_remain_failclosed() {
    for at in [99999, 100020, 100031] {
        let f = managed_fixture(100000);
        assert!(managed_prepare(&f, at).is_err());
        assert_eq!(managed_counts(&f.s), (0, 0, 0));
    }
    let f = managed_fixture(100000);
    let r = managed_prepare(&f, 100000).unwrap();
    f.s.mark_budget_uncertain(&f.request.request_id, 100001)
        .unwrap();
    assert!(f.e.claim(&f.s, &r.intent.operation_id, 100001).is_err());
    assert_eq!(managed_prepare(&f, 100001).unwrap(), r); // metadata retry not eligibility
}
#[test]
fn managed_public_real_clock_path_composes_one_child_cleanup_with_same_held_budget() {
    let at = utc_seconds().unwrap();
    let mut f = managed_fixture(at);
    let r = f.e.prepare_managed(&f.s, &f.request).unwrap();
    let c =
        f.e.claim(&f.s, &r.intent.operation_id, utc_seconds().unwrap())
            .unwrap();
    assert!(matches!(
        f.e.execute(&f.s, c).unwrap(),
        LocalDevelopmentOutcome::Bound(_)
    ));
    f.e.request_local_cleanup();
    finish_local_cleanup(&mut f.e);
    assert_eq!(
        f.s.budget_reservations().unwrap()[0].state,
        BudgetState::Uncertain
    );
    assert!(f.s.create_operations().unwrap()[0].cleanup_required);
}

#[test]
fn managed_current_caps_and_other_uncertain_liability_block_before_new_rows() {
    for kind in 0..4 {
        let f = managed_fixture(100000);
        match kind {
            0 => {
                let mut a: BudgetAccount = decode(
                    &f.s.conn
                        .query_row::<String, _, _>("SELECT record FROM budget_account", [], |r| {
                            r.get(0)
                        })
                        .unwrap(),
                )
                .unwrap();
                a.policy.trial_all_in = 1;
                f.s.conn
                    .execute("UPDATE budget_account SET record=?", [encode(&a).unwrap()])
                    .unwrap();
            }
            1 => {
                let other = managed_request("other-liability");
                managed_reserve(&f.s, &f.e, &other, 100000, &policy(100000));
                f.s.mark_budget_uncertain(&other.request_id, 100000)
                    .unwrap();
            }
            2 => {
                let mut b = f.s.budget_reservations().unwrap().remove(0);
                b.request.owner = "wrong-owner".into();
                f.s.conn
                    .execute(
                        "UPDATE budget_reservation SET record=?",
                        [encode(&b).unwrap()],
                    )
                    .unwrap();
            }
            _ => {
                let mut b = f.s.budget_reservations().unwrap().remove(0);
                b.binding = Some(ResourceBinding {
                    owner: "owner".into(),
                    run_id: "run".into(),
                    node_uid: b.request.node_uid.clone(),
                    instance_id: "already-bound".into(),
                    volumes: VolumeBinding::NoneAllocated,
                    provenance: "synthetic fixture".into(),
                    observed_at: 100000,
                    status: EvidenceStatus::Complete,
                });
                f.s.conn
                    .execute(
                        "UPDATE budget_reservation SET record=?",
                        [encode(&b).unwrap()],
                    )
                    .unwrap();
            }
        }
        assert!(managed_prepare(&f, 100001).is_err());
        assert_eq!(managed_counts(&f.s), (0, 0, 0));
    }
}
#[test]
fn managed_real_utc_is_sampled_after_waiting_for_sql_writer_not_before() {
    let at = utc_seconds().unwrap();
    let f = managed_fixture(at);
    let writer = Store::open(&f.db, "owner").unwrap();
    writer.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (started, seen) = mpsc::sync_channel(1);
    let db = f.db.clone();
    let task = thread::spawn(move || {
        started.send(()).unwrap();
        f.e.prepare_managed(&f.s, &f.request)
    });
    seen.recv_timeout(Duration::from_secs(5)).unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    let current = loop {
        let now = utc_seconds().unwrap();
        if now > at {
            break now;
        }
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(5));
    };
    let raw: String = writer
        .conn
        .query_row("SELECT record FROM budget_account", [], |r| r.get(0))
        .unwrap();
    let mut account: BudgetAccount = decode(&raw).unwrap();
    account.last_at = current;
    writer
        .conn
        .execute(
            "UPDATE budget_account SET record=?",
            [encode(&account).unwrap()],
        )
        .unwrap();
    writer.conn.execute_batch("COMMIT").unwrap();
    let r = task.join().unwrap().unwrap();
    assert!(r.prepared_at >= current);
    let s = Store::open(db, "owner").unwrap();
    assert_eq!(managed_counts(&s), (1, 1, 1));
}

#[test]
fn managed_existing_uid_or_attempt_collision_is_not_upsert_adoption_or_history_repair() {
    for attempt in [false, true] {
        let f = managed_fixture(100000);
        let uid = f.e.planned_intent(&f.s, &f.request).unwrap().node_uid;
        let before = f.s.budget_reservations().unwrap();
        let account: String =
            f.s.conn
                .query_row("SELECT record FROM budget_account", [], |r| r.get(0))
                .unwrap();
        if attempt {
            f.s.conn
                .execute(
                    "INSERT INTO launch_attempt(uid,tier,at)VALUES(?,'other-tier',100000)",
                    [&uid],
                )
                .unwrap();
        } else {
            f.s.conn.execute("INSERT INTO gpu_node(uid,tier,phase,created_at)VALUES(?,'other-tier','unknown-retained',100000)",[&uid]).unwrap();
        }
        let counts = managed_counts(&f.s);
        assert!(managed_prepare(&f, 100001).is_err());
        assert_eq!(managed_counts(&f.s), counts);
        assert_eq!(f.s.budget_reservations().unwrap(), before);
        assert_eq!(
            f.s.conn
                .query_row::<String, _, _>("SELECT record FROM budget_account", [], |r| r.get(0))
                .unwrap(),
            account
        );
    }
}

#[test]
fn managed_ignored_sql_writes_cannot_commit_partial_intent_or_stale_account_clock() {
    for stage in [
        "gpu_node",
        "launch_attempt",
        "create_operation",
        "budget_account",
    ] {
        let f = managed_fixture(100000);
        let before = f.s.budget_reservations().unwrap();
        let account: String =
            f.s.conn
                .query_row("SELECT record FROM budget_account", [], |r| r.get(0))
                .unwrap();
        let action = if stage == "budget_account" {
            "UPDATE"
        } else {
            "INSERT"
        };
        f.s.conn.execute_batch(&format!("CREATE TRIGGER ignore_managed BEFORE {action} ON {stage} BEGIN SELECT RAISE(IGNORE); END;")).unwrap();
        assert!(managed_prepare(&f, 100001).is_err());
        assert_eq!(managed_counts(&f.s), (0, 0, 0));
        assert_eq!(f.s.budget_reservations().unwrap(), before);
        assert_eq!(
            f.s.conn
                .query_row::<String, _, _>("SELECT record FROM budget_account", [], |r| r.get(0))
                .unwrap(),
            account
        );
    }
}
