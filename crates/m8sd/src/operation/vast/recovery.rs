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

fn read(conn: &rusqlite::Connection, owner: &str, id: &str) -> Result<(String, Vec<u8>)> {
    conn.query_row(
        "SELECT CASE WHEN typeof(tier)='text' AND length(CAST(tier AS BLOB)) BETWEEN 1 AND 64 THEN tier ELSE NULL END,
         CASE WHEN typeof(canonical)='blob' AND length(canonical) BETWEEN 1 AND 4096 THEN canonical ELSE NULL END
         FROM vast_launch WHERE owner=? AND operation_id=?",
        params![owner, id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).map_err(|_| Error::Corrupt)
}

pub(super) fn persist(
    tx: &Transaction<'_>,
    executor: &VastExecutor,
    intent: &CreateIntent,
) -> Result<()> {
    if executor.canonical.len() > 4096 {
        return Err(Error::Invalid);
    }
    tx.execute(
        "INSERT INTO vast_launch(owner,operation_id,tier,canonical) VALUES (?,?,?,?)",
        params![
            executor.owner,
            intent.operation_id,
            executor.tier,
            executor.canonical
        ],
    )?;
    Ok(())
}

pub(super) fn validate_persisted(
    conn: &rusqlite::Connection,
    executor: &VastExecutor,
    intent: &CreateIntent,
) -> Result<()> {
    let (tier, canonical) = read(conn, &executor.owner, &intent.operation_id)?;
    if tier != executor.tier || canonical != executor.canonical {
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
        let operation = get(&tx, &self.owner, operation_id)?.ok_or(Error::Invalid)?;
        let (tier, canonical) = read(&tx, &self.owner, operation_id)?;
        let request = LaunchRequest::from_canonical_intent(&canonical)?;
        let mut executor =
            VastExecutor::new(request, &self.owner, &operation.intent.node_uid, &tier)?;
        if executor.fingerprint(&operation.intent)? != operation.intent.launch_fingerprint {
            return Err(Error::Corrupt);
        }
        let current = budget(&tx, &operation.intent)?;
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
        tx.commit()?;
        Ok(recovered)
    }
}

#[cfg(test)]
mod tests;
