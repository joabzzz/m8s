//! Immutable restart inputs for observation and cleanup, never for CREATE.
use super::*;
use m8s_provider_vast::watchdog::OwnedResourceAbsence;

/// Reconstructed exact identity and original deadlines. No launch/claim method,
/// serialized effect permit, credential, signer, or live readiness assertion.
pub struct VastRecovery {
    executor: VastExecutor,
    operation: CreateOperation,
}
impl VastRecovery {
    pub fn operation(&self) -> &CreateOperation {
        &self.operation
    }
    pub fn identity(&self) -> &RecoveryIntent {
        &self.executor.identity
    }
    /// Expected original account, not a fresh authenticated account observation.
    pub fn creator_account_id(&self) -> NonZeroU64 {
        self.executor.creator_account_id
    }
    pub fn tier(&self) -> &str {
        &self.executor.tier
    }
    pub fn cleanup_starts_at(&self) -> Result<i64> {
        self.operation
            .budget
            .reserved_at
            .checked_add(
                i64::try_from(self.operation.budget.request.quote.runtime_seconds)
                    .map_err(|_| Error::Corrupt)?,
            )
            .ok_or(Error::Corrupt)
    }
    pub fn cleanup_deadline(&self) -> Result<i64> {
        self.cleanup_starts_at()?
            .checked_add(
                i64::try_from(self.operation.budget.request.quote.cleanup_seconds)
                    .map_err(|_| Error::Corrupt)?,
            )
            .ok_or(Error::Corrupt)
    }
    /// Explicit synchronous cleanup for an already bound invocation. This does
    /// not arm a worker, renew deadlines, mutate journals or authenticate at
    /// construction. Cleanup remains possible after the original deadline.
    pub fn bound_cleanup<S>(
        &self,
        supplier: S,
    ) -> Result<m8s_provider_vast::readonly::restart::RestartCleanup<S>>
    where
        S: FnMut() -> m8s_core::provider::Result<m8s_provider_vast::readonly::restart::SecretValue>,
    {
        if self.operation.invocation_started_at.is_none() {
            return Err(Error::Invalid);
        }
        let binding = self.operation.result.as_ref().ok_or(Error::Invalid)?;
        if binding.volumes != VolumeBinding::NoneAllocated {
            return Err(Error::Invalid);
        }
        let instance: NonZeroU64 = binding.instance_id.parse().map_err(|_| Error::Invalid)?;
        if instance.to_string() != binding.instance_id {
            return Err(Error::Invalid);
        }
        Ok(m8s_provider_vast::readonly::restart::RestartCleanup::new(
            self.identity().clone(),
            self.creator_account_id(),
            instance,
            supplier,
        )?)
    }
    /// The observation must still come from the creator-account adapter. Loading
    /// this capsule cannot turn a stored receipt into a fresh absence proof.
    pub fn finalize_removal(
        &self,
        store: &Store,
        receipt: &OwnedResourceAbsence,
        period: &CalendarPeriod,
    ) -> Result<super::VastRemoval> {
        self.executor
            .finalize_removal(store, &self.operation.intent, receipt, period)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Inputs {
    schema: String,
    creator_account_id: NonZeroU64,
    launch_intent: String,
}

fn encode_inputs(executor: &VastExecutor) -> Result<Vec<u8>> {
    let encoded = serde_json::to_vec(&Inputs {
        schema: "m8s.vast-recovery-inputs/v1".into(),
        creator_account_id: executor.creator_account_id,
        launch_intent: String::from_utf8(executor.canonical.clone()).map_err(|_| Error::Invalid)?,
    })
    .map_err(|_| Error::Invalid)?;
    if encoded.len() > 4096 {
        return Err(Error::Invalid);
    }
    Ok(encoded)
}

fn read(conn: &rusqlite::Connection, owner: &str, id: &str) -> Result<(String, Inputs)> {
    let (tier, bytes): (String, Vec<u8>) = conn.query_row(
        "SELECT CASE WHEN typeof(tier)='text' AND length(CAST(tier AS BLOB)) BETWEEN 1 AND 64 THEN tier ELSE NULL END,
         CASE WHEN typeof(canonical)='blob' AND length(canonical) BETWEEN 1 AND 4096 THEN canonical ELSE NULL END
         FROM vast_launch WHERE owner=? AND operation_id=?",
        params![owner, id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).map_err(|_| Error::Corrupt)?;
    let inputs: Inputs = serde_json::from_slice(&bytes).map_err(|_| Error::Corrupt)?;
    if inputs.schema != "m8s.vast-recovery-inputs/v1" {
        return Err(Error::Corrupt);
    }
    Ok((tier, inputs))
}

pub(super) fn persist(
    tx: &Transaction<'_>,
    executor: &VastExecutor,
    intent: &CreateIntent,
) -> Result<()> {
    let canonical = encode_inputs(executor)?;
    tx.execute(
        "INSERT INTO vast_launch(owner,operation_id,tier,canonical) VALUES (?,?,?,?)",
        params![
            executor.owner,
            intent.operation_id,
            executor.tier,
            canonical
        ],
    )?;
    Ok(())
}

pub(super) fn validate_persisted(
    conn: &rusqlite::Connection,
    executor: &VastExecutor,
    intent: &CreateIntent,
) -> Result<()> {
    let (tier, inputs) = read(conn, &executor.owner, &intent.operation_id)?;
    if tier != executor.tier
        || inputs.launch_intent.as_bytes() != executor.canonical
        || inputs.creator_account_id != executor.creator_account_id
    {
        return Err(Error::Corrupt);
    }
    Ok(())
}

impl Store {
    /// Restore immutable cleanup inputs from the same durable transaction that
    /// prepared CREATE. Missing legacy metadata or mismatched fingerprints refuse;
    /// no migration guesses an image/label and no new invocation is constructed.
    pub fn recover_vast(&self, operation_id: &str) -> Result<VastRecovery> {
        label(operation_id)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Deferred)?;
        owned(&tx, &self.owner)?;
        let recovered = restore(&tx, &self.owner, operation_id)?;
        tx.commit()?;
        Ok(recovered)
    }
}

pub(super) fn restore(
    conn: &rusqlite::Connection,
    owner: &str,
    operation_id: &str,
) -> Result<VastRecovery> {
    // Bound the accompanying JSON and row keys before the shared decoders copy
    // them into Rust. Capsule bounds alone do not bound these older journals.
    let bounded: bool = conn.query_row(
        "SELECT typeof(c.record)='text' AND length(CAST(c.record AS BLOB)) BETWEEN 1 AND 65536
         AND typeof(b.record)='text' AND length(CAST(b.record AS BLOB)) BETWEEN 1 AND 65536
         AND typeof(c.request_id)='text' AND length(CAST(c.request_id AS BLOB)) BETWEEN 1 AND 256
         AND typeof(c.node_uid)='text' AND length(CAST(c.node_uid AS BLOB)) BETWEEN 1 AND 256
         AND typeof(b.node_uid)='text' AND length(CAST(b.node_uid AS BLOB)) BETWEEN 1 AND 256
         FROM create_operation c JOIN budget_reservation b ON b.owner=c.owner AND b.request_id=c.request_id
         WHERE c.owner=? AND c.operation_id=?",
        params![owner, operation_id], |r| r.get(0),
    ).map_err(|_| Error::Corrupt)?;
    if !bounded {
        return Err(Error::Corrupt);
    }
    let operation = get(conn, owner, operation_id)?.ok_or(Error::Invalid)?;
    let (tier, inputs) = read(conn, owner, operation_id)?;
    let request = LaunchRequest::from_canonical_intent(inputs.launch_intent.as_bytes())?;
    let mut executor = VastExecutor::new(
        request,
        owner,
        &operation.intent.node_uid,
        &tier,
        inputs.creator_account_id,
    )?;
    if executor.fingerprint(&operation.intent)? != operation.intent.launch_fingerprint {
        return Err(Error::Corrupt);
    }
    let current = budget(conn, &operation.intent)?;
    if current.request != operation.budget.request
        || current.period != operation.budget.period
        || current.reserved_at != operation.budget.reserved_at
        || current.reserved_all_in != operation.budget.reserved_all_in
        || current.reserved_hourly != operation.budget.reserved_hourly
        || operation.result.as_ref() != current.binding.as_ref()
    {
        return Err(Error::Corrupt);
    }
    // Deliberately remove the reconstructed request before returning. Even
    // internal accidental use of claim/execute fails the availability gate.
    executor.request = None;
    let recovered = VastRecovery {
        executor,
        operation,
    };
    recovered.cleanup_deadline()?;
    Ok(recovered)
}

#[cfg(test)]
mod tests;
