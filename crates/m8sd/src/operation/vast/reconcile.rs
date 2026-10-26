//! Daemon restart reconciliation of persisted managed Vast operations.
//!
//! Observation and cleanup only: this module has no create request, claim or
//! signing key, never rebases a deadline, and never reads an empty or failed
//! lookup as absence. Provider access is an injected port so every persisted
//! transition can be fault-tested without a network.
use super::*;
use m8s_core::{Phase, policy::transition};
use m8s_provider::vast::watchdog::OwnedResourceAbsence;

/// Fresh exact-resource absence. Only the provider's opaque creator-account
/// receipt can build one outside tests; stored removal metadata cannot.
pub struct Absence(AbsenceInner);
enum AbsenceInner {
    Provider(OwnedResourceAbsence),
    #[cfg(test)]
    Synthetic {
        instance: NonZeroU64,
        observed_at: i64,
        matches: bool,
    },
}
impl Absence {
    pub fn from_provider(receipt: OwnedResourceAbsence) -> Self {
        Self(AbsenceInner::Provider(receipt))
    }
    #[cfg(test)]
    pub(crate) fn synthetic(instance: NonZeroU64, observed_at: i64, matches: bool) -> Self {
        Self(AbsenceInner::Synthetic {
            instance,
            observed_at,
            matches,
        })
    }
    fn instance(&self) -> NonZeroU64 {
        match &self.0 {
            AbsenceInner::Provider(r) => r.instance_id(),
            #[cfg(test)]
            AbsenceInner::Synthetic { instance, .. } => *instance,
        }
    }
    fn observed_at(&self) -> i64 {
        match &self.0 {
            AbsenceInner::Provider(r) => r.observed_at(),
            #[cfg(test)]
            AbsenceInner::Synthetic { observed_at, .. } => *observed_at,
        }
    }
    fn matches(&self, recovery: &VastRecovery) -> bool {
        match &self.0 {
            AbsenceInner::Provider(r) => {
                r.matches(recovery.identity(), recovery.creator_account_id())
            }
            #[cfg(test)]
            AbsenceInner::Synthetic { matches, .. } => *matches,
        }
    }
}

/// Provider effects available after restart. There is deliberately no create.
/// Every method receives the reconstructed original identity; implementations
/// must authenticate the original creator account with their own credential.
pub trait RestartPort {
    /// Exact owner/node/tier label and immutable image lookup for an invocation
    /// whose result was never recorded. `Ok(None)` and `Err` are both unresolved.
    fn lookup(&mut self, recovery: &VastRecovery) -> m8s_provider::Result<Option<NonZeroU64>>;
    /// Acknowledgement is not absence; failure may still have taken effect.
    fn destroy(
        &mut self,
        recovery: &VastRecovery,
        instance: NonZeroU64,
    ) -> m8s_provider::Result<()>;
    /// `Ok(None)` (still present/stopped) and `Err` leave liability unresolved.
    fn confirm_absent(
        &mut self,
        recovery: &VastRecovery,
        instance: NonZeroU64,
    ) -> m8s_provider::Result<Option<Absence>>;
}

/// When a bound resource without a live worker becomes eligible for destroy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupTrigger {
    /// Only at the original serving deadline, a down request or a destroying row.
    AtDeadline,
    /// Resources left by a prior process have no surviving worker: clean up now.
    Immediate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Exact removal is already recorded; nothing to do.
    Finished,
    /// Reservation/intent only. No effect is possible or taken after restart.
    NotDispatched,
    /// Claim consumed but the create was never invoked. No effect, liability kept.
    ConsumedNotInvoked,
    /// Create may have taken effect; lookup found nothing or failed. Not absence.
    CreateUnresolved,
    /// Exact resource bound; destroy waits for its original deadline.
    AwaitingCleanup,
    /// Destroy attempted; exact-ID absence not yet confirmed. Liability retained.
    AbsenceUnconfirmed,
    /// Provider-confirmed exact absence committed atomically.
    Removed,
    /// Inconsistent or unwritable state; nothing further was attempted.
    Refused,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperationReport {
    pub operation_id: String,
    pub outcome: Outcome,
}

