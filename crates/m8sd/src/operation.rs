//! Durable one-use create dispatch intent. No provider/executor or transport proof.
//! Inputs and launch fingerprints are trusted operation-manager assertions.
use crate::{
    Error, Result, Store,
    budget::{
        BudgetAccount, BudgetReservation, BudgetState, EVIDENCE_MAX_AGE_SECONDS, EvidenceStatus,
        ResourceBinding, VolumeBinding,
    },
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
struct DriverOrigin;
pub mod development;
#[cfg(all(feature = "local-development", unix))]
pub mod local_development;
#[cfg(all(feature = "prototype-vast", unix))]
pub mod vast;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateIntent {
    pub owner: String,
    pub run_id: String,
    pub request_id: String,
    pub node_uid: String,
    pub operation_id: String,
    /// Exact canonical nonsecret launch specification fingerprint; not approval,
    /// signed-image validation or proof that a future driver dispatches it.
    pub launch_fingerprint: [u8; 32],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreateState {
    Prepared,
    DispatchConsumed,
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateOperation {
    pub intent: CreateIntent,
    pub state: CreateState,
    pub prepared_at: i64,
    pub last_at: i64,
    pub dispatched_at: Option<i64>,
    #[serde(default)]
    pub invocation_started_at: Option<i64>,
    pub cancelled_at: Option<i64>,
    pub cleanup_required: bool,
    pub result: Option<ResourceBinding>,
    pub uncertainty_observed: bool,
    // Immutable original reservation: recovery metadata, never effect permission.
    budget: BudgetReservation,
}
/// Only committed Prepared -> DispatchConsumed can mint this value. No public
/// constructor, Clone, Debug, Serialize or Deserialize; recovery never remints.
/// A future trusted executor must take it BY VALUE and enforce the anchored
/// deadline, not restart the full rental runtime. This is NOT transport proof.
/// ```compile_fail
/// use m8sd::operation::CreateClaim;
/// fn twice(claim: CreateClaim) {
///     fn take(_: CreateClaim) {}
///     take(claim);
///     take(claim);
/// }
/// ```
pub struct CreateClaim {
    origin: Arc<crate::StoreOrigin>,
    driver: Option<Arc<DriverOrigin>>,
    dispatched_at: i64,
    intent: CreateIntent,
    serving_deadline: i64,
    cleanup_deadline: i64,
}
impl CreateClaim {
    pub fn intent(&self) -> &CreateIntent {
        &self.intent
    }
    pub fn serving_deadline(&self) -> i64 {
        self.serving_deadline
    }
    pub fn cleanup_deadline(&self) -> i64 {
        self.cleanup_deadline
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateObservation {
    /// Complete fresh exact owner/run/UID/resource binding, NOT provider proof.
    Bound(ResourceBinding),
    /// Timeout, error, lost response, ACK or incomplete observation; never absence.
    Ambiguous,
}
fn encode<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|_| Error::Invalid)
}
fn decode<T: for<'a> Deserialize<'a>>(v: &str) -> Result<T> {
    serde_json::from_str(v).map_err(|_| Error::Corrupt)
}
fn label(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 256 || s.chars().any(char::is_control) {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub(crate) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS create_operation(owner TEXT NOT NULL,operation_id TEXT NOT NULL,request_id TEXT NOT NULL,node_uid TEXT NOT NULL,record TEXT NOT NULL,
        PRIMARY KEY(owner,operation_id),UNIQUE(owner,request_id),UNIQUE(owner,node_uid));
        CREATE TABLE IF NOT EXISTS vast_removal(owner TEXT NOT NULL,operation_id TEXT NOT NULL,record TEXT NOT NULL,
        PRIMARY KEY(owner,operation_id));
        CREATE TABLE IF NOT EXISTS vast_launch(owner TEXT NOT NULL,operation_id TEXT NOT NULL,tier TEXT NOT NULL,canonical BLOB NOT NULL,
        PRIMARY KEY(owner,operation_id));")?;
    Ok(())
}
fn owned(tx: &Transaction<'_>, owner: &str) -> Result<()> {
    let owns: bool = tx.query_row(
        "SELECT count(*)=1 AND min(owner)=? FROM pool",
        [owner],
        |r| r.get(0),
    )?;
    if !owns {
        return Err(Error::OwnerMismatch);
    }
    Ok(())
}
fn account(tx: &Transaction<'_>, owner: &str, now: i64) -> Result<BudgetAccount> {
    crate::execution_domain::claim(tx, owner, crate::execution_domain::Domain::Managed)?;
    let json: String = tx.query_row(
        "SELECT record FROM budget_account WHERE owner=?",
        [owner],
        |r| r.get(0),
    )?;
    let a: BudgetAccount = decode(&json)?;
    if a.policy.owner != owner || a.last_at < 0 || now < 0 || now < a.last_at {
        return Err(Error::Invalid);
    }
    Ok(a)
}
fn budget(tx: &Connection, intent: &CreateIntent) -> Result<BudgetReservation> {
    let (uid, json): (String, String) = tx.query_row(
        "SELECT node_uid,record FROM budget_reservation WHERE owner=? AND request_id=?",
        params![intent.owner, intent.request_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let b: BudgetReservation = decode(&json)?;
    if uid != b.request.node_uid
        || b.request.owner != intent.owner
        || b.request.request_id != intent.request_id
    {
        return Err(Error::Corrupt);
    }
    if b.request.node_uid != intent.node_uid || b.request.run_id != intent.run_id {
        return Err(Error::Invalid);
    }
    Ok(b)
}
fn get(tx: &Connection, owner: &str, id: &str) -> Result<Option<CreateOperation>> {
    let row:Option<(String,String,String)>=tx.query_row("SELECT request_id,node_uid,record FROM create_operation WHERE owner=? AND operation_id=?",params![owner,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    row.map(|(request, uid, json)| {
        let r: CreateOperation = decode(&json)?;
        if r.intent.owner != owner
            || r.intent.operation_id != id
            || r.intent.request_id != request
            || r.intent.node_uid != uid
            || r.prepared_at < 0
            || r.last_at < r.prepared_at
            || (r.state == CreateState::Prepared
                && (r.dispatched_at.is_some() || r.cancelled_at.is_some()))
            || (r.state == CreateState::DispatchConsumed && r.dispatched_at.is_none())
            || (r.state == CreateState::Cancelled && r.cancelled_at.is_none())
            || r.invocation_started_at
                .is_some_and(|at| r.dispatched_at.is_none_or(|start| at < start) || at > r.last_at)
            || (r.state == CreateState::Prepared && r.invocation_started_at.is_some())
            || (r.state == CreateState::Cancelled
                && r.invocation_started_at
                    .zip(r.cancelled_at)
                    .is_some_and(|(start, cancel)| start > cancel))
        {
            return Err(Error::Corrupt);
        }
        if r.budget.request.owner != r.intent.owner
            || r.budget.request.request_id != r.intent.request_id
            || r.budget.request.run_id != r.intent.run_id
            || r.budget.request.node_uid != r.intent.node_uid
            || r.dispatched_at
                .is_some_and(|at| at < r.prepared_at || at > r.last_at)
            || r.cancelled_at
                .is_some_and(|at| at < r.prepared_at || at > r.last_at)
            || (r.dispatched_at.is_none()
                && (r.result.is_some() || r.uncertainty_observed || r.cleanup_required))
            || (r.dispatched_at.is_some() && !r.cleanup_required)
            || (r.state == CreateState::DispatchConsumed && r.cancelled_at.is_some())
        {
            return Err(Error::Corrupt);
        }
        if let Some(binding) = &r.result {
            let at = r.dispatched_at.ok_or(Error::Corrupt)?;
            if binding.observed_at < at || binding.observed_at > r.last_at {
                return Err(Error::Corrupt);
            }
            binding_valid(binding, &r.intent, binding.observed_at).map_err(|_| Error::Corrupt)?;
        }
        Ok(r)
    })
    .transpose()
}
fn save(tx: &Transaction<'_>, r: &CreateOperation) -> Result<()> {
    if tx.execute(
        "UPDATE create_operation SET record=? WHERE owner=? AND operation_id=?",
        params![encode(r)?, r.intent.owner, r.intent.operation_id],
    )? != 1
    {
        return Err(Error::Corrupt);
    }
    Ok(())
}
fn save_budget(
    tx: &Transaction<'_>,
    b: &BudgetReservation,
    a: &mut BudgetAccount,
    now: i64,
) -> Result<()> {
    a.last_at = now;
    if tx.execute(
        "UPDATE budget_reservation SET record=? WHERE owner=? AND request_id=?",
        params![encode(b)?, b.request.owner, b.request.request_id],
    )? != 1
    {
        return Err(Error::Corrupt);
    }
    if tx.execute(
        "UPDATE budget_account SET record=? WHERE owner=?",
        params![encode(a)?, a.policy.owner],
    )? != 1
    {
        return Err(Error::Corrupt);
    }
    Ok(())
}
fn live_launch(tx: &Transaction<'_>, intent: &CreateIntent, now: i64) -> Result<()> {
    let yes:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM gpu_node n JOIN tier t ON t.name=n.tier WHERE n.uid=? AND n.phase='\"Launching\"' AND n.instance_id IS NULL AND n.deleting=0 AND n.created_at>=0 AND n.created_at<=? AND t.desired>0)",params![intent.node_uid,now],|r|r.get(0))?;
    if !yes {
        return Err(Error::Invalid);
    }
    Ok(())
}
enum InvocationClock {
    Explicit(i64),
    #[cfg(all(any(feature = "local-development", feature = "prototype-vast"), unix))]
    LocalUtc,
}
// Shared internal invocation burn. No caller-supplied code or effects in TX.
fn start_bound_invocation(
    store: &Store,
    claim: &CreateClaim,
    clock: InvocationClock,
    tier: &str,
    #[cfg(test)] fail_before_commit: bool,
) -> Result<bool> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    // Local wall time sampled AFTER obtaining the SQL writer, not before a
    // possibly-blocking BEGIN; Explicit keeps existing synthetic semantics.
    #[cfg(all(any(feature = "local-development", feature = "prototype-vast"), unix))]
    let now = match clock {
        InvocationClock::Explicit(now) => now,
        InvocationClock::LocalUtc => wall_utc()?,
    };
    #[cfg(not(all(any(feature = "local-development", feature = "prototype-vast"), unix)))]
    let InvocationClock::Explicit(now) = clock;
    owned(&tx, &store.owner)?;
    let mut a = account(&tx, &store.owner, now)?;
    let mut r = get(&tx, &store.owner, &claim.intent.operation_id)?.ok_or(Error::Invalid)?;
    if r.intent != claim.intent || r.dispatched_at != Some(claim.dispatched_at) || now < r.last_at {
        return Err(Error::Invalid);
    }
    if r.state == CreateState::Cancelled {
        return Ok(false);
    }
    if r.state != CreateState::DispatchConsumed
        || r.result.is_some()
        || r.uncertainty_observed
        || r.invocation_started_at.is_some()
    {
        return Err(Error::Invalid);
    }
    let b = budget(&tx, &r.intent)?;
    let deadlines = crate::budget::validate_consumed_create(&tx, &r.budget, &b, &a, now)?;
    if deadlines != (claim.serving_deadline, claim.cleanup_deadline) {
        return Err(Error::Invalid);
    }
    live_launch(&tx, &r.intent, now)?;
    let actual: String = tx.query_row(
        "SELECT tier FROM gpu_node WHERE uid=?",
        [&r.intent.node_uid],
        |r| r.get(0),
    )?;
    if actual != tier {
        return Err(Error::Invalid);
    }
    r.invocation_started_at = Some(now);
    r.last_at = now;
    save(&tx, &r)?;
    save_budget(&tx, &b, &mut a, now)?;
    #[cfg(test)]
    if fail_before_commit {
        return Err(Error::Invalid);
    }
    tx.commit()?;
    Ok(true)
}

#[cfg(all(any(feature = "local-development", feature = "prototype-vast"), unix))]
fn wall_utc() -> Result<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .ok_or(Error::Invalid)
}
fn binding_valid(b: &ResourceBinding, intent: &CreateIntent, now: i64) -> Result<()> {
    if b.owner != intent.owner
        || b.run_id != intent.run_id
        || b.node_uid != intent.node_uid
        || b.status != EvidenceStatus::Complete
        || b.observed_at < 0
        || now < b.observed_at
        || now - b.observed_at > EVIDENCE_MAX_AGE_SECONDS
    {
        return Err(Error::Invalid);
    }
    label(&b.instance_id)?;
    label(&b.provenance)?;
    if let VolumeBinding::Ids(ids) = &b.volumes {
        if ids.is_empty() || ids.len() > 128 {
            return Err(Error::Invalid);
        }
        for (i, id) in ids.iter().enumerate() {
            label(id)?;
            if ids[..i].contains(id) {
                return Err(Error::Invalid);
            }
        }
    }
    Ok(())
}
fn validate_prepare_identity(owner: &str, intent: &CreateIntent) -> Result<()> {
    for s in [
        &intent.owner,
        &intent.run_id,
        &intent.request_id,
        &intent.node_uid,
        &intent.operation_id,
    ] {
        label(s)?;
    }
    if intent.owner != owner {
        return Err(Error::OwnerMismatch);
    }
    Ok(())
}
// Shared exact existing public prepare transaction; no nested BEGIN/effects.
// bool distinguishes a new row from metadata-only retry (no clock renewal).
fn prepare_create_tx(
    tx: &Transaction<'_>,
    owner: &str,
    intent: &CreateIntent,
    now: i64,
) -> Result<(CreateOperation, bool)> {
    validate_prepare_identity(owner, intent)?;
    owned(tx, owner)?;
    let mut a = account(tx, owner, now)?;
    if let Some(r) = get(tx, owner, &intent.operation_id)? {
        if r.intent != *intent || now < r.last_at {
            return Err(Error::Invalid);
        }
        return Ok((r, false));
    }
    let b = budget(tx, intent)?;
    crate::budget::validate_create_dispatch(tx, &b, &a, now)?;
    live_launch(tx, intent, now)?;
    let r = CreateOperation {
        intent: intent.clone(),
        state: CreateState::Prepared,
        prepared_at: now,
        last_at: now,
        dispatched_at: None,
        invocation_started_at: None,
        cancelled_at: None,
        cleanup_required: false,
        result: None,
        uncertainty_observed: false,
        budget: b,
    };
    tx.execute("INSERT INTO create_operation(owner,operation_id,request_id,node_uid,record)VALUES(?,?,?,?,?)",params![owner,intent.operation_id,intent.request_id,intent.node_uid,encode(&r)?])?;
    a.last_at = now;
    tx.execute(
        "UPDATE budget_account SET record=? WHERE owner=?",
        params![encode(&a)?, owner],
    )?;
    Ok((r, true))
}
impl Store {
    pub fn prepare_create(&self, intent: &CreateIntent, now: i64) -> Result<CreateOperation> {
        validate_prepare_identity(&self.owner, intent)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let (r, inserted) = prepare_create_tx(&tx, &self.owner, intent, now)?;
        if inserted {
            tx.commit()?;
        }
        Ok(r)
    }
    pub fn claim_create(&self, operation_id: &str, now: i64) -> Result<CreateClaim> {
        self.claim_create_bound(operation_id, now, None)
    }
    fn claim_create_bound(
        &self,
        operation_id: &str,
        now: i64,
        driver: Option<(Arc<DriverOrigin>, [u8; 32], &str)>,
    ) -> Result<CreateClaim> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        owned(&tx, &self.owner)?;
        let mut a = account(&tx, &self.owner, now)?;
        let mut r = get(&tx, &self.owner, operation_id)?.ok_or(Error::Invalid)?;
        if r.state != CreateState::Prepared
            || r.dispatched_at.is_some()
            || r.cancelled_at.is_some()
            || now < r.last_at
        {
            return Err(Error::Invalid);
        }
        if let Some((_, fingerprint, tier)) = &driver {
            if r.intent.launch_fingerprint != *fingerprint {
                return Err(Error::Invalid);
            }
            let actual: String = tx.query_row(
                "SELECT tier FROM gpu_node WHERE uid=?",
                [&r.intent.node_uid],
                |r| r.get(0),
            )?;
            if actual != *tier {
                return Err(Error::Invalid);
            }
        }
        let mut b = budget(&tx, &r.intent)?;
        if b != r.budget {
            return Err(Error::Invalid);
        }
        let (serving_deadline, cleanup_deadline) =
            crate::budget::validate_create_dispatch(&tx, &b, &a, now)?;
        live_launch(&tx, &r.intent, now)?;
        // Recheck all current costs/unknown liabilities, including bills that
        // settled after preparation. An old reservation is not a stale cap grant.
        b.state = BudgetState::Uncertain;
        r.state = CreateState::DispatchConsumed;
        r.dispatched_at = Some(now);
        r.last_at = now;
        r.cleanup_required = true;
        save(&tx, &r)?;
        save_budget(&tx, &b, &mut a, now)?;
        tx.commit()?;
        Ok(CreateClaim {
            origin: self.origin.clone(),
            driver: driver.map(|(token, _, _)| token),
            dispatched_at: now,
            intent: r.intent,
            serving_deadline,
            cleanup_deadline,
        })
    }
    /// Explicit cancellation only. Existing down/deleting flags separately gate
    /// claim; no executor/reconcile integration or refund is implied here.
    pub fn cancel_create(&self, operation_id: &str, now: i64) -> Result<CreateOperation> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        owned(&tx, &self.owner)?;
        let mut a = account(&tx, &self.owner, now)?;
        let mut r = get(&tx, &self.owner, operation_id)?.ok_or(Error::Invalid)?;
        if now < r.last_at {
            return Err(Error::Invalid);
        }
        if r.state == CreateState::Cancelled {
            return Ok(r);
        }
        let mut b = budget(&tx, &r.intent)?;
        if b.state == BudgetState::Settled {
            return Err(Error::Invalid);
        }
        r.state = CreateState::Cancelled;
        r.cancelled_at = Some(now);
        r.last_at = now;
        r.cleanup_required = r.dispatched_at.is_some();
        if b.state == BudgetState::Reserved {
            b.state = BudgetState::Uncertain;
        }
        save(&tx, &r)?;
        save_budget(&tx, &b, &mut a, now)?;
        tx.commit()?;
        Ok(r)
    }
    pub fn record_create_result(
        &self,
        operation_id: &str,
        observation: &CreateObservation,
        now: i64,
    ) -> Result<CreateOperation> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        owned(&tx, &self.owner)?;
        let mut a = account(&tx, &self.owner, now)?;
        let mut r = get(&tx, &self.owner, operation_id)?.ok_or(Error::Invalid)?;
        if r.dispatched_at.is_none() || now < r.last_at {
            return Err(Error::Invalid);
        }
        let mut b = budget(&tx, &r.intent)?;
        if b.state == BudgetState::Settled {
            return Err(Error::Invalid);
        }
        match observation {
            CreateObservation::Ambiguous => {
                if r.uncertainty_observed {
                    return Ok(r);
                }
                r.uncertainty_observed = true;
            }
            CreateObservation::Bound(binding) => {
                binding_valid(binding, &r.intent, now)?;
                if binding.observed_at < r.dispatched_at.ok_or(Error::Invalid)? {
                    return Err(Error::Invalid);
                }
                if r.result.as_ref().is_some_and(|old| old != binding)
                    || b.binding.as_ref().is_some_and(|old| old != binding)
                {
                    return Err(Error::Invalid);
                }
                if r.result.as_ref() == Some(binding) && b.binding.as_ref() == Some(binding) {
                    return Ok(r);
                }
                r.result = Some(binding.clone());
                b.binding = Some(binding.clone());
            }
        }
        if b.state == BudgetState::Reserved {
            b.state = BudgetState::Uncertain;
        }
        r.cleanup_required = true;
        r.last_at = now;
        save(&tx, &r)?;
        save_budget(&tx, &b, &mut a, now)?;
        tx.commit()?;
        Ok(r)
    }
    pub fn create_operations(&self) -> Result<Vec<CreateOperation>> {
        let mut q = self.conn.prepare(
            "SELECT operation_id FROM create_operation WHERE owner=? ORDER BY operation_id",
        )?;
        let ids = q
            .query_map([&self.owner], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| get(&self.conn, &self.owner, &id)?.ok_or(Error::Corrupt))
            .collect()
    }
}
#[cfg(test)]
mod tests;
