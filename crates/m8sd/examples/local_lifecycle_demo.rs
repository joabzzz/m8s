//! Explicit harmless LOCAL walkthrough. Synthetic accounting is NOT approval.
//! No paid provider, credentials, model, admission, readiness or billing proof.
#[cfg(unix)]
mod demo {
    use fs2::FileExt;
    use m8s_provider_local::{DevelopmentCommand, LocalTerminationState};
    use m8sd::{
        Store,
        budget::{
            AccountBaseline, AdminBudget, BudgetRequest, BudgetState, CalendarPeriod, CostQuote,
            EvidenceStatus, ExternalLiability, RateUnit, TotalUnit,
        },
        operation::{
            CreateState,
            local_development::{
                LocalCleanupStatus, LocalDevelopmentExecutor, LocalDevelopmentOutcome,
                LocalDevelopmentPlan, LocalLaunchRequest,
            },
        },
    };
    use rusqlite::{Connection, OpenFlags, params};
    use serde_json::{Value, json};
    use std::{
        ffi::OsString,
        fs::{self, File, OpenOptions},
        io::Write,
        os::unix::{
            ffi::OsStrExt,
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        },
        path::{Component, Path, PathBuf},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    const LABEL: &str = "SYNTHETIC-NOT-ADMIN-APPROVAL";
    const OWNER: &str = "SYNTHETIC-NOT-ADMIN-APPROVAL-local-demo";
    const TIER: &str = "SYNTHETIC-local-demo";
    const REQUEST: &str = "synthetic-local-demo-request-1";
    const OP: &str = "synthetic-local-demo-operation-1";
    const MAX_CAP: u64 = 1_000_000;
    const RUNTIME: u64 = 3;
    const GRACE: u64 = 2;
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Scenario {
        Cancel,
        Deadline,
    }
    struct Options {
        state: PathBuf,
        cap: u64,
        scenario: Scenario,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Error {
        Arguments,
        ExistingState,
        PrivateState,
        Persistence,
        SyntheticCapRefused,
        LaunchUncertain,
        CleanupUncertain,
        Verification,
        Output,
    }
    type Result<T> = std::result::Result<T, Error>;
    impl Error {
        fn code(self) -> &'static str {
            match self {
                Self::Arguments => "invalid-arguments",
                Self::ExistingState => "state-path-must-be-new-no-reset",
                Self::PrivateState => "private-state-unavailable",
                Self::Persistence => "persistence-error",
                Self::SyntheticCapRefused => "synthetic-cap-refused-before-child",
                Self::LaunchUncertain => "local-launch-uncertain-no-retry",
                Self::CleanupUncertain => "local-cleanup-uncertain-no-refund",
                Self::Verification => "retained-liability-verification-failed",
                Self::Output => "output-unavailable",
            }
        }
    }
    fn path_valid(path: &Path) -> bool {
        let raw = path.as_os_str().as_bytes();
        let normalized: PathBuf = path.components().collect();
        path.is_absolute()
            && raw.len() <= 4096
            && !raw.contains(&0)
            && raw == normalized.as_os_str().as_bytes()
            && path
                .components()
                .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
    }
    fn parse(args: Vec<OsString>) -> Result<Options> {
        if args.len() > 16 {
            return Err(Error::Arguments);
        }
        let (mut synthetic, mut state, mut cap, mut scenario) = (false, None, None, None);
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            match arg.to_str() {
                Some("--synthetic-accounting") if !synthetic => synthetic = true,
                Some("--state-dir") if state.is_none() => {
                    state = Some(PathBuf::from(it.next().ok_or(Error::Arguments)?))
                }
                Some("--synthetic-cap-micro-usd") if cap.is_none() => {
                    let raw = it.next().ok_or(Error::Arguments)?;
                    let raw = raw.to_str().ok_or(Error::Arguments)?;
                    if raw.is_empty() || raw.len() > 7 || !raw.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(Error::Arguments);
                    }
                    let n = raw.parse::<u64>().map_err(|_| Error::Arguments)?;
                    if n == 0 || n > MAX_CAP {
                        return Err(Error::Arguments);
                    }
                    cap = Some(n);
                }
                Some("--scenario") if scenario.is_none() => {
                    scenario = Some(match it.next().as_deref().and_then(|s| s.to_str()) {
                        Some("cancel") => Scenario::Cancel,
                        Some("deadline") => Scenario::Deadline,
                        _ => return Err(Error::Arguments),
                    })
                }
                _ => return Err(Error::Arguments),
            }
        }
        let state = state.ok_or(Error::Arguments)?;
        if !synthetic || !path_valid(&state) {
            return Err(Error::Arguments);
        }
        Ok(Options {
            state,
            cap: cap.ok_or(Error::Arguments)?,
            scenario: scenario.ok_or(Error::Arguments)?,
        })
    }
    fn private(file: &File, directory: bool) -> Result<()> {
        let m = file.metadata().map_err(|_| Error::PrivateState)?;
        // SAFETY: geteuid has no memory-safety preconditions.
        if m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o7777 != if directory { 0o700 } else { 0o600 }
            || if directory {
                !m.is_dir()
            } else {
                !m.is_file() || m.nlink() != 1
            }
        {
            return Err(Error::PrivateState);
        }
        Ok(())
    }
    fn directory(path: &Path) -> Result<File> {
        let f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| Error::PrivateState)?;
        private(&f, true)?;
        Ok(f)
    }
    struct FreshState {
        path: PathBuf,
        lock: File,
        dir: File,
    }
    impl FreshState {
        fn create(path: &Path) -> Result<Self> {
            // SQLite NOFOLLOW also rejects symlinked ancestors. Require an
            // already-existing canonical trusted parent before creating state.
            let parent = path.parent().ok_or(Error::PrivateState)?;
            if fs::canonicalize(parent).map_err(|_| Error::PrivateState)? != parent {
                return Err(Error::PrivateState);
            }
            // No create_dir_all, reset, deletion, force, stale-lock repair or chmod.
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        Error::ExistingState
                    } else {
                        Error::PrivateState
                    }
                })?;
            let dir = directory(path)?;
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path.join("demo.lock"))
                .map_err(|_| Error::PrivateState)?;
            private(&lock, false)?;
            lock.try_lock_exclusive().map_err(|_| Error::PrivateState)?;
            let state = Self {
                path: path.into(),
                lock,
                dir,
            };
            state.recheck()?;
            Ok(state)
        }
        fn recheck(&self) -> Result<()> {
            private(&self.dir, true)?;
            private(&self.lock, false)?;
            let current = directory(&self.path)?;
            let lock = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(self.path.join("demo.lock"))
                .map_err(|_| Error::PrivateState)?;
            private(&lock, false)?;
            for (a, b) in [(&self.dir, &current), (&self.lock, &lock)] {
                let a = a.metadata().map_err(|_| Error::PrivateState)?;
                let b = b.metadata().map_err(|_| Error::PrivateState)?;
                if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
                    return Err(Error::PrivateState);
                }
            }
            Ok(())
        }
    }
    fn now() -> Result<i64> {
        i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| Error::Persistence)?
                .as_secs(),
        )
        .map_err(|_| Error::Persistence)
    }
    fn fixtures(
        cap: u64,
        at: i64,
        node_uid: &str,
    ) -> Result<(AdminBudget, CalendarPeriod, BudgetRequest)> {
        let end = at.checked_add(60).ok_or(Error::Persistence)?;
        let period = CalendarPeriod {
            id: format!("{LABEL}-28-day-fixture-NOT-real-calendar"),
            starts_at: at,
            ends_at: at.checked_add(28 * 86400).ok_or(Error::Persistence)?,
        };
        let hourly = cap.checked_mul(3600).ok_or(Error::Arguments)?;
        let policy = AdminBudget {
            owner: OWNER.into(),
            approval_id: LABEL.into(),
            baseline: AccountBaseline {
                prior_lifetime_spent: 0,
                prior_current_period_spent: 0,
                initial_period: period.clone(),
                external_liability: ExternalLiability::NoneConfirmed,
                observed_at: at,
                status: EvidenceStatus::Complete,
                provenance: format!("{LABEL}-fictional-baseline-NOT-provider-reconciliation"),
            },
            valid_from: at,
            valid_until: end,
            trial_all_in: cap,
            monthly_all_in: cap,
            total_hourly: hourly,
            instance_hourly: hourly,
            max_instances: 1,
            max_runtime_seconds: RUNTIME,
            minimum_cleanup_seconds: GRACE,
            minimum_contingency: 10,
        };
        let request = BudgetRequest {
            owner: OWNER.into(),
            run_id: format!("{LABEL}-run"),
            request_id: REQUEST.into(),
            node_uid: node_uid.into(),
            quote: CostQuote {
                provenance: format!("{LABEL}-fictional-rates-NOT-measured-cost"),
                observed_at: at,
                valid_until: end,
                status: EvidenceStatus::Complete,
                compute_unit: RateUnit::MicroUsdPerHour,
                storage_unit: RateUnit::MicroUsdPerHour,
                totals_unit: TotalUnit::MicroUsd,
                compute_hourly: 3600,
                storage_hourly: 3600,
                transfer_total: 10,
                runtime_seconds: RUNTIME,
                cleanup_seconds: GRACE,
                contingency: 10,
            },
        };
        Ok((policy, period, request))
    }
    fn read_only_connection(path: &Path) -> Result<Connection> {
        Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| Error::Persistence)
    }
    fn event(out: &mut impl Write, value: Value) -> Result<()> {
        let bytes = serde_json::to_vec(&value).map_err(|_| Error::Output)?;
        if bytes.len() > 1024 {
            return Err(Error::Output);
        }
        out.write_all(&bytes)
            .and_then(|_| out.write_all(b"\n"))
            .map_err(|_| Error::Output)
    }
    fn cleanup(executor: &mut LocalDevelopmentExecutor) -> LocalCleanupStatus {
        executor.request_local_cleanup();
        let end = Instant::now() + Duration::from_secs(7);
        for _ in 0..700 {
            // Caller polling only; no latency bound around FS/kernel.
            let state = executor.poll_local_cleanup();
            if matches!(
                state,
                LocalCleanupStatus::LocalOnlyAbsent | LocalCleanupStatus::NoLocalSubmission
            ) {
                return state;
            }
            if Instant::now() >= end {
                return state;
            }
            thread::sleep(Duration::from_millis(10));
        }
        LocalCleanupStatus::Uncertain
    }
    fn verify_retained(store: &Store, path: &Path, hold: u64, scenario: Scenario) -> Result<()> {
        let reservations = store
            .budget_reservations()
            .map_err(|_| Error::Verification)?;
        let operations = store.create_operations().map_err(|_| Error::Verification)?;
        if reservations.len() != 1
            || reservations[0].state != BudgetState::Uncertain
            || reservations[0].reserved_all_in != hold
            || reservations[0].settlement.is_some()
            || operations.len() != 1
            || operations[0].intent.operation_id != OP
            || operations[0].dispatched_at.is_none()
            || operations[0].invocation_started_at.is_none()
            || !operations[0].cleanup_required
            || operations[0].result.is_none()
            || operations[0].state
                != if scenario == Scenario::Cancel {
                    CreateState::Cancelled
                } else {
                    CreateState::DispatchConsumed
                }
        {
            return Err(Error::Verification);
        }
        let c = read_only_connection(path)?;
        let yes:bool=c.query_row("SELECT (SELECT count(*) FROM gpu_node WHERE uid=? AND phase='\"Launching\"')=1 AND (SELECT count(*) FROM launch_attempt WHERE uid=?)=1",params![operations[0].intent.node_uid,operations[0].intent.node_uid],|r|r.get(0)).map_err(|_|Error::Verification)?;
        if !yes {
            return Err(Error::Verification);
        }
        Ok(())
    }
    fn drive(
        store: &Store,
        executor: &mut LocalDevelopmentExecutor,
        opts: &Options,
        db: &Path,
        hold: u64,
        state: &FreshState,
        out: &mut impl Write,
    ) -> Result<()> {
        let request = LocalLaunchRequest {
            run_id: format!("{LABEL}-run"),
            request_id: REQUEST.into(),
            operation_id: OP.into(),
        };
        executor
            .prepare_managed(store, &request)
            .map_err(|_| Error::Persistence)?;
        let at = now()?;
        let claim = executor
            .claim(store, OP, at)
            .map_err(|_| Error::LaunchUncertain)?;
        event(
            out,
            json!({"phase":"claim_consumed","accounting":"synthetic-not-approval"}),
        )?;
        if !matches!(
            executor
                .execute(store, claim)
                .map_err(|_| Error::LaunchUncertain)?,
            LocalDevelopmentOutcome::Bound(_)
        ) {
            return Err(Error::LaunchUncertain);
        }
        event(
            out,
            json!({"phase":"local_child_spawned","invocation_committed":true,"service":"not-admitted-not-model-service"}),
        )?;
        if opts.scenario == Scenario::Cancel {
            store
                .cancel_create(OP, now()?)
                .map_err(|_| Error::Persistence)?;
            executor.request_local_cleanup();
            event(
                out,
                json!({"phase":"stop_requested","cause":"paired-durable-cancel-and-local-stop"}),
            )?;
        } else {
            let end = Instant::now() + Duration::from_secs(7);
            loop {
                match executor
                    .launch()
                    .ok_or(Error::LaunchUncertain)?
                    .termination_state()
                {
                    LocalTerminationState::DirectChildReaped => break,
                    LocalTerminationState::Uncertain => return Err(Error::CleanupUncertain),
                    _ if Instant::now() >= end => return Err(Error::CleanupUncertain),
                    _ => thread::sleep(Duration::from_millis(10)),
                }
            }
            event(
                out,
                json!({"phase":"deadline_stop_observed","direct_child":"DirectChildReaped","deadline":"original-absolute-no-renewal"}),
            )?;
        }
        if cleanup(executor) != LocalCleanupStatus::LocalOnlyAbsent {
            return Err(Error::CleanupUncertain);
        }
        state.recheck()?;
        verify_retained(store, db, hold, opts.scenario)?;
        event(
            out,
            json!({"phase":"summary","mode":"local-development-demo","accounting":"synthetic-not-approval",
            "journal_cleanup":"LocalOnlyAbsent","billing":"unknown","budget_state":"Uncertain","reserved_hold_retained":true,
            "simulated_hold_micro_usd":hold,"claim_state":"consumed","cleanup_required":true,"node_state":"managed-local-Launching-not-reconciled-fleet"}),
        )
    }
    fn run(opts: Options, out: &mut impl Write) -> Result<()> {
        // All caller arguments validated before creating ANY state/effects.
        let state = FreshState::create(&opts.state)?;
        let db = opts.state.join("state.db");
        let store = Store::open(&db, OWNER).map_err(|_| Error::PrivateState)?;
        state.recheck()?;
        event(
            out,
            json!({"phase":"fixture_initialized","mode":"local-development-demo","accounting":"synthetic-not-approval","synthetic_cap_micro_usd":opts.cap}),
        )?;
        store.scale(TIER, 1).map_err(|_| Error::Persistence)?;
        let journal = opts.state.join("local-journal");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&journal)
            .map_err(|_| Error::PrivateState)?;
        let command = DevelopmentCommand::new("/bin/sleep".into(), vec!["15".into()])
            .map_err(|_| Error::Persistence)?;
        let plan = LocalDevelopmentPlan::new(
            command,
            journal,
            "SYNTHETIC-demo",
            "fixed-harmless-sleep",
            TIER,
        )
        .map_err(|_| Error::Persistence)?;
        let mut executor =
            LocalDevelopmentExecutor::open(plan, OWNER).map_err(|_| Error::Persistence)?;
        let request_metadata = LocalLaunchRequest {
            run_id: format!("{LABEL}-run"),
            request_id: REQUEST.into(),
            operation_id: OP.into(),
        };
        let preview = executor
            .planned_intent(&store, &request_metadata)
            .map_err(|_| Error::Persistence)?;
        let at = now()?;
        let (policy, period, request) = fixtures(opts.cap, at, &preview.node_uid)?;
        let reservation = store
            .reserve_budget(&policy, &period, &request, at)
            .map_err(|error| match error {
                m8sd::Error::Invalid => Error::SyntheticCapRefused,
                _ => Error::Persistence,
            })?;
        event(
            out,
            json!({"phase":"reservation_persisted","simulated_hold_micro_usd":reservation.reserved_all_in,"accounting":"synthetic-not-approval"}),
        )?;
        state.recheck()?;
        let result = drive(
            &store,
            &mut executor,
            &opts,
            &db,
            reservation.reserved_all_in,
            &state,
            out,
        );
        if let Err(error) = result {
            let status = cleanup(&mut executor); // Never retry create or recreate state.
            let label = match status {
                LocalCleanupStatus::LocalOnlyAbsent => "LocalOnlyAbsent",
                LocalCleanupStatus::NoLocalSubmission => "NoLocalSubmission",
                LocalCleanupStatus::Pending { .. } => "Pending",
                _ => "Uncertain",
            };
            let _ = event(
                out,
                json!({"phase":"error","error":error.code(),"journal_cleanup":label,"billing":"unknown","state_preserved":true}),
            );
            return Err(error);
        }
        Ok(())
    }

    pub fn main() -> i32 {
        let args = std::env::args_os().skip(1).take(17).collect();
        let mut out = std::io::stdout().lock();
        match parse(args).and_then(|opts| run(opts, &mut out)) {
            Ok(()) => 0,
            Err(e) => {
                let _ = event(
                    &mut out,
                    json!({"phase":"failure","error":e.code(),"accounting":"synthetic-not-approval","no_reset":true}),
                );
                1
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        struct Root(PathBuf);
        impl Root {
            fn new() -> Self {
                let p = std::env::temp_dir().canonicalize().unwrap().join(format!(
                    "m8s-lifecycle-demo-test-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
                Self(p)
            }
            fn state(&self) -> PathBuf {
                self.0.join("new-demo-state")
            }
        }
        impl Drop for Root {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        } // only exact test-owned synthetic root, NEVER operator state
        fn args(path: &Path, cap: &str, scenario: &str) -> Vec<OsString> {
            vec![
                "--synthetic-accounting".into(),
                "--state-dir".into(),
                path.into(),
                "--synthetic-cap-micro-usd".into(),
                cap.into(),
                "--scenario".into(),
                scenario.into(),
            ]
        }
        fn lines(bytes: &[u8]) -> Vec<Value> {
            std::str::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|line| {
                    assert!(line.len() <= 1024);
                    serde_json::from_str(line).unwrap()
                })
                .collect()
        }
        fn retained(path: &Path, scenario: Scenario) {
            let c = read_only_connection(&path.join("state.db")).unwrap();
            let raw: String = c
                .query_row("SELECT record FROM budget_reservation", [], |r| r.get(0))
                .unwrap();
            let b: m8sd::budget::BudgetReservation = serde_json::from_str(&raw).unwrap();
            assert_eq!(b.state, BudgetState::Uncertain);
            assert!(b.reserved_all_in > 0);
            assert!(b.settlement.is_none());
            let raw: String = c
                .query_row("SELECT record FROM create_operation", [], |r| r.get(0))
                .unwrap();
            let op: m8sd::operation::CreateOperation = serde_json::from_str(&raw).unwrap();
            assert!(op.cleanup_required);
            assert!(op.dispatched_at.is_some());
            assert!(op.invocation_started_at.is_some());
            assert_eq!(
                op.state,
                if scenario == Scenario::Cancel {
                    CreateState::Cancelled
                } else {
                    CreateState::DispatchConsumed
                }
            );
            let journal: Value =
                serde_json::from_slice(&fs::read(path.join("local-journal/state.json")).unwrap())
                    .unwrap();
            let records = journal["records"].as_object().unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records.values().next().unwrap()["status"], "Absent");
        }
        #[test]
        fn parser_refuses_missing_duplicate_unknown_bad_cap_and_scenario_without_effects() {
            let root = Root::new();
            let path = root.state();
            let good = args(&path, "10000", "cancel");
            for index in [0, 3, 5] {
                let mut bad = good.clone();
                if index == 0 {
                    bad.remove(0);
                } else {
                    bad.drain(index..index + 2);
                }
                assert!(parse(bad).is_err());
            }
            for cap in [
                "0",
                "-1",
                "+1",
                "1.0",
                "1000001",
                "18446744073709551616",
                " 1",
                "",
            ] {
                assert!(parse(args(&path, cap, "cancel")).is_err());
            }
            assert!(parse(args(&path, "10000", "paid")).is_err());
            let mut bad = good.clone();
            bad.push("--force".into());
            assert!(parse(bad).is_err());
            let mut bad = good;
            bad.push("--synthetic-accounting".into());
            assert!(parse(bad).is_err());
            assert!(parse(args(Path::new("relative"), "10000", "cancel")).is_err());
            assert!(!path.exists());
        }
        #[test]
        fn cancel_real_local_lifecycle_retains_unknown_hold_and_refuses_rerun_without_mutation() {
            let root = Root::new();
            let path = root.state();
            let mut out = Vec::new();
            run(parse(args(&path, "10000", "cancel")).unwrap(), &mut out).unwrap();
            let values = lines(&out);
            let last = values.last().unwrap();
            assert_eq!(last["journal_cleanup"], "LocalOnlyAbsent");
            assert_eq!(last["billing"], "unknown");
            assert_eq!(last["budget_state"], "Uncertain");
            assert_eq!(last["reserved_hold_retained"], true);
            retained(&path, Scenario::Cancel);
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o700);
            assert_eq!(
                fs::metadata(path.join("demo.lock")).unwrap().mode() & 0o7777,
                0o600
            );
            let db = fs::read(path.join("state.db")).unwrap();
            let journal = fs::read(path.join("local-journal/state.json")).unwrap();
            assert_eq!(
                run(
                    parse(args(&path, "10000", "cancel")).unwrap(),
                    &mut Vec::new()
                ),
                Err(Error::ExistingState)
            );
            assert_eq!(fs::read(path.join("state.db")).unwrap(), db);
            assert_eq!(
                fs::read(path.join("local-journal/state.json")).unwrap(),
                journal
            );
            let text = std::str::from_utf8(&out).unwrap();
            for forbidden in ["budget_refunded", "spent", "proof", "Ready"] {
                assert!(!text.contains(forbidden));
            }
        }
        #[test]
        fn deadline_real_worker_uses_original_window_then_explicit_cleanup_with_hold() {
            let root = Root::new();
            let path = root.state();
            let mut out = Vec::new();
            run(parse(args(&path, "10000", "deadline")).unwrap(), &mut out).unwrap();
            let values = lines(&out);
            assert!(
                values
                    .iter()
                    .any(|v| v["phase"] == "deadline_stop_observed")
            );
            retained(&path, Scenario::Deadline);
            let c = read_only_connection(&path.join("state.db")).unwrap();
            let raw: String = c
                .query_row("SELECT record FROM budget_reservation", [], |r| r.get(0))
                .unwrap();
            let b: m8sd::budget::BudgetReservation = serde_json::from_str(&raw).unwrap();
            assert_eq!(b.request.quote.runtime_seconds, RUNTIME);
            assert_eq!(b.request.quote.cleanup_seconds, GRACE);
        }
        #[test]
        fn tiny_synthetic_cap_refuses_before_local_submission_and_keeps_state() {
            let root = Root::new();
            let path = root.state();
            assert_eq!(
                run(parse(args(&path, "1", "cancel")).unwrap(), &mut Vec::new()),
                Err(Error::SyntheticCapRefused)
            );
            assert!(path.join("state.db").is_file());
            let journal: Value =
                serde_json::from_slice(&fs::read(path.join("local-journal/state.json")).unwrap())
                    .unwrap();
            assert!(journal["records"].as_object().unwrap().is_empty());
            let c = read_only_connection(&path.join("state.db")).unwrap();
            let n: i64 = c
                .query_row(
                    "SELECT (SELECT count(*) FROM gpu_node)+(SELECT count(*) FROM launch_attempt)",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 0);
            let c = read_only_connection(&path.join("state.db")).unwrap();
            let n: i64 = c
                .query_row("SELECT count(*) FROM create_operation", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0);
            assert_eq!(
                run(
                    parse(args(&path, "10000", "cancel")).unwrap(),
                    &mut Vec::new()
                ),
                Err(Error::ExistingState)
            );
        }
        #[test]
        fn existing_symlink_and_noncanonical_paths_never_reset_or_follow_targets() {
            let root = Root::new();
            let target = root.0.join("existing");
            fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
            fs::write(target.join("canary"), b"synthetic-canary").unwrap();
            let link = root.state();
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert_eq!(
                run(
                    parse(args(&link, "10000", "cancel")).unwrap(),
                    &mut Vec::new()
                ),
                Err(Error::ExistingState)
            );
            assert_eq!(
                fs::read(target.join("canary")).unwrap(),
                b"synthetic-canary"
            );
            assert!(!target.join("state.db").exists());
            assert!(parse(args(&root.0.join("a/../b"), "10000", "cancel")).is_err());
        }
        #[test]
        fn fresh_lock_is_exclusive_and_symlinked_parent_refuses_before_creation() {
            let root = Root::new();
            let path = root.state();
            let state = FreshState::create(&path).unwrap();
            let second = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path.join("demo.lock"))
                .unwrap();
            assert!(second.try_lock_exclusive().is_err());
            state.recheck().unwrap();
            let target = root.0.join("real-parent");
            fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
            let alias = root.0.join("alias-parent");
            std::os::unix::fs::symlink(&target, &alias).unwrap();
            let opts = parse(args(&alias.join("new-state"), "10000", "cancel")).unwrap();
            assert_eq!(run(opts, &mut Vec::new()), Err(Error::PrivateState));
            assert!(!target.join("new-state").exists());
        }
        struct FailAfterSpawn {
            bytes: Vec<u8>,
        }
        impl Write for FailAfterSpawn {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                if b.windows(b"local_child_spawned".len())
                    .any(|v| v == b"local_child_spawned")
                {
                    return Err(std::io::Error::other("synthetic-output-failure"));
                }
                self.bytes.extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        #[test]
        fn output_error_after_spawn_stops_owned_child_and_preserves_uncertain_nonzero_outcome() {
            let root = Root::new();
            let path = root.state();
            let mut writer = FailAfterSpawn { bytes: Vec::new() };
            assert_eq!(
                run(parse(args(&path, "10000", "cancel")).unwrap(), &mut writer),
                Err(Error::Output)
            );
            let c = read_only_connection(&path.join("state.db")).unwrap();
            let raw: String = c
                .query_row("SELECT record FROM budget_reservation", [], |r| r.get(0))
                .unwrap();
            let b: m8sd::budget::BudgetReservation = serde_json::from_str(&raw).unwrap();
            assert_eq!(b.state, BudgetState::Uncertain);
            assert!(b.settlement.is_none());
            let raw: String = c
                .query_row("SELECT record FROM create_operation", [], |r| r.get(0))
                .unwrap();
            let operation: m8sd::operation::CreateOperation = serde_json::from_str(&raw).unwrap();
            assert!(operation.dispatched_at.is_some());
            assert!(operation.invocation_started_at.is_some());
            assert!(operation.cleanup_required);
            let journal: Value =
                serde_json::from_slice(&fs::read(path.join("local-journal/state.json")).unwrap())
                    .unwrap();
            assert_eq!(
                journal["records"]
                    .as_object()
                    .unwrap()
                    .values()
                    .next()
                    .unwrap()["status"],
                "Absent"
            );
            assert!(
                lines(&writer.bytes)
                    .iter()
                    .any(|v| v["phase"] == "error" && v["state_preserved"] == true)
            );
        }
    }
}
#[cfg(unix)]
fn main() {
    std::process::exit(demo::main());
}
#[cfg(not(unix))]
fn main() {
    eprintln!("local lifecycle demo unavailable on this platform");
    std::process::exit(1);
}
