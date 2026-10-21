//! Durable conservative budget intent, NOT an effect executor or spend approval.
//! Every input is a trusted operation-manager assertion, not provider proof.
use crate::{Error, Result, Store};
use m8s_core::MicroUsd;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub const EVIDENCE_MAX_AGE_SECONDS: i64 = 30;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminBudget {
    pub owner: String,
    pub approval_id: String,
    pub baseline: AccountBaseline,
    pub valid_from: i64,
    pub valid_until: i64,
    pub trial_all_in: MicroUsd,
    pub monthly_all_in: MicroUsd,
    pub total_hourly: MicroUsd,
    pub instance_hourly: MicroUsd,
    pub max_instances: u32,
    pub max_runtime_seconds: u64,
    pub minimum_cleanup_seconds: u64,
    pub minimum_contingency: MicroUsd,
}
/// Explicit trusted initial reconciliation, not universal provider balance proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountBaseline {
    pub prior_lifetime_spent: MicroUsd,
    pub prior_current_period_spent: MicroUsd,
    pub initial_period: CalendarPeriod,
    pub external_liability: ExternalLiability,
    pub observed_at: i64,
    pub status: EvidenceStatus,
    pub provenance: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternalLiability {
    NoneConfirmed,
    KnownReserved(MicroUsd),
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RateUnit {
    MicroUsdPerHour,
    Unknown,
    Unsupported,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TotalUnit {
    MicroUsd,
    Unknown,
    Unsupported,
}
/// Explicit trusted UTC calendar-month interval. No automatic clock-derived reset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarPeriod {
    pub id: String,
    pub starts_at: i64,
    pub ends_at: i64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceStatus {
    Complete,
    Partial,
    Unknown,
    Error,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostQuote {
    pub provenance: String,
    pub observed_at: i64,
    /// Trusted normalized rates and all cost components are bounded through
    /// this time, INCLUSIVE of the full reserved cleanup deadline. Not merely
    /// a deadline for accepting an offer.
    pub valid_until: i64,
    pub status: EvidenceStatus,
    pub compute_unit: RateUnit,
    pub storage_unit: RateUnit,
    pub totals_unit: TotalUnit,
    pub compute_hourly: MicroUsd,
    pub storage_hourly: MicroUsd,
    /// Conservative bounded total transfer charge, not a per-byte rate.
    pub transfer_total: MicroUsd,
    pub runtime_seconds: u64,
    pub cleanup_seconds: u64,
    pub contingency: MicroUsd,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetRequest {
    pub owner: String,
    pub run_id: String,
    pub request_id: String,
    pub node_uid: String,
    pub quote: CostQuote,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VolumeBinding {
    NoneAllocated,
    Ids(Vec<String>),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceBinding {
    pub owner: String,
    pub run_id: String,
    pub node_uid: String,
    pub instance_id: String,
    pub volumes: VolumeBinding,
    pub provenance: String,
    pub observed_at: i64,
    pub status: EvidenceStatus,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceOutcome {
    OwnedAbsent,
    DestroyAcknowledged,
    EmptyList,
    Stopped,
    ProcessDead,
    Unknown,
    Partial,
    Error,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VolumeOutcome {
    BoundVolumesAbsent,
    DeleteAcknowledged,
    EmptyList,
    Unknown,
    Partial,
    Error,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinalBilling {
    Unknown,
    Known {
        actual_all_in: MicroUsd,
        provenance: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementEvidence {
    /// Exact persisted binding, not a fresh caller-selected resource identity.
    pub binding: ResourceBinding,
    pub observed_at: i64,
    pub status: EvidenceStatus,
    pub resource: ResourceOutcome,
    pub volumes: VolumeOutcome,
    pub billing: FinalBilling,
    pub provenance: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BudgetState {
    Reserved,
    Uncertain,
    BillingUnknown,
    Settled,
}
/// Recovery metadata only: receiving this value never authorizes a second create.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetReservation {
    pub request: BudgetRequest,
    pub period: CalendarPeriod,
    pub reserved_all_in: MicroUsd,
    pub reserved_hourly: MicroUsd,
    pub reserved_at: i64,
    pub state: BudgetState,
    pub binding: Option<ResourceBinding>,
    pub settlement: Option<SettlementEvidence>,
    pub billing_period: Option<CalendarPeriod>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetAccount {
    pub policy: AdminBudget,
    pub last_at: i64,
    pub period: CalendarPeriod,
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
fn fresh(status: EvidenceStatus, observed_at: i64, now: i64) -> Result<()> {
    if status != EvidenceStatus::Complete
        || observed_at < 0
        || now < observed_at
        || now - observed_at > EVIDENCE_MAX_AGE_SECONDS
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn period_valid(p: &CalendarPeriod, now: i64) -> Result<()> {
    label(&p.id)?;
    let length = p.ends_at.checked_sub(p.starts_at).ok_or(Error::Invalid)?;
    if p.starts_at < 0
        || !(28 * 86400..=31 * 86400).contains(&length)
        || now < p.starts_at
        || now >= p.ends_at
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn policy_valid(p: &AdminBudget, owner: &str) -> Result<()> {
    label(&p.approval_id)?;
    if p.owner != owner
        || p.valid_from < 0
        || p.valid_until <= p.valid_from
        || p.trial_all_in == 0
        || p.monthly_all_in == 0
        || p.total_hourly == 0
        || p.instance_hourly == 0
        || p.max_instances == 0
        || p.max_runtime_seconds == 0
        || p.minimum_cleanup_seconds == 0
        || p.minimum_contingency == 0
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn amount(v: u128) -> Result<MicroUsd> {
    v.try_into().map_err(|_| Error::Invalid)
}
fn ceil_hourly(rate: MicroUsd, seconds: u64) -> Result<MicroUsd> {
    let product = u128::from(rate)
        .checked_mul(u128::from(seconds))
        .ok_or(Error::Invalid)?;
    amount(product.checked_add(3599).ok_or(Error::Invalid)? / 3600)
}
fn costs(q: &CostQuote, policy: &AdminBudget, now: i64) -> Result<(MicroUsd, MicroUsd)> {
    label(&q.provenance)?;
    fresh(q.status, q.observed_at, now)?;
    if q.compute_unit != RateUnit::MicroUsdPerHour
        || q.storage_unit != RateUnit::MicroUsdPerHour
        || q.totals_unit != TotalUnit::MicroUsd
        || q.valid_until <= now
        || q.valid_until < q.observed_at
        || q.runtime_seconds == 0
        || q.runtime_seconds > policy.max_runtime_seconds
        || q.cleanup_seconds < policy.minimum_cleanup_seconds
        || q.contingency < policy.minimum_contingency
    {
        return Err(Error::Invalid);
    }
    let seconds = q
        .runtime_seconds
        .checked_add(q.cleanup_seconds)
        .ok_or(Error::Invalid)?;
    let deadline = now
        .checked_add(i64::try_from(seconds).map_err(|_| Error::Invalid)?)
        .ok_or(Error::Invalid)?;
    if deadline > q.valid_until {
        return Err(Error::Invalid);
    }
    // Round separate compute/storage components upward; never under-reserve by
    // combining fractional rates or dropping cleanup's continued storage charge.
    let total = [
        ceil_hourly(q.compute_hourly, seconds)?,
        ceil_hourly(q.storage_hourly, seconds)?,
        q.transfer_total,
        q.contingency,
    ]
    .into_iter()
    .try_fold(0u128, |sum, v| {
        sum.checked_add(u128::from(v)).ok_or(Error::Invalid)
    })?;
    // All-in amortized hourly cap includes cleanup, transfer and contingency,
    // normalized over the requested serving runtime (not the longer grace).
    let numerator = total.checked_mul(3600).ok_or(Error::Invalid)?;
    let denominator = u128::from(q.runtime_seconds);
    let hourly = amount(
        numerator
            .checked_add(denominator - 1)
            .ok_or(Error::Invalid)?
            / denominator,
    )?;
    if hourly == 0 || hourly > policy.instance_hourly {
        return Err(Error::Invalid);
    }
    Ok((amount(total)?, hourly))
}
pub(crate) fn initialize(conn: &Connection) -> Result<()> {
    // Same private owner database/connection. No separate ledger file or authority.
    conn.execute_batch("CREATE TABLE IF NOT EXISTS budget_account(owner TEXT PRIMARY KEY,record TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS budget_period(owner TEXT NOT NULL,id TEXT NOT NULL,record TEXT NOT NULL,PRIMARY KEY(owner,id));
        CREATE TABLE IF NOT EXISTS budget_reservation(owner TEXT NOT NULL,request_id TEXT NOT NULL,node_uid TEXT NOT NULL,record TEXT NOT NULL,
            PRIMARY KEY(owner,request_id),UNIQUE(owner,node_uid));")?;
    Ok(())
}
fn account(tx: &Connection, owner: &str) -> Result<Option<BudgetAccount>> {
    let row: Option<String> = tx
        .query_row(
            "SELECT record FROM budget_account WHERE owner=?",
            [owner],
            |r| r.get(0),
        )
        .optional()?;
    row.map(|s| decode(&s)).transpose()
}
fn rows(conn: &Connection, owner: &str) -> Result<Vec<BudgetReservation>> {
    let mut q =
        conn.prepare("SELECT record FROM budget_reservation WHERE owner=? ORDER BY request_id")?;
    q.query_map([owner], |r| r.get::<_, String>(0))?
        .map(|r| decode(&r?))
        .collect()
}
fn clock(a: &BudgetAccount, now: i64) -> Result<()> {
    if now < 0 || now < a.last_at {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn advance(a: &mut BudgetAccount, p: &CalendarPeriod, now: i64) -> Result<()> {
    clock(a, now)?;
    period_valid(p, now)?;
    if a.period != *p && (p.id == a.period.id || p.starts_at < a.period.ends_at) {
        return Err(Error::Invalid);
    }
    a.period = p.clone();
    a.last_at = now;
    Ok(())
}
fn register_period(tx: &Transaction<'_>, owner: &str, p: &CalendarPeriod) -> Result<()> {
    let prior: Option<String> = tx
        .query_row(
            "SELECT record FROM budget_period WHERE owner=? AND id=?",
            params![owner, p.id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(prior) = prior {
        if decode::<CalendarPeriod>(&prior)? != *p {
            return Err(Error::Invalid);
        }
    } else {
        tx.execute(
            "INSERT INTO budget_period(owner,id,record) VALUES (?,?,?)",
            params![owner, p.id, encode(p)?],
        )?;
    }
    Ok(())
}

fn save_account(tx: &Transaction<'_>, a: &BudgetAccount) -> Result<()> {
    tx.execute("INSERT INTO budget_account(owner,record) VALUES (?,?) ON CONFLICT(owner) DO UPDATE SET record=excluded.record",params![a.policy.owner,encode(a)?])?;
    Ok(())
}
fn save_row(tx: &Transaction<'_>, r: &BudgetReservation) -> Result<()> {
    tx.execute(
        "UPDATE budget_reservation SET record=? WHERE owner=? AND request_id=?",
        params![encode(r)?, r.request.owner, r.request.request_id],
    )?;
    Ok(())
}
fn owned(tx: &Transaction<'_>, owner: &str) -> Result<()> {
    let yes: bool = tx.query_row(
        "SELECT count(*)=1 AND min(owner)=? FROM pool",
        [owner],
        |r| r.get(0),
    )?;
    if !yes {
        return Err(Error::OwnerMismatch);
    }
    Ok(())
}
fn get_row(tx: &Transaction<'_>, owner: &str, request_id: &str) -> Result<BudgetReservation> {
    let s: String = tx.query_row(
        "SELECT record FROM budget_reservation WHERE owner=? AND request_id=?",
        params![owner, request_id],
        |r| r.get(0),
    )?;
    let r: BudgetReservation = decode(&s)?;
    if r.request.owner != owner || r.request.request_id != request_id {
        return Err(Error::Corrupt);
    }
    Ok(r)
}
impl Store {
    /// Persist the explicit initial reconciliation once, even when unknown debt
    /// or already-exceeded caps mean no new reservation can be admitted.
    pub fn initialize_budget(
        &self,
        policy: &AdminBudget,
        period: &CalendarPeriod,
        now: i64,
    ) -> Result<()> {
        policy_valid(policy, &self.owner)?;
        period_valid(period, now)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        owned(&tx, &self.owner)?;
        if let Some(a) = account(&tx, &self.owner)? {
            clock(&a, now)?;
            return if a.policy == *policy {
                Ok(())
            } else {
                Err(Error::Invalid)
            };
        }
        // A missing account with existing accounting rows is corrupt, not a new
        // approval epoch. No repair/reset is implicit in initialization.
        if !rows(&tx, &self.owner)?.is_empty() {
            return Err(Error::Corrupt);
        }
        if policy.baseline.initial_period != *period
            || policy.baseline.prior_current_period_spent > policy.baseline.prior_lifetime_spent
        {
            return Err(Error::Invalid);
        }
        label(&policy.baseline.provenance)?;
        fresh(policy.baseline.status, policy.baseline.observed_at, now)?;
        if policy.baseline.external_liability == ExternalLiability::KnownReserved(0) {
            return Err(Error::Invalid);
        }
        register_period(&tx, &self.owner, period)?;
        save_account(
            &tx,
            &BudgetAccount {
                policy: policy.clone(),
                period: period.clone(),
                last_at: now,
            },
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn budget_account(&self) -> Result<Option<BudgetAccount>> {
        account(&self.conn, &self.owner)
    }
    pub fn reserve_budget(
        &self,
        policy: &AdminBudget,
        period: &CalendarPeriod,
        request: &BudgetRequest,
        now: i64,
    ) -> Result<BudgetReservation> {
        policy_valid(policy, &self.owner)?;
        period_valid(period, now)?;
        if now < policy.valid_from || now >= policy.valid_until {
            return Err(Error::Invalid);
        }
        for v in [&request.run_id, &request.request_id, &request.node_uid] {
            label(v)?;
        }
        if request.owner != self.owner {
            return Err(Error::OwnerMismatch);
        }
        self.initialize_budget(policy, period, now)?;
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        owned(&tx, &self.owner)?;
        let mut a = account(&tx, &self.owner)?.ok_or(Error::Corrupt)?;
        clock(&a, now)?;
        if a.policy != *policy {
            return Err(Error::Invalid);
        }
        let previous: Option<String> = tx
            .query_row(
                "SELECT record FROM budget_reservation WHERE owner=? AND request_id=?",
                params![self.owner, request.request_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            let old: BudgetReservation = decode(&previous)?;
            if old.request != *request || old.period != *period {
                return Err(Error::Invalid);
            }
            // Metadata recovery, not a new launch permit; deliberately no debit,
            // clock update, or quote renewal on exact idempotent retries.
            return Ok(old);
        }
        register_period(&tx, &self.owner, period)?;
        advance(&mut a, period, now)?;
        let (reserved, hourly) = costs(&request.quote, policy, now)?;
        let horizon = request
            .quote
            .runtime_seconds
            .checked_add(request.quote.cleanup_seconds)
            .ok_or(Error::Invalid)?;
        let deadline = now
            .checked_add(i64::try_from(horizon).map_err(|_| Error::Invalid)?)
            .ok_or(Error::Invalid)?;
        if deadline > policy.valid_until {
            return Err(Error::Invalid);
        }
        // Initial known spend retains lifetime liability; the explicitly bound
        // initial-period spend is counted only in that same calendar period.
        // Nonzero/unknown external liabilities remain durably blocking until a
        // future explicit trusted reconciliation/migration operation exists.
        if policy.baseline.external_liability != ExternalLiability::NoneConfirmed {
            return Err(Error::Invalid);
        }
        let mut lifetime = u128::from(reserved)
            .checked_add(u128::from(policy.baseline.prior_lifetime_spent))
            .ok_or(Error::Invalid)?;
        let initial_period_spent = if policy.baseline.initial_period == *period {
            policy.baseline.prior_current_period_spent
        } else {
            0
        };
        let mut monthly = u128::from(reserved)
            .checked_add(u128::from(initial_period_spent))
            .ok_or(Error::Invalid)?;
        let mut active_hourly = u128::from(hourly);
        let mut active = 1u64;
        for r in rows(&tx, &self.owner)? {
            if r.request.node_uid == request.node_uid {
                return Err(Error::Invalid);
            }
            if r.state == BudgetState::Settled {
                let Some(SettlementEvidence {
                    billing: FinalBilling::Known { actual_all_in, .. },
                    ..
                }) = r.settlement
                else {
                    return Err(Error::Corrupt);
                };
                lifetime = lifetime
                    .checked_add(u128::from(actual_all_in))
                    .ok_or(Error::Invalid)?;
                if r.billing_period.as_ref() == Some(period) {
                    monthly = monthly
                        .checked_add(u128::from(actual_all_in))
                        .ok_or(Error::Invalid)?;
                }
            } else {
                let horizon = r
                    .request
                    .quote
                    .runtime_seconds
                    .checked_add(r.request.quote.cleanup_seconds)
                    .ok_or(Error::Corrupt)?;
                let deadline = r
                    .reserved_at
                    .checked_add(i64::try_from(horizon).map_err(|_| Error::Corrupt)?)
                    .ok_or(Error::Corrupt)?;
                if r.state != BudgetState::Reserved || now >= deadline {
                    // An ambiguous effect, unfinalized billing, or missed cleanup
                    // deadline freezes fresh spend, even if numeric caps have room.
                    return Err(Error::Invalid);
                }
                lifetime = lifetime
                    .checked_add(u128::from(r.reserved_all_in))
                    .ok_or(Error::Invalid)?;
                monthly = monthly
                    .checked_add(u128::from(r.reserved_all_in))
                    .ok_or(Error::Invalid)?;
                active_hourly = active_hourly
                    .checked_add(u128::from(r.reserved_hourly))
                    .ok_or(Error::Invalid)?;
                active = active.checked_add(1).ok_or(Error::Invalid)?;
            }
        }
        if lifetime > u128::from(policy.trial_all_in)
            || monthly > u128::from(policy.monthly_all_in)
            || active_hourly > u128::from(policy.total_hourly)
            || active > u64::from(policy.max_instances)
        {
            return Err(Error::Invalid);
        }
        let r = BudgetReservation {
            request: request.clone(),
            period: period.clone(),
            reserved_all_in: reserved,
            reserved_hourly: hourly,
            reserved_at: now,
            state: BudgetState::Reserved,
            binding: None,
            settlement: None,
            billing_period: None,
        };
        tx.execute(
            "INSERT INTO budget_reservation(owner,request_id,node_uid,record) VALUES (?,?,?,?)",
            params![
                self.owner,
                request.request_id,
                request.node_uid,
                encode(&r)?
            ],
        )?;
        save_account(&tx, &a)?;
        tx.commit()?;
        Ok(r)
    }
    pub fn budget_reservations(&self) -> Result<Vec<BudgetReservation>> {
        rows(&self.conn, &self.owner)
    }
    pub fn mark_budget_uncertain(&self, request_id: &str, now: i64) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        owned(&tx, &self.owner)?;
        let mut a = account(&tx, &self.owner)?.ok_or(Error::Invalid)?;
        clock(&a, now)?;
        let mut r = get_row(&tx, &self.owner, request_id)?;
        if r.state == BudgetState::Settled {
            return Err(Error::Invalid);
        }
        if r.state != BudgetState::Reserved {
            return Ok(());
        }
        r.state = BudgetState::Uncertain;
        a.last_at = now;
        save_row(&tx, &r)?;
        save_account(&tx, &a)?;
        tx.commit()?;
        Ok(())
    }
    /// One immutable trusted resource binding after create/recovery; uncertainty
    /// remains fully reserved. Never infer no-volume from a missing list field.
    pub fn bind_budget_resources(
        &self,
        request_id: &str,
        binding: &ResourceBinding,
        now: i64,
    ) -> Result<()> {
        fresh(binding.status, binding.observed_at, now)?;
        for v in [&binding.instance_id, &binding.provenance] {
            label(v)?;
        }
        if let VolumeBinding::Ids(ids) = &binding.volumes {
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
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        owned(&tx, &self.owner)?;
        let mut a = account(&tx, &self.owner)?.ok_or(Error::Invalid)?;
        clock(&a, now)?;
        let mut r = get_row(&tx, &self.owner, request_id)?;
        if binding.owner != self.owner
            || binding.run_id != r.request.run_id
            || binding.node_uid != r.request.node_uid
        {
            return Err(Error::Invalid);
        }
        if let Some(old) = r.binding {
            return if old == *binding {
                Ok(())
            } else {
                Err(Error::Invalid)
            };
        }
        if r.state == BudgetState::Settled {
            return Err(Error::Invalid);
        }
        r.binding = Some(binding.clone());
        a.last_at = now;
        save_row(&tx, &r)?;
        save_account(&tx, &a)?;
        tx.commit()?;
        Ok(())
    }
    pub fn settle_budget(
        &self,
        request_id: &str,
        period: &CalendarPeriod,
        evidence: &SettlementEvidence,
        now: i64,
    ) -> Result<BudgetReservation> {
        fresh(evidence.status, evidence.observed_at, now)?;
        label(&evidence.provenance)?;
        if evidence.resource != ResourceOutcome::OwnedAbsent
            || evidence.volumes != VolumeOutcome::BoundVolumesAbsent
        {
            return Err(Error::Invalid);
        }
        if let FinalBilling::Known { provenance, .. } = &evidence.billing {
            label(provenance)?;
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        owned(&tx, &self.owner)?;
        let mut a = account(&tx, &self.owner)?.ok_or(Error::Invalid)?;
        clock(&a, now)?;
        let mut r = get_row(&tx, &self.owner, request_id)?;
        if r.binding.as_ref() != Some(&evidence.binding) {
            return Err(Error::Invalid);
        }
        if r.state == BudgetState::Settled || r.settlement.as_ref() == Some(evidence) {
            return if r.settlement.as_ref() == Some(evidence)
                && r.billing_period.as_ref() == Some(period)
            {
                Ok(r)
            } else {
                Err(Error::Invalid)
            };
        }
        register_period(&tx, &self.owner, period)?;
        advance(&mut a, period, now)?;
        r.state = match evidence.billing {
            FinalBilling::Unknown => BudgetState::BillingUnknown,
            FinalBilling::Known { .. } => BudgetState::Settled,
        };
        r.settlement = Some(evidence.clone());
        r.billing_period = Some(period.clone());
        // Final actual may exceed quote/caps: record the debt, never reject or
        // truncate it to make future capacity appear available.
        save_row(&tx, &r)?;
        save_account(&tx, &a)?;
        tx.commit()?;
        Ok(r)
    }
}
#[cfg(test)]
mod tests;
