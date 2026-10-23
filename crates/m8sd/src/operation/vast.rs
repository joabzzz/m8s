//! One-shot Vast dispatch backed by the Store's durable claim and an acknowledged
//! independent cleanup process. No credential discovery, retries, or Ready grant.
use super::*;
use crate::budget::{AdminBudget, BudgetRequest, CalendarPeriod};
use m8s_provider::vast::{
    readonly::{
        access::NodeAccess,
        launch::{CreatedCandidate, LaunchRequest},
        recovery::RecoveryIntent,
    },
    watchdog::ArmedCleanup,
};
use sha2::{Digest, Sha256};
use std::num::NonZeroU64;

pub enum DispatchOutcome {
    Candidate(CreatedCandidate),
    Ambiguous,
    NotInvoked,
}

/// Persisted preparation metadata, not permission to invoke a provider effect.
/// Reopening a Store may recover this metadata but never a consumed claim.
pub struct PreparedVastLaunch {
    pub reservation: BudgetReservation,
    pub intent: CreateIntent,
    pub cleanup_starts_at: u64,
}

/// Trusted prototype coordinator only. The account budget, original immutable
/// source/image, price evidence and cleanup plan must be supplied independently.
/// One allocation consumes at most one request; reopening never remints a claim.
pub struct VastExecutor {
    request: Option<LaunchRequest>,
    canonical: Vec<u8>,
    creator_account_id: NonZeroU64,
    identity: RecoveryIntent,
    owner: String,
    node_uid: String,
    tier: String,
    token: Arc<DriverOrigin>,
}

mod audit;
pub use audit::{AttemptStage, RecoveryMetadata, VastAttemptStatus, VastAudit};
mod recovery;
pub use recovery::VastRecovery;
mod removal;
pub use removal::VastRemoval;
#[cfg(test)]
mod tests;
impl VastExecutor {
    /// The account ID is a trusted coordinator input from authenticated preflight
    /// using the same retained credential as dispatch. Storing/fingerprinting it
    /// is not a fresh account check and never grants credential access.
    pub fn new(
        request: LaunchRequest,
        owner: &str,
        node_uid: &str,
        tier: &str,
        creator_account_id: NonZeroU64,
    ) -> Result<Self> {
        let identity = RecoveryIntent::new(owner, node_uid, tier, request.identity().image())?;
        if identity.label() != request.identity().label() {
            return Err(Error::Invalid);
        }
        let canonical = request.canonical_intent()?;
        Ok(Self {
            request: Some(request),
            canonical,
            creator_account_id,
            identity,
            owner: owner.into(),
            node_uid: node_uid.into(),
            tier: tier.into(),
            token: Arc::new(DriverOrigin),
        })
    }

