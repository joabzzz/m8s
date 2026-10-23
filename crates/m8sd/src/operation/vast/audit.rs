//! Bounded historical journal projection. No provider, credentials or effects.
use super::*;
use crate::budget::{FinalBilling, ResourceOutcome, VolumeOutcome};
use std::collections::BTreeMap;

const MAX_ROWS: usize = 128;
const MAX_RECORD_BYTES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStage {
    ReservationOnly,
    Prepared,
    CancelledBeforeDispatch,
    ConsumedNotInvoked,
    InvocationUnresolved,
    BindingRecorded,
    RemovalRecorded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RecoveryMetadata {
    NoOperation,
    MissingLegacy,
    Verified {
        cleanup_starts_at: i64,
        cleanup_deadline: i64,
    },
}

/// A journal stage never means current provider presence, absence, readiness,
/// settled billing, remaining approval or permission to create a replacement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VastAttemptStatus {
    pub request_id: String,
    pub operation_id: Option<String>,
    pub stage: AttemptStage,
    pub cancelled: bool,
    pub billing_state: BudgetState,
    pub reserved_micro_usd: u64,
    pub metadata: RecoveryMetadata,
}

/// All collections come from one DEFERRED SQLite snapshot. At most 128 rows in
/// each journal and 64 KiB per JSON record; overflow refuses, never truncates.
pub struct VastAudit {
    pub operations: Vec<CreateOperation>,
    pub budgets: Vec<BudgetReservation>,
    pub removals: Vec<VastRemoval>,
    pub attempts: Vec<VastAttemptStatus>,
}

/// Fixed internal table/column names only. Bounds are checked in SQL before
/// materializing JSON in Rust. This does not bound SQLite engine work/memory.
fn preflight(conn: &Connection, owner: &str) -> Result<()> {
    for (table, fields) in [
        (
            "create_operation",
            &["operation_id", "request_id", "node_uid"][..],
        ),
        ("budget_reservation", &["request_id", "node_uid"][..]),
        ("vast_removal", &["operation_id"][..]),
        ("vast_launch", &["operation_id", "tier"][..]),
    ] {
        let record = if table == "vast_launch" {
            "typeof(canonical)='blob' AND length(canonical) BETWEEN 1 AND 4096".to_owned()
        } else {
            format!(
                "typeof(record)='text' AND length(CAST(record AS BLOB)) BETWEEN 1 AND {MAX_RECORD_BYTES}"
            )
        };
        let mut valid = record;
        for field in fields {
            valid.push_str(&format!(
                " AND typeof({field})='text' AND length(CAST({field} AS BLOB)) BETWEEN 1 AND 256"
            ));
        }
        let (count, invalid): (i64, i64) = conn.query_row(
            &format!("SELECT count(*),coalesce(sum(CASE WHEN {valid} THEN 0 ELSE 1 END),0) FROM (SELECT * FROM {table} WHERE owner=? LIMIT {})", MAX_ROWS + 1),
            [owner],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count > MAX_ROWS as i64 || invalid != 0 {
            return Err(Error::Corrupt);
        }
    }
    Ok(())
}

impl Store {
    /// No provider calls, clock sampling, claim consumption or accounting writes.
    /// Store opening may migrate schema; this query is not an immutable file API.
    pub fn vast_audit(&self) -> Result<VastAudit> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Deferred)?;
        owned(&tx, &self.owner)?;
        let snapshot = self.read_vast_audit(&tx)?;
        tx.commit()?;
        Ok(snapshot)
    }

    fn read_vast_audit(&self, tx: &Transaction<'_>) -> Result<VastAudit> {
        preflight(tx, &self.owner)?;
        let operations = self.create_operations()?;
        let removals = self.vast_removals()?;
        let budgets = read_budgets(tx, &self.owner)?;
        let mut by_request = BTreeMap::new();
        for operation in &operations {
            if by_request
                .insert(operation.intent.request_id.as_str(), operation)
                .is_some()
            {
                return Err(Error::Corrupt);
            }
        }
        // A capsule without a managed operation is corrupt, not ignorable history.
        let orphan: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM vast_launch v LEFT JOIN create_operation c ON c.owner=v.owner AND c.operation_id=v.operation_id WHERE v.owner=? AND c.operation_id IS NULL)",
            [&self.owner], |r| r.get(0),
        )?;
        if orphan {
            return Err(Error::Corrupt);
        }
        let mut attempts = Vec::with_capacity(budgets.len());
        for budget in &budgets {
            let operation = by_request.remove(budget.request.request_id.as_str());
            let (stage, metadata) = if let Some(operation) = operation {
                if operation.budget.request != budget.request
                    || operation.budget.period != budget.period
                    || operation.budget.reserved_at != budget.reserved_at
                    || operation.budget.reserved_all_in != budget.reserved_all_in
                    || operation.budget.reserved_hourly != budget.reserved_hourly
                    || operation.result != budget.binding
                {
                    return Err(Error::Corrupt);
                }
                let has_capsule: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM vast_launch WHERE owner=? AND operation_id=?)",
                    params![self.owner, operation.intent.operation_id],
                    |r| r.get(0),
                )?;
                let metadata = if has_capsule {
                    let recovered =
                        recovery::restore(tx, &self.owner, &operation.intent.operation_id)?;
                    if removals.iter().any(|r| {
                        r.operation_id == operation.intent.operation_id
                            && r.image != recovered.identity().image()
                    }) {
                        return Err(Error::Corrupt);
                    }
                    RecoveryMetadata::Verified {
                        cleanup_starts_at: recovered.cleanup_starts_at()?,
                        cleanup_deadline: recovered.cleanup_deadline()?,
                    }
                } else {
                    RecoveryMetadata::MissingLegacy
                };
                let removed = removals
                    .iter()
                    .find(|r| r.operation_id == operation.intent.operation_id);
                let stage = if let Some(removed) = removed {
                    if !matches!(
                        budget.state,
                        BudgetState::BillingUnknown | BudgetState::Settled
                    ) {
                        return Err(Error::Corrupt);
                    }
                    let retained: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM gpu_node WHERE uid=?)",
                        [&removed.node_uid],
                        |r| r.get(0),
                    )?;
                    if retained {
                        return Err(Error::Corrupt);
                    }
                    AttemptStage::RemovalRecorded
                } else if operation.result.is_some() {
                    AttemptStage::BindingRecorded
                } else if operation.invocation_started_at.is_some() {
                    AttemptStage::InvocationUnresolved
                } else if operation.dispatched_at.is_some() {
                    AttemptStage::ConsumedNotInvoked
                } else if operation.state == CreateState::Cancelled {
                    AttemptStage::CancelledBeforeDispatch
                } else {
                    AttemptStage::Prepared
                };
                (stage, metadata)
            } else {
                if budget.binding.is_some() {
                    return Err(Error::Corrupt);
                }
                (AttemptStage::ReservationOnly, RecoveryMetadata::NoOperation)
            };
            attempts.push(VastAttemptStatus {
                request_id: budget.request.request_id.clone(),
                operation_id: operation.map(|r| r.intent.operation_id.clone()),
                stage,
                cancelled: operation.is_some_and(|r| r.state == CreateState::Cancelled),
                billing_state: budget.state,
                reserved_micro_usd: budget.reserved_all_in,
                metadata,
            });
        }
        if !by_request.is_empty() {
            return Err(Error::Corrupt);
        }
        Ok(VastAudit {
            operations,
            budgets,
            removals,
            attempts,
        })
    }
}

