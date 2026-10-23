//! Explicit real LOCAL developer child only. No cloud, admission, billing or
//! readiness proof; no main/reconcile wiring. Trusted executable/UTC/ancestors.
use super::*;
use m8s_provider::LaunchSpec;
use m8s_provider::local::{
    DevelopmentCommand, Local, LocalDeadlines, LocalLaunch, LocalStopHandle,
};
use sha2::{Digest, Sha256};
use std::{
    os::unix::ffi::OsStrExt,
    path::{Component, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Ordinary bounded request metadata, NOT approval, UID override or claim.
/// ```compile_fail
/// use m8sd::operation::local_development::LocalLaunchRequest;
/// let _ = LocalLaunchRequest { run_id: "r".into(), request_id: "q".into(),
///     operation_id: "op".into(), node_uid: "override".into() };
/// ```
#[derive(Clone)]
pub struct LocalLaunchRequest {
    pub run_id: String,
    pub request_id: String,
    pub operation_id: String,
}
struct Identity {
    journal: PathBuf,
    namespace: String,
    target: String,
    tier: String,
    command: [u8; 32],
}
impl Identity {
    fn fingerprint(&self, intent: &CreateIntent) -> Result<[u8; 32]> {
        let mut hash = Sha256::new();
        hash.update(b"m8s-local-development-plan\0");
        hash.update(1u16.to_be_bytes());
        for text in [
            &intent.owner,
            &intent.run_id,
            &intent.request_id,
            &intent.node_uid,
            &intent.operation_id,
        ] {
            label(text)?;
        }
        for bytes in [
            b"LocalDevelopmentV1".as_slice(),
            self.command.as_slice(),
            self.journal.as_os_str().as_bytes(),
            self.namespace.as_bytes(),
            self.target.as_bytes(),
            self.tier.as_bytes(),
            intent.owner.as_bytes(),
            intent.run_id.as_bytes(),
            intent.request_id.as_bytes(),
            intent.node_uid.as_bytes(),
            intent.operation_id.as_bytes(),
        ] {
            hash.update((bytes.len() as u32).to_be_bytes());
            hash.update(bytes);
        }
        Ok(hash.finalize().into())
    }
}
/// Fixed LocalDevelopmentV1, owned command and immutable nonsecret target.
/// Digest binds raw Unix command/path configuration, NOT executable contents,
/// filesystem immutability, sandboxing, approval or provider identity.
pub struct LocalDevelopmentPlan {
    command: DevelopmentCommand,
    identity: Identity,
}
impl LocalDevelopmentPlan {
    pub fn new(
        command: DevelopmentCommand,
        journal: PathBuf,
        namespace: &str,
        target: &str,
        tier: &str,
    ) -> Result<Self> {
        for text in [namespace, target, tier] {
            label(text)?;
            if text.len() > 64 {
                return Err(Error::Invalid);
            }
        }
        let raw = journal.as_os_str().as_bytes();
        let normalized: PathBuf = journal.components().collect();
        if !journal.is_absolute()
            || raw.len() > 4096
            || raw.contains(&0)
            || raw != normalized.as_os_str().as_bytes()
            || journal
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err(Error::Invalid);
        }
        let digest = command.fingerprint();
        Ok(Self {
            command,
            identity: Identity {
                journal,
                namespace: namespace.into(),
                target: target.into(),
                tier: tier.into(),
                command: digest,
            },
        })
    }
    pub fn fingerprint(&self, intent: &CreateIntent) -> Result<[u8; 32]> {
        self.identity.fingerprint(intent)
    }
}
const REQUESTED: u8 = 1;
const PUBLISHED: u8 = 2;
// BOTH operations use the SAME atomic modification order. For one request and
// one publication, whichever RMW is second necessarily sees the other's bit.
// Publication's Release follows OnceLock initialization; a requesting Acquire
// that sees PUBLISHED therefore also sees the actual handle. If request wins,
// publication sees REQUESTED and signals itself. No cross-variable store/load
// visibility assumption (or stress-test-as-proof) is needed.
fn mark_requested(state: &AtomicU8) -> bool {
    state.fetch_or(REQUESTED, Ordering::AcqRel) & PUBLISHED != 0
}
fn mark_published(state: &AtomicU8) -> bool {
    state.fetch_or(PUBLISHED, Ordering::AcqRel) & REQUESTED != 0
}
struct StopLatch {
    state: AtomicU8,
    actual: OnceLock<LocalStopHandle>,
}
/// Cloneable stop-only latch, available BEFORE execute can block on journal or
/// Store persistence. Before the actual Local handle is published (after startup
/// and active journal save), a request is ONLY latched, not a prompt signal.
/// The independent timed worker still progresses after spawn in that gap.
/// No launch/renewal/reap/absence capability. One executor,
/// one irreversible stop latch; never reset for another attempt.
#[derive(Clone)]
pub struct LocalDevelopmentStop(Arc<StopLatch>);
impl LocalDevelopmentStop {
    pub fn request_stop(&self) {
        if mark_requested(&self.0.state) {
            // PUBLISHED can only follow successful OnceLock initialization.
            self.0
                .actual
                .get()
                .expect("published local stop handle")
                .request_stop();
        }
    }
    fn requested(&self) -> bool {
        self.0.state.load(Ordering::Acquire) & REQUESTED != 0
    }
    fn attach(&self, actual: LocalStopHandle) -> Result<()> {
        // Private single-call executor: one successful publication only. Even
        // an internal duplicate must stop BOTH handles, never discard a newly
        // owned child silently or replace the original publication.
        if let Err(rejected) = self.0.actual.set(actual) {
            rejected.request_stop();
            self.request_stop();
            return Err(Error::Invalid);
        }
        if mark_published(&self.0.state) {
            self.0
                .actual
                .get()
                .expect("initialized local stop handle")
                .request_stop();
        }
        Ok(())
    }
}
pub enum LocalDevelopmentOutcome {
    /// Exact LOCAL process binding, cleanup metadata only; never Ready/absence.
    Bound(Box<CreateOperation>),
    Ambiguous,
    NotInvoked,
}
/// Ordinary LOCAL diagnostic only, never a settlement/absence-proof capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalCleanupStatus {
    NoLocalSubmission,
    NotRequested,
    Pending {
        direct_child: m8s_provider::local::LocalTerminationState,
    },
    LocalOnlyAbsent,
    Uncertain,
}
/// One allocation, at most one timed Local submission; no Provider/callback API.
/// The caller pairs durable Store::cancel_create with stop_handle.request_stop.
/// DB-only down/cancel is cleanup intent, NOT automatic wakeup/containment.
/// Drop requests stop. Parent loss/host loss/kernel stalls remain uncontained.
/// ```compile_fail
/// use m8sd::operation::local_development::LocalDevelopmentPlan;
/// let _ = LocalDevelopmentPlan { command: todo!(), identity: todo!() };
/// ```
pub struct LocalDevelopmentExecutor {
    identity: Identity,
    owner: String,
    token: Arc<DriverOrigin>,
    local: Option<Local>,
    launch: Option<Arc<LocalLaunch>>,
    stop: LocalDevelopmentStop,
    submitted: bool,
    executed_intent: Option<CreateIntent>,
    executed_instance: Option<m8s_core::Instance>,
    cleanup_requested: bool,
    #[cfg(test)]
    hooks: Hooks,
}
impl LocalDevelopmentExecutor {
    pub fn open(plan: LocalDevelopmentPlan, owner: &str) -> Result<Self> {
        label(owner)?;
        // Local validates private eUID/mode/descriptor/journal/lock policy.
        // This creates no child. Trusted ancestors/executable remain assumptions.
        let local = Local::open_development(&plan.identity.journal, owner.into(), plan.command)?;
        Ok(Self {
            identity: plan.identity,
            owner: owner.into(),
            token: Arc::new(DriverOrigin),
            local: Some(local),
            launch: None,
            stop: LocalDevelopmentStop(Arc::new(StopLatch {
                state: AtomicU8::new(0),
                actual: OnceLock::new(),
            })),
            submitted: false,
            executed_intent: None,
            executed_instance: None,
            cleanup_requested: false,
            #[cfg(test)]
            hooks: Hooks::default(),
        })
    }
    /// Pure preview only: no database/journal access, effects or permission.
    /// UID depends on owner/run/request, NEVER operationID or mutable target.
    pub fn planned_intent(
        &self,
        store: &Store,
        request: &LocalLaunchRequest,
    ) -> Result<CreateIntent> {
        if store.owner != self.owner {
            return Err(Error::OwnerMismatch);
        }
        for value in [
            &store.owner,
            &request.run_id,
            &request.request_id,
            &request.operation_id,
        ] {
            label(value)?;
        }
        let mut hash = Sha256::new();
        hash.update(b"m8s-managed-local-node\0");
        hash.update(1u16.to_be_bytes());
        for value in [&store.owner, &request.run_id, &request.request_id] {
            hash.update((value.len() as u32).to_be_bytes());
            hash.update(value.as_bytes());
        }
        let mut node_uid = String::from("local-dev-");
        for byte in hash.finalize() {
            use std::fmt::Write;
            write!(&mut node_uid, "{byte:02x}").map_err(|_| Error::Invalid)?;
        }
        let mut intent = CreateIntent {
            owner: store.owner.clone(),
            run_id: request.run_id.clone(),
            request_id: request.request_id.clone(),
            operation_id: request.operation_id.clone(),
            node_uid,
            launch_fingerprint: [0; 32],
        };
        intent.launch_fingerprint = self.identity.fingerprint(&intent)?;
        Ok(intent)
    }
    /// Authoritative same-Store node/attempt/Prepared metadata, NEVER a claim.
    /// Requires explicit already-persisted matching Reserved budget and desired.
    pub fn prepare_managed(
        &self,
        store: &Store,
        request: &LocalLaunchRequest,
    ) -> Result<CreateOperation> {
        self.prepare_managed_inner(store, request, InvocationClock::LocalUtc)
    }
    fn prepare_managed_inner(
        &self,
        store: &Store,
        request: &LocalLaunchRequest,
        clock: InvocationClock,
    ) -> Result<CreateOperation> {
        let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
        let now = match clock {
            InvocationClock::Explicit(now) => now, // private deterministic tests only here
            InvocationClock::LocalUtc => utc_seconds()?, // AFTER writer acquired
        };
        owned(&tx, &store.owner)?;
        let intent = self.planned_intent(store, request)?; // recompute INSIDE owner TX
        let a = account(&tx, &store.owner, now)?;
        if let Some(existing) = get(&tx, &store.owner, &intent.operation_id)? {
            if existing.intent != intent || now < existing.last_at {
                return Err(Error::Invalid);
            }
            // Metadata only, including cancelled/dispatched/stopped/reopened.
            // No repair of missing history, new slot, clock renewal or permit.
            return Ok(existing);
        }
        self.available(store, &intent)?;
        let request_used: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM create_operation WHERE owner=? AND request_id=?)",
            params![store.owner, intent.request_id],
            |r| r.get(0),
        )?;
        if request_used {
            return Err(Error::Invalid);
        }
        let b = budget(&tx, &intent)?;
        crate::budget::validate_create_dispatch(&tx, &b, &a, now)?;
        let desired: Option<u32> = tx
            .query_row(
                "SELECT desired FROM tier WHERE name=?",
                [&self.identity.tier],
                |r| r.get(0),
            )
            .optional()?;
        let retained: i64 = tx.query_row(
            "SELECT count(*) FROM gpu_node WHERE tier=?",
            [&self.identity.tier],
            |r| r.get(0),
        )?;
        if desired.is_none_or(|n| n == 0 || retained < 0 || retained >= i64::from(n))
            || !crate::launch_rate_available(&tx, &self.identity.tier, now)?
        {
            return Err(Error::Invalid);
        }
        if tx.execute("INSERT INTO gpu_node(uid,tier,phase,instance_id,created_at,orphan,deleting) VALUES (?,?,'\"Launching\"',NULL,?,0,0)", params![intent.node_uid, self.identity.tier, now])? != 1 { return Err(Error::Corrupt); }
        if tx.execute(
            "INSERT INTO launch_attempt(uid,tier,at) VALUES (?,?,?)",
            params![intent.node_uid, self.identity.tier, now],
        )? != 1
        {
            return Err(Error::Corrupt);
        }
        let (operation, inserted) = prepare_create_tx(&tx, &store.owner, &intent, now)?;
        if !inserted
            || get(&tx, &store.owner, &intent.operation_id)?.as_ref() != Some(&operation)
            || account(&tx, &store.owner, now)?.last_at != now
        {
            return Err(Error::Corrupt);
        }
        tx.commit()?;
        Ok(operation)
    }
    pub fn fingerprint(&self, intent: &CreateIntent) -> Result<[u8; 32]> {
        self.identity.fingerprint(intent)
    }
    pub fn stop_handle(&self) -> LocalDevelopmentStop {
        self.stop.clone()
    }
    /// Retained local launch observation only; never provider absence/billing.
    pub fn launch(&self) -> Option<&LocalLaunch> {
        self.launch.as_deref()
    }
    pub fn submission_started(&self) -> bool {
        self.submitted
    }
    /// Stop-only first, then irreversible intent. No SQL, journal I/O or wait.
    /// Does not cancel Store state or promise immediate stop before publication.
    pub fn request_local_cleanup(&mut self) {
        self.stop.request_stop();
        self.cleanup_requested = true;
    }
    fn cleanup_target(&self) -> Result<&m8s_core::Instance> {
        let intent = self.executed_intent.as_ref().ok_or(Error::Invalid)?;
        let snapshot = self.executed_instance.as_ref().ok_or(Error::Invalid)?;
        let actual = self.launch.as_ref().ok_or(Error::Invalid)?.instance();
        if intent.owner != self.owner
            || self.identity.fingerprint(intent)? != intent.launch_fingerprint
            || snapshot != actual
            || actual.owner != self.owner
            || actual.node_uid != intent.node_uid
            || actual.tier != self.identity.tier
            || label(&actual.id).is_err()
        {
            return Err(Error::Invalid);
        }
        Ok(actual)
    }
    /// At most ONE concrete Local destroy and ONE fresh describe, no internal
    /// loops/retries/sleeps. Journal/FS I/O may block with NO latency guarantee;
    /// the independent timed worker never waits for this coordinator/Store.
    /// No Store/IDs supplied, no accounting mutation, PID scan or restart adopt.
    pub fn poll_local_cleanup(&mut self) -> LocalCleanupStatus {
        if !self.submitted {
            return LocalCleanupStatus::NoLocalSubmission;
        }
        if self.local.is_none()
            || self.launch.is_none()
            || self.executed_intent.is_none()
            || self.executed_instance.is_none()
        {
            return LocalCleanupStatus::Uncertain;
        }
        if !self.cleanup_requested {
            return LocalCleanupStatus::NotRequested;
        }
        self.stop.request_stop(); // before any journal work, including retries
        let expected = match self.cleanup_target() {
            Ok(target) => target.clone(),
            Err(_) => return LocalCleanupStatus::Uncertain,
        };
        #[cfg(test)]
        {
            if pause(&self.hooks.before_cleanup_io).is_err() || self.hooks.cleanup_destroy_error {
                return LocalCleanupStatus::Uncertain;
            }
        }
        let Some(local) = self.local.as_mut() else {
            return LocalCleanupStatus::Uncertain;
        };
        // Concrete fixed backend only, never a generic Provider input. A destroy
        // acknowledgment alone is NOT absence. Even a previous terminal poll
        // must revalidate the actual journal/anchor via these calls, not cache.
        if m8s_provider::Provider::destroy(local, &expected.id).is_err() {
            return LocalCleanupStatus::Uncertain;
        }
        match m8s_provider::Provider::describe(local, &expected.id) {
            Ok(observation) => cleanup_observation(
                &expected,
                self.launch.as_ref().expect("validated retained launch"),
                observation,
            ),
            Err(_) => LocalCleanupStatus::Uncertain,
        }
    }
    fn available(&self, store: &Store, intent: &CreateIntent) -> Result<()> {
        if self.submitted
            || self.stop.requested()
            || self.owner != store.owner
            || intent.owner != self.owner
            || self.identity.fingerprint(intent)? != intent.launch_fingerprint
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub fn prepare(
        &self,
        store: &Store,
        intent: &CreateIntent,
        now: i64,
    ) -> Result<CreateOperation> {
        self.available(store, intent)?;
        let tier: String = store.conn.query_row(
            "SELECT tier FROM gpu_node WHERE uid=?",
            [&intent.node_uid],
            |r| r.get(0),
        )?;
        if tier != self.identity.tier {
            return Err(Error::Invalid);
        }
        store.prepare_create(intent, now)
    }
    pub fn claim(&self, store: &Store, operation_id: &str, now: i64) -> Result<CreateClaim> {
        let r = get(&store.conn, &store.owner, operation_id)?.ok_or(Error::Invalid)?;
        self.available(store, &r.intent)?;
        store.claim_create_bound(
            operation_id,
            now,
            Some((
                self.token.clone(),
                self.identity.fingerprint(&r.intent)?,
                &self.identity.tier,
            )),
        )
    }
    /// Consumes claim on ALL outcomes. Clock is real UTC Unix seconds sampled
    /// internally; absolute reservation deadlines are NEVER rebased to runtime.
    pub fn execute(
        &mut self,
        store: &Store,
        claim: CreateClaim,
    ) -> Result<LocalDevelopmentOutcome> {
        self.available(store, &claim.intent)?;
        if !Arc::ptr_eq(&claim.origin, &store.origin)
            || claim
                .driver
                .as_ref()
                .is_none_or(|d| !Arc::ptr_eq(d, &self.token))
        {
            return Err(Error::Invalid);
        }
        if !start_bound_invocation(
            store,
            &claim,
            InvocationClock::LocalUtc,
            &self.identity.tier,
            #[cfg(test)]
            self.hooks.before_commit_fail,
        )? {
            return Ok(LocalDevelopmentOutcome::NotInvoked);
        }
        #[cfg(test)]
        {
            pause(&self.hooks.after_commit)?;
            if self.hooks.after_commit_unwind {
                panic!("local after-commit unwind");
            }
        }
        // Stop before submission may prevent child; after commit a racing stop
        // may still allow one. Its latch attaches before result Store writes.
        if self.stop.requested() {
            return Ok(LocalDevelopmentOutcome::NotInvoked);
        }
        let deadlines = LocalDeadlines::new(
            absolute(claim.serving_deadline)?,
            absolute(claim.cleanup_deadline)?,
        )?;
        self.executed_intent = Some(claim.intent.clone()); // immutable audit binding BEFORE submission
        self.submitted = true; // irrevocable before exactly one Local submission
        let result = self
            .local
            .as_mut()
            .ok_or(Error::Invalid)?
            .launch_development_until(
                &LaunchSpec {
                    owner: claim.intent.owner.clone(),
                    node_uid: claim.intent.node_uid.clone(),
                    tier: self.identity.tier.clone(),
                },
                deadlines,
            );
        let launch = match result {
            Ok(launch) => launch,
            Err(_) => {
                // Local::drop requests stop for retained timed workers; those
                // workers retain child+directory lock independently until reap.
                self.stop.request_stop();
                self.local.take();
                let _ = store.record_create_result(
                    &claim.intent.operation_id,
                    &CreateObservation::Ambiguous,
                    utc_seconds()?,
                );
                return Ok(LocalDevelopmentOutcome::Ambiguous);
            }
        };
        let actual_stop = launch.stop_handle();
        let instance = launch.instance().clone();
        self.executed_instance = Some(instance.clone());
        self.launch = Some(Arc::new(launch)); // retain ownership even if attach refuses
        #[cfg(test)]
        pause(&self.hooks.before_attach)?;
        if self.stop.attach(actual_stop).is_err() {
            let _ = store.record_create_result(
                &claim.intent.operation_id,
                &CreateObservation::Ambiguous,
                utc_seconds()?,
            );
            return Ok(LocalDevelopmentOutcome::Ambiguous);
        }
        #[cfg(test)]
        pause(&self.hooks.after_launch)?;
        let observed_at = match utc_seconds() {
            Ok(now) => now,
            Err(e) => {
                self.stop.request_stop();
                return Err(e);
            }
        };
        if instance.owner != claim.intent.owner
            || instance.node_uid != claim.intent.node_uid
            || instance.tier != self.identity.tier
            || label(&instance.id).is_err()
        {
            self.stop.request_stop();
            let _ = store.record_create_result(
                &claim.intent.operation_id,
                &CreateObservation::Ambiguous,
                observed_at,
            );
            return Ok(LocalDevelopmentOutcome::Ambiguous);
        }
        let binding = ResourceBinding { owner: claim.intent.owner.clone(), run_id: claim.intent.run_id.clone(),
            node_uid: claim.intent.node_uid.clone(), instance_id: instance.id,
            volumes: VolumeBinding::NoneAllocated, provenance: "local-development-v1: retained child; local adapter creates no volumes; not cloud proof".into(),
            observed_at, status: EvidenceStatus::Complete };
        #[cfg(test)]
        if self.hooks.lose_response {
            self.stop.request_stop();
            let _ = store.record_create_result(
                &claim.intent.operation_id,
                &CreateObservation::Ambiguous,
                observed_at,
            );
            return Ok(LocalDevelopmentOutcome::Ambiguous);
        }
        match store.record_create_result(
            &claim.intent.operation_id,
            &CreateObservation::Bound(binding),
            observed_at,
        ) {
            Ok(r) => Ok(LocalDevelopmentOutcome::Bound(Box::new(r))),
            Err(_) => {
                self.stop.request_stop();
                Ok(LocalDevelopmentOutcome::Ambiguous)
            }
        }
    }
}
impl Drop for LocalDevelopmentExecutor {
    fn drop(&mut self) {
        self.stop.request_stop();
    }
}
fn cleanup_observation(
    expected: &m8s_core::Instance,
    launch: &LocalLaunch,
    observation: m8s_core::Observation,
) -> LocalCleanupStatus {
    let direct_child = launch.termination_state();
    if direct_child == m8s_provider::local::LocalTerminationState::Uncertain {
        return LocalCleanupStatus::Uncertain;
    }
    match observation {
        m8s_core::Observation::Absent
            if direct_child == m8s_provider::local::LocalTerminationState::DirectChildReaped =>
        {
            LocalCleanupStatus::LocalOnlyAbsent
        }
        m8s_core::Observation::Present(actual)
            if actual.id == expected.id
                && actual.owner == expected.owner
                && actual.node_uid == expected.node_uid
                && actual.tier == expected.tier
                && matches!(
                    actual.state,
                    m8s_core::InstanceState::Starting
                        | m8s_core::InstanceState::Running
                        | m8s_core::InstanceState::Stopped
                ) =>
        {
            LocalCleanupStatus::Pending { direct_child }
        }
        _ => LocalCleanupStatus::Uncertain,
    }
}
fn absolute(seconds: i64) -> Result<SystemTime> {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(
            u64::try_from(seconds).map_err(|_| Error::Invalid)?,
        ))
        .ok_or(Error::Invalid)
}
pub(super) fn utc_seconds() -> Result<i64> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Invalid)?
            .as_secs(),
    )
    .map_err(|_| Error::Invalid)
}
#[cfg(test)]
struct Pause {
    entered: std::sync::mpsc::SyncSender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}
#[cfg(test)]
fn pause(p: &Option<Pause>) -> Result<()> {
    if let Some(p) = p {
        p.entered.send(()).map_err(|_| Error::Invalid)?;
        p.resume
            .recv_timeout(Duration::from_secs(8))
            .map_err(|_| Error::Invalid)?;
    }
    Ok(())
}
#[cfg(test)]
#[derive(Default)]
struct Hooks {
    before_commit_fail: bool,
    after_commit_unwind: bool,
    after_commit: Option<Pause>,
    after_launch: Option<Pause>,
    before_attach: Option<Pause>,
    lose_response: bool,
    before_cleanup_io: Option<Pause>,
    cleanup_destroy_error: bool,
}
#[cfg(test)]
mod tests;
