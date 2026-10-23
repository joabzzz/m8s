//! Shared exact-removal finalization. Public receipts are recovery metadata;
//! only a creator-account observation can enter the effectful finalizer.
use super::*;
use crate::budget::{FinalBilling, ResourceOutcome, SettlementEvidence, VolumeOutcome};
use m8s_provider::vast::watchdog::OwnedResourceAbsence;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VastRemoval {
    pub owner: String,
    pub operation_id: String,
    pub request_id: String,
    pub node_uid: String,
    pub instance_id: String,
    pub image: String,
    pub observed_at: i64,
}

impl Store {
    /// Stored provider evidence, not a new observation or finalized invoice.
    /// Reading this never launches, destroys, releases holds or restores claims.
    pub fn vast_removals(&self) -> Result<Vec<VastRemoval>> {
        let mut q = self.conn.prepare(
            "SELECT operation_id,record FROM vast_removal WHERE owner=? ORDER BY operation_id LIMIT 1025",
        )?;
        let rows = q.query_map([&self.owner], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut records = Vec::new();
        for row in rows {
            let (id, raw) = row?;
            let record: VastRemoval = decode(&raw)?;
            if record.owner != self.owner || record.operation_id != id || records.len() == 1024 {
                return Err(Error::Corrupt);
            }
            let operation = get(&self.conn, &self.owner, &id)?.ok_or(Error::Corrupt)?;
            validate_record(&record, &operation)?;
            records.push(record);
        }
        Ok(records)
    }
}

fn validate_record(record: &VastRemoval, operation: &CreateOperation) -> Result<()> {
    let binding = operation.result.as_ref().ok_or(Error::Corrupt)?;
    if record.operation_id != operation.intent.operation_id
        || record.owner != operation.intent.owner
        || record.request_id != operation.intent.request_id
        || record.node_uid != operation.intent.node_uid
        || record.instance_id != binding.instance_id
        || operation
            .invocation_started_at
            .is_none_or(|at| record.observed_at < at)
        || record.observed_at < binding.observed_at
        || RecoveryIntent::new(&record.owner, &record.node_uid, "validation", &record.image)
            .is_err()
    {
        return Err(Error::Corrupt);
    }
    Ok(())
}

impl VastExecutor {
    /// Atomically retain the absence receipt, cancel node grants, remove only
    /// its exact row, and retain the budget as BillingUnknown. No final charges
    /// are invented, no CREATE claim is released, no replacement is launched.
    pub fn finalize_removal(
        &self,
        store: &Store,
        intent: &CreateIntent,
        receipt: &OwnedResourceAbsence,
        period: &CalendarPeriod,
    ) -> Result<VastRemoval> {
        if !receipt.matches(&self.identity, self.creator_account_id) {
            return Err(Error::Invalid);
        }
        self.finalize_removal_at(
            store,
            intent,
            &receipt.instance_id().to_string(),
            receipt.observed_at(),
            period,
            wall_utc()?,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finalize_removal_at(
        &self,
        store: &Store,
        intent: &CreateIntent,
        instance: &str,
        observed_at: i64,
        period: &CalendarPeriod,
        now: i64,
    ) -> Result<VastRemoval> {
        if store.owner != self.owner
            || self.fingerprint(intent)? != intent.launch_fingerprint
            || observed_at < 0
            || observed_at > now
            || now - observed_at > EVIDENCE_MAX_AGE_SECONDS
        {
            return Err(Error::Invalid);
        }
        let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
        owned(&tx, &self.owner)?;
        let operation = get(&tx, &self.owner, &intent.operation_id)?.ok_or(Error::Invalid)?;
        let b = budget(&tx, intent)?;
        let binding = operation.result.as_ref().ok_or(Error::Invalid)?;
        if operation.intent != *intent
            || operation.state == CreateState::Prepared
            || operation.invocation_started_at.is_none()
            || binding.instance_id != instance
            || binding.volumes != VolumeBinding::NoneAllocated
            || b.binding.as_ref() != Some(binding)
        {
            return Err(Error::Invalid);
        }
        let record = VastRemoval {
            owner: self.owner.clone(),
            operation_id: intent.operation_id.clone(),
            request_id: intent.request_id.clone(),
            node_uid: self.node_uid.clone(),
            instance_id: instance.into(),
            image: self.identity.image().into(),
            observed_at,
        };
        validate_record(&record, &operation)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT record FROM vast_removal WHERE owner=? AND operation_id=?",
                params![self.owner, intent.operation_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            let previous: VastRemoval = decode(&existing)?;
            validate_record(&previous, &operation)?;
            if previous.image != record.image || previous.observed_at > observed_at {
                return Err(Error::Corrupt);
            }
            let retained: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM gpu_node WHERE uid=?)",
                [&self.node_uid],
                |r| r.get(0),
            )?;
            if retained || !matches!(b.state, BudgetState::BillingUnknown | BudgetState::Settled) {
                return Err(Error::Corrupt);
            }
            // Do not replace the original observation or downgrade a settled bill.
            return Ok(previous);
        }
        crate::budget::settle_in_transaction(
            &tx,
            &self.owner,
            &intent.request_id,
            period,
            &SettlementEvidence {
                binding: binding.clone(),
                observed_at,
                status: EvidenceStatus::Complete,
                resource: ResourceOutcome::OwnedAbsent,
                volumes: VolumeOutcome::BoundVolumesAbsent,
                billing: FinalBilling::Unknown,
                provenance: "creator-account-exact-get-null".into(),
            },
            now,
        )?;
        if !crate::removal::remove_absent_node(&tx, &self.owner, &self.node_uid, instance, now)? {
            return Err(Error::Invalid);
        }
        tx.execute(
            "INSERT INTO vast_removal(owner,operation_id,record) VALUES (?,?,?)",
            params![self.owner, intent.operation_id, encode(&record)?],
        )?;
        tx.commit()?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests;