    /// Shared CLI/daemon preparation boundary. The caller must establish desired
    /// state and supply fresh account/price evidence first. Reservation commits
    /// before launch intent; any later failure retains that conservative debt.
    /// No provider call, cleanup process or create claim is made here.
    pub fn prepare_launch(
        &self,
        store: &Store,
        policy: &AdminBudget,
        period: &CalendarPeriod,
        request: &BudgetRequest,
        operation_id: &str,
    ) -> Result<PreparedVastLaunch> {
        self.prepare_launch_with_clock(store, policy, period, request, operation_id, wall_utc)
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_launch_with_clock(
        &self,
        store: &Store,
        policy: &AdminBudget,
        period: &CalendarPeriod,
        request: &BudgetRequest,
        operation_id: &str,
        mut clock: impl FnMut() -> Result<i64>,
    ) -> Result<PreparedVastLaunch> {
        if request.owner != self.owner || request.node_uid != self.node_uid {
            return Err(Error::Invalid);
        }
        let intent = self.intent(&request.run_id, &request.request_id, operation_id)?;
        self.available(store, &intent)?;
        let reservation = store.reserve_budget(policy, period, request, clock()?)?;
        self.prepare_at(store, &intent, &mut clock)?;
        let cleanup_starts_at = self.cleanup_starts_at_with_clock(store, &intent, clock)?;
        Ok(PreparedVastLaunch {
            reservation,
            intent,
            cleanup_starts_at,
        })
    }

    /// The shared dispatch boundary checks an independently armed cleanup guard
    /// before consuming the one-use claim. Execution rechecks the same guard
    /// after claim and again after durable invocation, including writer delays.
    pub fn dispatch_prepared(
        &mut self,
        store: &Store,
        intent: &CreateIntent,
        cleanup: &mut ArmedCleanup,
    ) -> Result<DispatchOutcome> {
        let deadline = self.cleanup_starts_at(store, intent)?;
        if !cleanup.guards(&self.identity, deadline)? {
            return Err(Error::Invalid);
        }
        let claim = self.claim(store, &intent.operation_id)?;
        self.execute(store, claim, cleanup)
    }

    pub fn intent(
        &self,
        run_id: &str,
        request_id: &str,
        operation_id: &str,
    ) -> Result<CreateIntent> {
        let mut intent = CreateIntent {
            owner: self.owner.clone(),
            node_uid: self.node_uid.clone(),
            run_id: run_id.into(),
            request_id: request_id.into(),
            operation_id: operation_id.into(),
            launch_fingerprint: [0; 32],
        };
        intent.launch_fingerprint = self.fingerprint(&intent)?;
        Ok(intent)
    }

    fn fingerprint(&self, intent: &CreateIntent) -> Result<[u8; 32]> {
        if intent.owner != self.owner || intent.node_uid != self.node_uid {
            return Err(Error::Invalid);
        }
        let mut hash = Sha256::new();
        hash.update(b"m8s-vast-prototype-create-v2\0");
        hash.update(self.creator_account_id.get().to_be_bytes());
        for field in [
            &intent.owner,
            &intent.run_id,
            &intent.request_id,
            &intent.node_uid,
            &intent.operation_id,
        ] {
            label(field)?;
            hash.update((field.len() as u32).to_be_bytes());
            hash.update(field.as_bytes());
        }
        hash.update((self.canonical.len() as u32).to_be_bytes());
        hash.update(&self.canonical);
        Ok(hash.finalize().into())
    }

    fn available(&self, store: &Store, intent: &CreateIntent) -> Result<()> {
        if self.request.is_none()
            || store.owner != self.owner
            || self.fingerprint(intent)? != intent.launch_fingerprint
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }

    /// Insert the real launch row only alongside its already-reserved budget and
    /// durable create intent. This prototype allows one lifetime attempt in this
    /// owner Store, even if a previous operation was cancelled or settled.
    pub fn prepare_managed(&self, store: &Store, intent: &CreateIntent) -> Result<CreateOperation> {
        self.prepare_at(store, intent, wall_utc)
    }

    fn prepare_at(
        &self,
        store: &Store,
        intent: &CreateIntent,
        clock: impl FnOnce() -> Result<i64>,
    ) -> Result<CreateOperation> {
        self.available(store, intent)?;
        let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
        let now = clock()?; // sample after acquiring SQLite's writer
        owned(&tx, &store.owner)?;
        let a = account(&tx, &store.owner, now)?;
        if let Some(existing) = get(&tx, &store.owner, &intent.operation_id)? {
            if existing.intent != *intent || now < existing.last_at {
                return Err(Error::Invalid);
            }
            recovery::validate_persisted(&tx, self, intent)?;
            return Ok(existing); // metadata only, never a new invocation
        }
        let history: i64 = tx.query_row(
            "SELECT count(*) FROM create_operation WHERE owner=?",
            [&store.owner],
            |r| r.get(0),
        )?;
        if history != 0
            || a.policy.trial_all_in > 6_000_000
            || a.policy.max_instances != 1
            || a.policy.max_runtime_seconds > 1800
            || a.policy.minimum_cleanup_seconds < 240
        {
            return Err(Error::Invalid);
        }
        let b = budget(&tx, intent)?;
        crate::budget::validate_create_dispatch(&tx, &b, &a, now)?;
        let desired: Option<u32> = tx
            .query_row("SELECT desired FROM tier WHERE name=?", [&self.tier], |r| {
                r.get(0)
            })
            .optional()?;
        let retained: i64 = tx.query_row("SELECT count(*) FROM gpu_node", [], |r| r.get(0))?;
        if desired != Some(1)
            || retained != 0
            || !crate::launch_rate_available(&tx, &self.tier, now)?
        {
            return Err(Error::Invalid);
        }
        if tx.execute("INSERT INTO gpu_node(uid,tier,phase,instance_id,created_at,orphan,deleting) VALUES (?,?,'\"Launching\"',NULL,?,0,0)", params![intent.node_uid,self.tier,now])? != 1 {
            return Err(Error::Corrupt);
        }
        tx.execute(
            "INSERT INTO launch_attempt(uid,tier,at) VALUES (?,?,?)",
            params![intent.node_uid, self.tier, now],
        )?;
        let (operation, inserted) = prepare_create_tx(&tx, &store.owner, intent, now)?;
        if !inserted {
            return Err(Error::Corrupt);
        }
        recovery::persist(&tx, self, intent)?;
        tx.commit()?;
        Ok(operation)
    }

    /// The cleanup worker must start at the original serving deadline, leaving
    /// the separately reserved cleanup window for destruction. Read this from
    /// the same durable budget used by claim; coordinators must not reconstruct
    /// it from a fresh clock or confuse it with the final cleanup deadline.
    pub fn cleanup_starts_at(&self, store: &Store, intent: &CreateIntent) -> Result<u64> {
        self.cleanup_starts_at_with_clock(store, intent, wall_utc)
    }
    fn cleanup_starts_at_with_clock(
        &self,
        store: &Store,
        intent: &CreateIntent,
        clock: impl FnOnce() -> Result<i64>,
    ) -> Result<u64> {
        self.available(store, intent)?;
        let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
        let now = clock()?;
        owned(&tx, &store.owner)?;
        let a = account(&tx, &store.owner, now)?;
        let operation = get(&tx, &store.owner, &intent.operation_id)?.ok_or(Error::Invalid)?;
        if operation.intent != *intent || operation.state != CreateState::Prepared {
            return Err(Error::Invalid);
        }
        let b = budget(&tx, intent)?;
        let (serving, _) = crate::budget::validate_create_dispatch(&tx, &b, &a, now)?;
        u64::try_from(serving).map_err(|_| Error::Invalid)
    }

    pub fn claim(&self, store: &Store, operation_id: &str) -> Result<CreateClaim> {
        let r = get(&store.conn, &store.owner, operation_id)?.ok_or(Error::Invalid)?;
        self.available(store, &r.intent)?;
        store.claim_create_bound(
            operation_id,
            wall_utc()?,
            Some((self.token.clone(), self.fingerprint(&r.intent)?, &self.tier)),
        )
    }

    /// Burns the durable invocation before the sole HTTP create request. The
    /// cleanup process remains with the caller on every outcome. A candidate ID
    /// is not a resource observation: all outcomes retain uncertain liability
    /// until separate exact provider observation/destruction and settlement.
    pub fn execute(
        &mut self,
        store: &Store,
        claim: CreateClaim,
        cleanup: &mut ArmedCleanup,
    ) -> Result<DispatchOutcome> {
        self.available(store, &claim.intent)?;
        if !Arc::ptr_eq(&claim.origin, &store.origin)
            || claim
                .driver
                .as_ref()
                .is_none_or(|d| !Arc::ptr_eq(d, &self.token))
        {
            return Err(Error::Invalid);
        }
        let destroy_at = u64::try_from(claim.serving_deadline).map_err(|_| Error::Invalid)?;
        if claim
            .cleanup_deadline
            .checked_sub(claim.serving_deadline)
            .is_none_or(|n| n < 240)
            || !cleanup.guards(&self.identity, destroy_at)?
        {
            return Err(Error::Invalid);
        }
        let request = self.request.take().ok_or(Error::Invalid)?;
        if !start_bound_invocation(
            store,
            &claim,
            InvocationClock::LocalUtc,
            &self.tier,
            #[cfg(test)]
            false,
        )? {
            return Ok(DispatchOutcome::NotInvoked);
        }
        // SQL writer contention must not allow the cleanup deadline to pass
        // between the first guard check and the actual provider invocation.
        if !cleanup.guards(&self.identity, destroy_at)? {
            return Ok(DispatchOutcome::NotInvoked);
        }
        let result = cleanup.dispatch_once(request);
        store.record_create_result(
            &claim.intent.operation_id,
            &CreateObservation::Ambiguous,
            wall_utc()?,
        )?;
        Ok(match result {
            Ok(candidate) => DispatchOutcome::Candidate(candidate),
            Err(_) => DispatchOutcome::Ambiguous,
        })
    }

    /// Persist positive exact-instance observation before allowing nonce issue.
    /// This only advances Launching -> Booting; no certificate or model readiness
    /// is inferred. A partial database failure retains resource cleanup custody.
    pub fn bind_observed(
        &self,
        store: &Store,
        intent: &CreateIntent,
        access: &NodeAccess,
    ) -> Result<CreateOperation> {
        let now = wall_utc()?;
        if store.owner != self.owner
            || self.fingerprint(intent)? != intent.launch_fingerprint
            || !access.matches(&self.identity)
        {
            return Err(Error::Invalid);
        }
        let operation =
            get(&store.conn, &store.owner, &intent.operation_id)?.ok_or(Error::Invalid)?;
        if operation.intent != *intent
            || operation
                .invocation_started_at
                .is_none_or(|at| access.observed_at() < at)
        {
            return Err(Error::Invalid);
        }
        let binding = ResourceBinding {
            owner: self.owner.clone(),
            run_id: intent.run_id.clone(),
            node_uid: self.node_uid.clone(),
            instance_id: access.instance_id().to_string(),
            volumes: VolumeBinding::NoneAllocated,
            provenance:
                "Vast exact GET: owner label, immutable image and public ports; no volume requested"
                    .into(),
            observed_at: access.observed_at(),
            status: EvidenceStatus::Complete,
        };
        // Persist even if cancellation raced: it is still an owned cleanup debt.
        let bound = store.record_create_result(
            &intent.operation_id,
            &CreateObservation::Bound(binding),
            now,
        )?;
        let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
        let now = wall_utc()?;
        let current = get(&tx, &store.owner, &intent.operation_id)?.ok_or(Error::Invalid)?;
        if current.state != CreateState::DispatchConsumed
            || current.invocation_started_at.is_none()
            || now < access.observed_at()
            || now - access.observed_at() > EVIDENCE_MAX_AGE_SECONDS
        {
            return Err(Error::Invalid);
        }
        let changed=tx.execute("UPDATE gpu_node SET instance_id=?1,phase='\"Booting\"' WHERE uid=?2 AND tier=?3 AND deleting=0 AND (instance_id IS NULL OR instance_id=?1) AND phase IN ('\"Launching\"','\"Booting\"') AND EXISTS(SELECT 1 FROM tier WHERE name=?3 AND desired=1)",params![access.instance_id().to_string(),self.node_uid,self.tier])?;
        if changed != 1 {
            return Err(Error::Invalid);
        }
        tx.commit()?;
        Ok(bound)
    }
}
