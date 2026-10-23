//! The single node-row deletion boundary. Callers establish explicit exact
//! provider absence; a stopped node or an empty inventory is insufficient.
use crate::{Error, Result, nonce};
use m8s_core::Phase;
use m8s_core::policy::{LifecycleEvidence, may_delete, transition};
use rusqlite::{OptionalExtension, Transaction, params};

pub(crate) fn remove_absent_node(
    tx: &Transaction<'_>,
    owner: &str,
    uid: &str,
    instance: &str,
    now: i64,
) -> Result<bool> {
    let owns: bool = tx.query_row(
        "SELECT count(*)=1 AND min(owner)=? FROM pool",
        [owner],
        |r| r.get(0),
    )?;
    if !owns || now < 0 {
        return Err(Error::Invalid);
    }
    let row: Option<(Option<String>, String)> = tx
        .query_row(
            "SELECT instance_id,phase FROM gpu_node WHERE uid=?",
            [uid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((bound, phase)) = row else {
        return Ok(false);
    };
    if bound.as_deref() != Some(instance) {
        return Err(Error::Invalid);
    }
    let phase = serde_json::from_str(&phase).map_err(|_| Error::Corrupt)?;
    let evidence = LifecycleEvidence {
        provider_confirmed_absent: true,
        ..LifecycleEvidence::default()
    };
    let destroying = transition(phase, Phase::Destroying, evidence).map_err(|_| Error::Corrupt)?;
    let gone = transition(destroying, Phase::Gone, evidence).map_err(|_| Error::Corrupt)?;
    if !may_delete(gone, evidence) {
        return Err(Error::Corrupt);
    }
    // Nonce and certificate revocation metadata survive removal of the node row.
    nonce::cancel_uid(tx, owner, uid)?;
    if tx.execute(
        "DELETE FROM gpu_node WHERE uid=? AND instance_id=?",
        params![uid, instance],
    )? != 1
    {
        return Err(Error::Corrupt);
    }
    tx.execute(
        "INSERT INTO event(at,message) VALUES (?,?)",
        params![now, "provider confirmed absence; finalizer released"],
    )?;
    Ok(true)
}
