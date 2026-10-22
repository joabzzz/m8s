//! Explicit harmless LOCAL walkthrough. Synthetic accounting is NOT approval.
//! No paid provider, credentials, model, admission, readiness or billing proof.
use crate::{
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
use fs2::FileExt;
use m8s_provider_local::{DevelopmentCommand, LocalTerminationState};
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
            crate::Error::Invalid => Error::SyntheticCapRefused,
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

/// Runs the explicit synthetic local walkthrough; arguments exclude the command name.
pub fn run_cli(args: Vec<OsString>, out: &mut impl Write) -> i32 {
    match parse(args).and_then(|opts| run(opts, out)) {
        Ok(()) => 0,
        Err(e) => {
            let _ = event(
                out,
                json!({"phase":"failure","error":e.code(),"accounting":"synthetic-not-approval","no_reset":true}),
            );
            1
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InspectError {
    Arguments,
    Busy,
    Unavailable,
}
impl From<Error> for InspectError {
    fn from(_: Error) -> Self {
        Self::Unavailable
    }
}
type InspectResult<T> = std::result::Result<T, InspectError>;

// Historical inspection deliberately does not use Store, Local or the journal.
// Advisory locking coordinates cooperating demos, not hostile same-UID writers.
struct InspectState {
    anchor: FreshState,
    db: File,
}
fn inspect_file(path: &Path) -> InspectResult<File> {
    inspect_open(path, false)
}
fn inspect_open(path: &Path, is_directory: bool) -> InspectResult<File> {
    let flags = libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | libc::O_CLOEXEC
        | if is_directory { libc::O_DIRECTORY } else { 0 };
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)
        .map_err(|_| InspectError::Unavailable)?;
    private(&f, is_directory)?;
    Ok(f)
}
impl InspectState {
    fn open(path: &Path) -> InspectResult<Self> {
        if fs::canonicalize(path).map_err(|_| InspectError::Unavailable)? != path {
            return Err(InspectError::Unavailable);
        }
        let dir = inspect_open(path, true)?;
        let lock = inspect_file(&path.join("demo.lock"))?;
        FileExt::try_lock_shared(&lock).map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                InspectError::Busy
            } else {
                InspectError::Unavailable
            }
        })?;
        let db = inspect_file(&path.join("state.db"))?;
        let state = Self {
            anchor: FreshState {
                path: path.into(),
                lock,
                dir,
            },
            db,
        };
        state.recheck()?;
        Ok(state)
    }
    fn recheck(&self) -> InspectResult<()> {
        private(&self.anchor.dir, true)?;
        private(&self.anchor.lock, false)?;
        let current_dir = inspect_open(&self.anchor.path, true)?;
        let current_lock = inspect_file(&self.anchor.path.join("demo.lock"))?;
        for (retained, current) in [
            (&self.anchor.dir, &current_dir),
            (&self.anchor.lock, &current_lock),
        ] {
            let a = retained.metadata().map_err(|_| InspectError::Unavailable)?;
            let b = current.metadata().map_err(|_| InspectError::Unavailable)?;
            if (a.dev(), a.ino()) != (b.dev(), b.ino()) {
                return Err(InspectError::Unavailable);
            }
        }
        if fs::canonicalize(&self.anchor.path).map_err(|_| InspectError::Unavailable)?
            != self.anchor.path
        {
            return Err(InspectError::Unavailable);
        }
        private(&self.db, false)?;
        let current = inspect_file(&self.anchor.path.join("state.db"))?;
        let a = self.db.metadata().map_err(|_| InspectError::Unavailable)?;
        let b = current.metadata().map_err(|_| InspectError::Unavailable)?;
        if (a.dev(), a.ino()) != (b.dev(), b.ino()) || a.len() == 0 || a.len() > 16 * 1024 * 1024 {
            return Err(InspectError::Unavailable);
        }
        for suffix in ["state.db-wal", "state.db-shm", "state.db-journal"] {
            match fs::symlink_metadata(self.anchor.path.join(suffix)) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                _ => return Err(InspectError::Unavailable),
            }
        }
        Ok(())
    }
}
fn inspect_connection(path: &Path) -> InspectResult<Connection> {
    // Encode every path byte, including URI metacharacters; no URI injection.
    let mut uri = String::from("file:");
    for byte in path.as_os_str().as_bytes() {
        use std::fmt::Write;
        write!(&mut uri, "%{byte:02X}").map_err(|_| InspectError::Unavailable)?;
    }
    uri.push_str("?immutable=1&mode=ro");
    let c = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| InspectError::Unavailable)?;
    c.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")
        .map_err(|_| InspectError::Unavailable)?;
    Ok(c)
}
fn inspect_record<T: serde::de::DeserializeOwned>(c: &Connection, table: &str) -> InspectResult<T> {
    // Only internal fixed table names. Bound rows and byte lengths BEFORE decode.
    let sql = format!(
        "SELECT CASE WHEN typeof(record)='text' AND length(CAST(record AS BLOB)) BETWEEN 1 AND 65536 THEN record ELSE NULL END FROM {table} LIMIT 2"
    );
    let mut stmt = c.prepare(&sql).map_err(|_| InspectError::Unavailable)?;
    let mut rows = stmt.query([]).map_err(|_| InspectError::Unavailable)?;
    let raw: String = rows
        .next()
        .map_err(|_| InspectError::Unavailable)?
        .ok_or(InspectError::Unavailable)?
        .get(0)
        .map_err(|_| InspectError::Unavailable)?;
    if rows
        .next()
        .map_err(|_| InspectError::Unavailable)?
        .is_some()
    {
        return Err(InspectError::Unavailable);
    }
    serde_json::from_str(&raw).map_err(|_| InspectError::Unavailable)
}
fn inspect_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}
fn inspect_snapshot(path: &Path) -> InspectResult<(Value, InspectState)> {
    use crate::{
        budget::{BudgetReservation, VolumeBinding},
        operation::CreateOperation,
    };
    let state = InspectState::open(path)?;
    let c = inspect_connection(&path.join("state.db"))?;
    let owned: bool = c
        .query_row(
            "SELECT count(*)=1 AND min(owner)=? FROM (SELECT owner FROM pool LIMIT 2)",
            [OWNER],
            |r| r.get(0),
        )
        .map_err(|_| InspectError::Unavailable)?;
    if !owned {
        return Err(InspectError::Unavailable);
    }
    let b: BudgetReservation = inspect_record(&c, "budget_reservation")?;
    let op: CreateOperation = inspect_record(&c, "create_operation")?;
    let run = format!("{LABEL}-run");
    // Recorded identity is bounded metadata, not a recomputed identity proof.
    // Check only public cross-record consistency; the stored fingerprint and
    // private operation budget are not inspection authority.
    let node = op.intent.node_uid.as_str();
    if !inspect_label(node) {
        return Err(InspectError::Unavailable);
    }
    let (_, period, request) = fixtures(MAX_CAP, b.reserved_at, node)?;
    let binding = b.binding.as_ref().ok_or(InspectError::Unavailable)?;
    if op.uncertainty_observed
        || b.reserved_at < 0
        || op.prepared_at < b.reserved_at
        || op.last_at < op.prepared_at
        || op
            .dispatched_at
            .is_some_and(|at| at < op.prepared_at || at > op.last_at)
        || op
            .invocation_started_at
            .zip(op.dispatched_at)
            .is_some_and(|(at, start)| at < start || at > op.last_at)
        || op
            .cancelled_at
            .is_some_and(|at| at < op.invocation_started_at.unwrap_or(i64::MAX) || at > op.last_at)
        || binding.observed_at < op.invocation_started_at.unwrap_or(i64::MAX)
        || binding.observed_at > op.last_at
        || b.request != request
        || b.period != period
        || b.state != BudgetState::Uncertain
        || b.reserved_all_in != 30
        || b.reserved_hourly != 36000
        || b.settlement.is_some()
        || b.billing_period.is_some()
        || binding.owner != OWNER
        || binding.run_id != run
        || binding.node_uid != node
        || !inspect_label(&binding.instance_id)
        || binding.provenance
            != "local-development-v1: retained child; local adapter creates no volumes; not cloud proof"
        || binding.volumes != VolumeBinding::NoneAllocated
        || binding.status != EvidenceStatus::Complete
        || op.intent.owner != OWNER
        || op.intent.run_id != run
        || op.intent.request_id != REQUEST
        || op.intent.operation_id != OP
        || op.dispatched_at.is_none()
        || op.invocation_started_at.is_none()
        || !op.cleanup_required
        || op.result.as_ref() != Some(binding)
        || !matches!(
            op.state,
            CreateState::Cancelled | CreateState::DispatchConsumed
        )
        || (op.state == CreateState::Cancelled) != op.cancelled_at.is_some()
    {
        return Err(InspectError::Unavailable);
    }
    let consistent: bool = c.query_row("SELECT
        (SELECT count(*)=1 AND min(owner)=?1 AND min(request_id)=?2 AND min(node_uid)=?3 FROM (SELECT owner,request_id,node_uid FROM budget_reservation LIMIT 2)) AND
        (SELECT count(*)=1 AND min(owner)=?1 AND min(request_id)=?2 AND min(node_uid)=?3 AND min(operation_id)=?4 FROM (SELECT owner,request_id,node_uid,operation_id FROM create_operation LIMIT 2)) AND
        (SELECT count(*)=1 AND min(uid)=?3 AND min(tier)=?5 AND min(phase)='\"Launching\"' FROM (SELECT uid,tier,phase FROM gpu_node LIMIT 2)) AND
        (SELECT count(*)=1 AND min(uid)=?3 AND min(tier)=?5 FROM (SELECT uid,tier FROM launch_attempt LIMIT 2))",
        params![OWNER, REQUEST, node, OP, TIER], |r| r.get(0)).map_err(|_| InspectError::Unavailable)?;
    if !consistent {
        return Err(InspectError::Unavailable);
    }
    drop(c);
    state.recheck()?;
    Ok((
        json!({"version":"m8s.local-inspect/v1","phase":"snapshot","historical":true,
        "accounting":"synthetic-not-approval","billing":"unknown","readiness":"not-assessed",
        "child_state":"not-checked","budget_state":b.state,"simulated_hold_micro_usd":b.reserved_all_in,
        "reserved_hold_retained":true,"operation_state":op.state,"claim_state":"consumed","cleanup_required":true}),
        state,
    ))
}
/// Read one completed, quiescent synthetic demo snapshot, without opening Store
/// or its journal. Arguments exclude the command name; errors never echo input.
pub fn inspect_cli(args: Vec<OsString>, out: &mut impl Write) -> i32 {
    if args == [OsString::from("--help")] {
        return if out.write_all(b"usage: m8s local-inspect --state-dir CANONICAL_ABSOLUTE_PATH\nUnix only; build with --features local-development; historical synthetic state only.\n").is_ok() { 0 } else { 1 };
    }
    let parsed = if args.len() == 2 && args[0] == "--state-dir" {
        args[1].to_str().map(Path::new).filter(|p| path_valid(p))
    } else {
        None
    };
    let result = parsed
        .ok_or(InspectError::Arguments)
        .and_then(inspect_snapshot);
    match result {
        Ok((value, state)) => {
            if state.recheck().is_err() {
                let _ = event(
                    out,
                    json!({"version":"m8s.local-inspect/v1","phase":"failure","error":"snapshot-unavailable"}),
                );
                return 1;
            }
            if event(out, value).is_ok() { 0 } else { 1 }
        }
        Err(e) => {
            let error = match e {
                InspectError::Arguments => "invalid-arguments",
                InspectError::Busy => "state-busy",
                InspectError::Unavailable => "snapshot-unavailable",
            };
            let _ = event(
                out,
                json!({"version":"m8s.local-inspect/v1","phase":"failure","error":error}),
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
        let b: crate::budget::BudgetReservation = serde_json::from_str(&raw).unwrap();
        assert_eq!(b.state, BudgetState::Uncertain);
        assert!(b.reserved_all_in > 0);
        assert!(b.settlement.is_none());
        let raw: String = c
            .query_row("SELECT record FROM create_operation", [], |r| r.get(0))
            .unwrap();
        let op: crate::operation::CreateOperation = serde_json::from_str(&raw).unwrap();
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
        let b: crate::budget::BudgetReservation = serde_json::from_str(&raw).unwrap();
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
        let b: crate::budget::BudgetReservation = serde_json::from_str(&raw).unwrap();
        assert_eq!(b.state, BudgetState::Uncertain);
        assert!(b.settlement.is_none());
        let raw: String = c
            .query_row("SELECT record FROM create_operation", [], |r| r.get(0))
            .unwrap();
        let operation: crate::operation::CreateOperation = serde_json::from_str(&raw).unwrap();
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

#[cfg(test)]
mod inspect_tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "m8s-inspect-synthetic-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            for name in ["demo.lock", "state.db"] {
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path.join(name))
                    .unwrap();
                f.write_all(b"synthetic-invalid-db").unwrap();
            }
            Self(path)
        }
        fn refused(&self) {
            self.refused_error("snapshot-unavailable");
        }
        fn refused_error(&self, expected: &str) {
            let before = inventory(&self.0);
            let mut out = Vec::new();
            assert_eq!(
                inspect_cli(vec!["--state-dir".into(), self.0.clone().into()], &mut out),
                1
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&out).unwrap()["error"],
                expected
            );
            assert!(!String::from_utf8_lossy(&out).contains(self.0.to_str().unwrap()));
            assert_eq!(inventory(&self.0), before);
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn inventory(path: &Path) -> Vec<(OsString, u32, u64, Vec<u8>)> {
        let mut entries = Vec::new();
        for item in fs::read_dir(path).unwrap() {
            let item = item.unwrap();
            let m = item.path().symlink_metadata().unwrap();
            entries.push((
                item.file_name(),
                m.mode(),
                m.nlink(),
                if m.is_file() {
                    fs::read(item.path()).unwrap()
                } else {
                    Vec::new()
                },
            ));
        }
        entries.sort();
        entries
    }
    #[test]
    fn inspect_corrupt_missing_unsafe_symlink_hardlink_and_sidecars_never_mutate() {
        let f = Fixture::new();
        f.refused();
        for name in ["state.db-wal", "state.db-shm", "state.db-journal"] {
            symlink("missing-synthetic-target", f.0.join(name)).unwrap();
            f.refused();
            fs::remove_file(f.0.join(name)).unwrap();
            fs::write(f.0.join(name), b"synthetic-sidecar-sentinel").unwrap();
            f.refused();
            fs::remove_file(f.0.join(name)).unwrap();
        }
        for name in ["state.db", "demo.lock"] {
            let path = f.0.join(name);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            f.refused();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            fs::rename(&path, f.0.join("retained")).unwrap();
            symlink("retained", &path).unwrap();
            f.refused();
            fs::remove_file(&path).unwrap();
            fs::hard_link(f.0.join("retained"), &path).unwrap();
            f.refused();
            fs::remove_file(&path).unwrap();
            f.refused();
            fs::rename(f.0.join("retained"), &path).unwrap();
        }
        fs::set_permissions(&f.0, fs::Permissions::from_mode(0o750)).unwrap();
        f.refused();
        fs::set_permissions(&f.0, fs::Permissions::from_mode(0o700)).unwrap();
        let alias = f.0.join("alias");
        symlink(&f.0, &alias).unwrap();
        let mut out = Vec::new();
        assert_eq!(
            inspect_cli(vec!["--state-dir".into(), alias.into()], &mut out),
            1
        );
    }
    #[test]
    fn inspect_active_exclusive_lock_and_anchor_replacement_refuse() {
        let f = Fixture::new();
        let lock = inspect_file(&f.0.join("demo.lock")).unwrap();
        lock.try_lock_exclusive().unwrap();
        f.refused_error("state-busy");
        FileExt::unlock(&lock).unwrap();
        for name in ["state.db", "demo.lock"] {
            let state = InspectState::open(&f.0).unwrap();
            fs::rename(f.0.join(name), f.0.join("original")).unwrap();
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(f.0.join(name))
                .unwrap()
                .write_all(b"replacement")
                .unwrap();
            assert!(state.recheck().is_err());
            drop(state);
            fs::remove_file(f.0.join(name)).unwrap();
            fs::rename(f.0.join("original"), f.0.join(name)).unwrap();
        }
        let state = InspectState::open(&f.0).unwrap();
        let moved = f.0.with_extension("moved");
        fs::rename(&f.0, &moved).unwrap();
        fs::DirBuilder::new().mode(0o700).create(&f.0).unwrap();
        assert!(state.recheck().is_err());
        fs::remove_dir(&f.0).unwrap();
        fs::rename(moved, &f.0).unwrap();
    }
    #[test]
    fn inspect_owner_gate_and_bounded_records_refuse_opaque_without_sidecars() {
        let f = Fixture::new();
        fs::remove_file(f.0.join("state.db")).unwrap();
        let c = Connection::open(f.0.join("state.db")).unwrap();
        c.execute_batch("CREATE TABLE pool(owner TEXT); CREATE TABLE budget_reservation(record TEXT); CREATE TABLE create_operation(record TEXT);").unwrap();
        c.execute("INSERT INTO pool VALUES (?)", ["foreign-sensitive-owner"])
            .unwrap();
        drop(c);
        fs::set_permissions(f.0.join("state.db"), fs::Permissions::from_mode(0o600)).unwrap();
        f.refused();
        let c = Connection::open(f.0.join("state.db")).unwrap();
        c.execute("UPDATE pool SET owner=?", [OWNER]).unwrap();
        drop(c);
        f.refused(); // missing rows
        for raw in ["{}".to_owned(), "x".repeat(65537)] {
            let c = Connection::open(f.0.join("state.db")).unwrap();
            c.execute("DELETE FROM budget_reservation", []).unwrap();
            c.execute("INSERT INTO budget_reservation VALUES (?)", [&raw])
                .unwrap();
            drop(c);
            f.refused();
        }
        let c = Connection::open(f.0.join("state.db")).unwrap();
        c.execute("INSERT INTO budget_reservation VALUES ('{}')", [])
            .unwrap();
        drop(c);
        f.refused();
    }
    #[test]
    fn inspect_immutable_uri_encodes_metacharacters_and_cannot_write_or_create_sidecars() {
        let mut f = Fixture::new();
        let encoded_path = f.0.with_extension("synthetic ?mode=rw#% snow-雪");
        fs::rename(&f.0, &encoded_path).unwrap();
        f.0 = encoded_path;
        fs::remove_file(f.0.join("state.db")).unwrap();
        let c = Connection::open(f.0.join("state.db")).unwrap();
        c.execute_batch("CREATE TABLE sentinel(value INTEGER); INSERT INTO sentinel VALUES (7);")
            .unwrap();
        drop(c);
        fs::set_permissions(f.0.join("state.db"), fs::Permissions::from_mode(0o600)).unwrap();
        let before = inventory(&f.0);
        let state = InspectState::open(&f.0).unwrap();
        let c = inspect_connection(&f.0.join("state.db")).unwrap();
        assert_eq!(
            c.query_row("SELECT value FROM sentinel", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            7
        );
        assert!(c.execute("INSERT INTO sentinel VALUES (8)", []).is_err());
        drop(c);
        state.recheck().unwrap();
        assert_eq!(inventory(&f.0), before);
    }
    // Synthetic historical metadata only: no child, Local, Executor or command.
    fn consistent_fixture() -> Fixture {
        use crate::{
            budget::{ResourceBinding, VolumeBinding},
            operation::CreateIntent,
        };
        let f = Fixture::new();
        let db = f.0.join("state.db");
        fs::remove_file(&db).unwrap();
        let store = Store::open(&db, OWNER).unwrap();
        store.scale(TIER, 1).unwrap();
        let node = "synthetic-recorded-node"; // deliberately no derived-ID format
        let (policy, period, request) = fixtures(10000, 100, node).unwrap();
        let mut b = store
            .reserve_budget(&policy, &period, &request, 100)
            .unwrap();
        let c = Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES (?,?,'\"Launching\"',100)",
            params![node, TIER],
        )
        .unwrap();
        c.execute(
            "INSERT INTO launch_attempt(uid,tier,at) VALUES (?,?,100)",
            params![node, TIER],
        )
        .unwrap();
        drop(c);
        let intent = CreateIntent {
            owner: OWNER.into(),
            run_id: format!("{LABEL}-run"),
            request_id: REQUEST.into(),
            node_uid: node.into(),
            operation_id: OP.into(),
            launch_fingerprint: [0; 32],
        };
        // The public prepare result supplies its own private internals; tests
        // alter only public historical fields, never inspect private payloads.
        let mut op = store.prepare_create(&intent, 100).unwrap();
        let binding = ResourceBinding { owner: OWNER.into(), run_id: intent.run_id.clone(),
            node_uid: node.into(), instance_id: "synthetic-recorded-instance".into(),
            volumes: VolumeBinding::NoneAllocated,
            provenance: "local-development-v1: retained child; local adapter creates no volumes; not cloud proof".into(),
            observed_at: 100, status: EvidenceStatus::Complete };
        b.state = BudgetState::Uncertain;
        b.binding = Some(binding.clone());
        op.state = CreateState::DispatchConsumed;
        op.dispatched_at = Some(100);
        op.invocation_started_at = Some(100);
        op.cleanup_required = true;
        op.result = Some(binding);
        drop(store);
        let c = Connection::open(db).unwrap();
        c.execute(
            "UPDATE budget_reservation SET record=?",
            [serde_json::to_string(&b).unwrap()],
        )
        .unwrap();
        c.execute(
            "UPDATE create_operation SET record=?",
            [serde_json::to_string(&op).unwrap()],
        )
        .unwrap();
        drop(c);
        f
    }
    #[test]
    fn inspect_checks_recorded_uid_consistency_not_fingerprint_authority() {
        use crate::{budget::BudgetReservation, operation::CreateOperation};
        let f = consistent_fixture();
        let read = || {
            let before = inventory(&f.0);
            let mut out = Vec::new();
            assert_eq!(
                inspect_cli(vec!["--state-dir".into(), f.0.clone().into()], &mut out),
                0
            );
            assert_eq!(inventory(&f.0), before);
            let v: Value = serde_json::from_slice(&out).unwrap();
            assert_eq!(v["historical"], true);
            assert_eq!(v["accounting"], "synthetic-not-approval");
            assert_eq!(v["readiness"], "not-assessed");
            assert!(v.get("launch_fingerprint").is_none());
            out
        };
        let original_output = read();
        let c = Connection::open(f.0.join("state.db")).unwrap();
        let raw: String = c
            .query_row("SELECT record FROM create_operation", [], |r| r.get(0))
            .unwrap();
        let mut op: CreateOperation = serde_json::from_str(&raw).unwrap();
        op.intent.launch_fingerprint = [255; 32];
        c.execute(
            "UPDATE create_operation SET record=?",
            [serde_json::to_string(&op).unwrap()],
        )
        .unwrap();
        drop(c);
        assert_eq!(read(), original_output); // fingerprint is stored, unverified metadata
        for table in [
            "budget_reservation",
            "create_operation",
            "gpu_node",
            "launch_attempt",
        ] {
            let c = Connection::open(f.0.join("state.db")).unwrap();
            let column = if table == "gpu_node" || table == "launch_attempt" {
                "uid"
            } else {
                "node_uid"
            };
            c.execute(
                &format!("UPDATE {table} SET {column}='synthetic-mismatch'"),
                [],
            )
            .unwrap();
            drop(c);
            f.refused();
            let c = Connection::open(f.0.join("state.db")).unwrap();
            c.execute(
                &format!("UPDATE {table} SET {column}='synthetic-recorded-node'"),
                [],
            )
            .unwrap();
        }
        let c = Connection::open(f.0.join("state.db")).unwrap();
        let raw: String = c
            .query_row("SELECT record FROM budget_reservation", [], |r| r.get(0))
            .unwrap();
        let original: BudgetReservation = serde_json::from_str(&raw).unwrap();
        for change_request in [true, false] {
            let mut b = original.clone();
            if change_request {
                b.request.node_uid = "synthetic-mismatch".into();
            } else {
                b.binding.as_mut().unwrap().node_uid = "synthetic-mismatch".into();
            }
            c.execute(
                "UPDATE budget_reservation SET record=?",
                [serde_json::to_string(&b).unwrap()],
            )
            .unwrap();
            f.refused();
        }
        c.execute("UPDATE budget_reservation SET record=?", [raw])
            .unwrap();
        op.intent.node_uid = "synthetic-mismatch".into();
        c.execute(
            "UPDATE create_operation SET record=?",
            [serde_json::to_string(&op).unwrap()],
        )
        .unwrap();
        drop(c);
        f.refused();
    }
    #[test]
    fn inspect_parser_rejects_non_utf8_and_help_has_no_state_effect() {
        use std::os::unix::ffi::OsStringExt;
        let mut out = Vec::new();
        assert_eq!(
            inspect_cli(
                vec!["--state-dir".into(), OsString::from_vec(vec![b'/', 255])],
                &mut out
            ),
            1
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap()["error"],
            "invalid-arguments"
        );
        assert_eq!(inspect_cli(vec!["--help".into()], &mut Vec::new()), 0);
    }
}