fn lookup_binding(
    recovery: &VastRecovery,
    instance: NonZeroU64,
    observed_at: i64,
) -> ResourceBinding {
    let intent = &recovery.operation().intent;
    ResourceBinding {
        owner: intent.owner.clone(),
        run_id: intent.run_id.clone(),
        node_uid: intent.node_uid.clone(),
        instance_id: instance.to_string(),
        volumes: VolumeBinding::NoneAllocated,
        provenance:
            "Vast restart lookup: exact owner label and immutable image; no volume requested".into(),
        observed_at,
        status: EvidenceStatus::Complete,
    }
}

fn event_in(tx: &Transaction<'_>, now: i64, message: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO event(at,message) VALUES (?,?)",
        params![now, message],
    )?;
    Ok(())
}

impl Store {
    /// One bounded pass over every persisted managed operation. Each operation is
    /// handled independently; a refusal never aborts the others, and the
    /// returned report is the only success signal (no empty-means-gone result).
    pub fn reconcile_vast(
        &self,
        port: &mut impl RestartPort,
        trigger: CleanupTrigger,
        mut clock: impl FnMut() -> Result<i64>,
    ) -> Result<Vec<OperationReport>> {
        let ids: Vec<String> = self
            .create_operations()?
            .into_iter()
            .map(|o| o.intent.operation_id)
            .collect();
        let mut report = Vec::with_capacity(ids.len());
        for id in ids {
            let outcome = self
                .reconcile_vast_one(&id, port, trigger, &mut clock)
                .unwrap_or(Outcome::Refused);
            if outcome == Outcome::Refused {
                let _ = clock().and_then(|now| {
                    self.event(now, "managed restart reconcile refused; state retained")
                });
            }
            report.push(OperationReport {
                operation_id: id,
                outcome,
            });
        }
        Ok(report)
    }