fn read_budgets(conn: &Connection, owner: &str) -> Result<Vec<BudgetReservation>> {
    let mut q = conn.prepare("SELECT request_id,node_uid,record FROM budget_reservation WHERE owner=? ORDER BY request_id")?;
    let rows = q.query_map([owner], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut budgets = Vec::new();
    for row in rows {
        let (request, uid, raw) = row?;
        let budget: BudgetReservation = decode(&raw)?;
        if budget.request.owner != owner
            || budget.request.request_id != request
            || budget.request.node_uid != uid
            || budget.reserved_at < 0
        {
            return Err(Error::Corrupt);
        }
        match (&budget.state, &budget.settlement, &budget.billing_period) {
            (BudgetState::Reserved | BudgetState::Uncertain, None, None) => {}
            (BudgetState::BillingUnknown | BudgetState::Settled, Some(evidence), Some(period)) => {
                if budget.binding.as_ref() != Some(&evidence.binding)
                    || evidence.status != EvidenceStatus::Complete
                    || evidence.resource != ResourceOutcome::OwnedAbsent
                    || evidence.volumes != VolumeOutcome::BoundVolumesAbsent
                    || evidence.observed_at < budget.reserved_at
                    || period.starts_at < 0
                    || period.ends_at <= period.starts_at
                    || !matches!(
                        (&budget.state, &evidence.billing),
                        (BudgetState::BillingUnknown, FinalBilling::Unknown)
                            | (BudgetState::Settled, FinalBilling::Known { .. })
                    )
                {
                    return Err(Error::Corrupt);
                }
            }
            _ => return Err(Error::Corrupt),
        }
        label(&request)?;
        label(&uid)?;
        budgets.push(budget);
    }
    Ok(budgets)
}

#[cfg(test)]
mod tests;
