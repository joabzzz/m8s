//! Sticky execution-domain selection, serialized with budget initialization.
//! Prevents a mock reconciler racing a managed coordinator into one Store.
use crate::{Error, Result, Store};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Domain {
    Unmanaged,
    Managed,
}
impl Domain {
    fn text(self) -> &'static str {
        match self {
            Self::Unmanaged => "unmanaged",
            Self::Managed => "managed",
        }
    }
}

pub(crate) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS execution_domain(owner TEXT PRIMARY KEY,mode TEXT NOT NULL CHECK(mode IN ('unmanaged','managed')));")?;
    Ok(())
}

fn recorded(conn: &Connection, owner: &str) -> Result<Option<Domain>> {
    let count: i64 = conn.query_row("SELECT count(*) FROM execution_domain", [], |r| r.get(0))?;
    let value: Option<String> = conn
        .query_row(
            "SELECT mode FROM execution_domain WHERE owner=?",
            [owner],
            |r| r.get(0),
        )
        .optional()?;
    match (count, value.as_deref()) {
        (0, None) => Ok(None),
        (1, Some("unmanaged")) => Ok(Some(Domain::Unmanaged)),
        (1, Some("managed")) => Ok(Some(Domain::Managed)),
        _ => Err(Error::Corrupt),
    }
}

fn managed_history(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM create_operation) OR EXISTS(SELECT 1 FROM budget_account)
        OR EXISTS(SELECT 1 FROM budget_reservation) OR EXISTS(SELECT 1 FROM budget_period)
        OR EXISTS(SELECT 1 FROM vast_launch)
        OR EXISTS(SELECT 1 FROM vast_removal)",
        [],
        |r| r.get(0),
    )?)
}

/// A missing account row does not erase prior domain/accounting history.
pub(crate) fn managed_initialized(conn: &Connection, owner: &str) -> Result<bool> {
    Ok(recorded(conn, owner)? == Some(Domain::Managed) || managed_history(conn)?)
}

pub(crate) fn require_unmanaged(conn: &Connection, owner: &str) -> Result<()> {
    if recorded(conn, owner)? == Some(Domain::Managed) || managed_history(conn)? {
        return Err(Error::Invalid);
    }
    Ok(())
}

pub(crate) fn claim(tx: &Transaction<'_>, owner: &str, desired: Domain) -> Result<()> {
    let owns: bool = tx.query_row(
        "SELECT count(*)=1 AND min(owner)=? FROM pool",
        [owner],
        |r| r.get(0),
    )?;
    if !owns {
        return Err(Error::OwnerMismatch);
    }
    let prior = recorded(tx, owner)?;
    let managed = managed_history(tx)?;
    if prior.is_some_and(|prior| prior != desired) || (desired == Domain::Unmanaged && managed) {
        return Err(Error::Invalid);
    }
    // An old mock database with nodes/attempts cannot be silently converted.
    // Existing managed journals select managed custody, retaining every row.
    if desired == Domain::Managed && prior.is_none() && !managed {
        let unmanaged_history: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM gpu_node) OR EXISTS(SELECT 1 FROM launch_attempt)",
            [],
            |r| r.get(0),
        )?;
        if unmanaged_history {
            return Err(Error::Invalid);
        }
    }
    if prior.is_none() {
        tx.execute(
            "INSERT INTO execution_domain(owner,mode) VALUES (?,?)",
            [owner, desired.text()],
        )?;
    }
    Ok(())
}

impl Store {
    /// Commit the unmanaged domain before opening a mock provider/control socket.
    /// No reset/conversion is exposed. This supplements, not replaces, the
    /// caller's exclusive process lock across same-domain provider effects.
    pub fn claim_unmanaged_execution(&self) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        claim(&tx, &self.owner, Domain::Unmanaged)?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