    fn reconcile_vast_one(
        &self,
        id: &str,
        port: &mut impl RestartPort,
        trigger: CleanupTrigger,
        clock: &mut impl FnMut() -> Result<i64>,
    ) -> Result<Outcome> {
        let removed: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM vast_removal WHERE owner=? AND operation_id=?)",
            params![self.owner, id],
            |r| r.get(0),
        )?;
        if removed {
            return Ok(Outcome::Finished);
        }
        let mut recovery = self.recover_vast(id)?;
        let op = recovery.operation();
        if op.dispatched_at.is_none() {
            return Ok(Outcome::NotDispatched);
        }
        if op.invocation_started_at.is_none() {
            return Ok(Outcome::ConsumedNotInvoked);
        }
        if recovery.operation().result.is_none() {
            // A crash can land between the invocation burn and its ambiguity mark.
            if !recovery.operation().uncertainty_observed {
                self.record_create_result(id, &CreateObservation::Ambiguous, clock()?)?;
            }
            let found = match port.lookup(&recovery) {
                Ok(Some(instance)) => instance,
                Ok(None) | Err(_) => {
                    self.event(clock()?, "create outcome unresolved; lookup is not absence")?;
                    return Ok(Outcome::CreateUnresolved);
                }
            };
            let now = clock()?;
            let binding = lookup_binding(&recovery, found, now);
            self.bind_lookup(id, &recovery, binding, now)?;
            recovery = self.recover_vast(id)?;
        }
        let binding = recovery.operation().result.clone().ok_or(Error::Corrupt)?;
        let instance: NonZeroU64 = binding.instance_id.parse().map_err(|_| Error::Corrupt)?;
        let now = clock()?;
        if !self.cleanup_due(&recovery, trigger, now)? {
            return Ok(Outcome::AwaitingCleanup);
        }
        // The Destroying phase and grant cancellation are durable before any
        // provider effect, so a crash after DELETE still resumes cleanup.
        self.mark_destroying(&recovery, &binding, now)?;
        match port.destroy(&recovery, instance) {
            Ok(()) => self.event(
                clock()?,
                "restart cleanup destroy acknowledged; absence unconfirmed",
            )?,
            Err(_) => self.event(
                clock()?,
                "restart cleanup destroy uncertain; absence unconfirmed",
            )?,
        }
        let absence = match port.confirm_absent(&recovery, instance) {
            Ok(Some(absence)) => absence,
            Ok(None) | Err(_) => return Ok(Outcome::AbsenceUnconfirmed),
        };
        if !absence.matches(&recovery) || absence.instance() != instance {
            return Err(Error::Invalid);
        }
        let period = recovery.operation().budget.period.clone();
        recovery.executor.finalize_removal_at(
            self,
            &recovery.operation().intent,
            &instance.to_string(),
            absence.observed_at(),
            &period,
            clock()?,
        )?;
        Ok(Outcome::Removed)
    }

    fn cleanup_due(
        &self,
        recovery: &VastRecovery,
        trigger: CleanupTrigger,
        now: i64,
    ) -> Result<bool> {
        if trigger == CleanupTrigger::Immediate || now >= recovery.cleanup_starts_at()? {
            return Ok(true);
        }
        let intent = &recovery.operation().intent;
        Ok(self.conn.query_row(
            "SELECT COALESCE((SELECT t.desired=0 FROM tier t WHERE t.name=n.tier),1)
                 OR n.deleting=1 OR n.phase IN ('\"Destroying\"','\"Draining\"')
             FROM gpu_node n WHERE n.uid=?",
            [&intent.node_uid],
            |r| r.get(0),
        )?)
    }

    /// Persist an exact lookup match and its node row together. Failure leaves
    /// neither: the unresolved invocation and its cleanup custody stay intact.
    fn bind_lookup(
        &self,
        id: &str,
        recovery: &VastRecovery,
        binding: ResourceBinding,
        now: i64,
    ) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        record_create_result_in(
            &tx,
            &self.owner,
            id,
            &CreateObservation::Bound(binding.clone()),
            now,
        )?;
        let changed = tx.execute(
            "UPDATE gpu_node SET instance_id=?1,
                 phase=CASE WHEN phase='\"Launching\"' THEN '\"Booting\"' ELSE phase END
             WHERE uid=?2 AND tier=?3 AND (instance_id IS NULL OR instance_id=?1)",
            params![
                binding.instance_id,
                recovery.operation().intent.node_uid,
                recovery.tier()
            ],
        )?;
        if changed != 1 {
            return Err(Error::Invalid);
        }
        event_in(
            &tx,
            now,
            "restart lookup bound exact resource; cleanup custody recorded",
        )?;
        tx.commit()?;
        Ok(())
    }

    fn mark_destroying(
        &self,
        recovery: &VastRecovery,
        binding: &ResourceBinding,
        now: i64,
    ) -> Result<()> {
        let uid = &recovery.operation().intent.node_uid;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        owned(&tx, &self.owner)?;
        let (instance, phase): (Option<String>, String) = tx.query_row(
            "SELECT instance_id,phase FROM gpu_node WHERE uid=?",
            [uid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if instance
            .as_deref()
            .is_some_and(|i| i != binding.instance_id)
        {
            return Err(Error::Corrupt);
        }
        let phase: Phase = serde_json::from_str(&phase).map_err(|_| Error::Corrupt)?;
        transition(phase, Phase::Destroying, Default::default()).map_err(|_| Error::Corrupt)?;
        crate::nonce::cancel_uid(&tx, &self.owner, uid)?;
        tx.execute(
            "UPDATE gpu_node SET instance_id=?1,phase='\"Destroying\"' WHERE uid=?2",
            params![binding.instance_id, uid],
        )?;
        event_in(
            &tx,
            now,
            "restart cleanup custody: node destroying; grants cancelled",
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
