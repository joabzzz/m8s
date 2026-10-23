//! Bounded stored control metadata, not provider currentness or execution authority.
use crate::{Error, Result, Store};
use m8s_core::{GpuNode, Phase};
use rusqlite::{TransactionBehavior, types::ValueRef};

/// Plain stored metadata only; no admission, authenticity or readiness evidence.
pub struct ControlSnapshot {
    pub nodes: Vec<GpuNode>,
    pub tiers: Vec<(String, u32)>,
    pub events: Vec<String>,
}
fn text<'a>(value: ValueRef<'a>, max: usize) -> Result<&'a str> {
    match value {
        ValueRef::Text(bytes) if bytes.len() <= max => {
            std::str::from_utf8(bytes).map_err(|_| Error::Corrupt)
        }
        _ => Err(Error::Corrupt),
    }
}
fn tier(value: ValueRef<'_>) -> Result<&str> {
    let s = text(value, 64)?;
    if s.is_empty()
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Error::Corrupt);
    }
    Ok(s)
}
fn integer(value: ValueRef<'_>) -> Result<i64> {
    match value {
        ValueRef::Integer(n) => Ok(n),
        _ => Err(Error::Corrupt),
    }
}
impl Store {
    /// Stored metadata for a managed coordinator only. An unmanaged/mock Store
    /// must never be displayed under a live execution mode. This is not Ready
    /// or fresh provider evidence and does not select or change a domain.
    pub fn managed_control_snapshot(&self) -> Result<ControlSnapshot> {
        if !crate::execution_domain::managed_initialized(&self.conn, &self.owner)? {
            return Err(Error::Invalid);
        }
        self.control_snapshot()
    }

    /// One bounded DEFERRED stored snapshot for display, not a global-currentness proof.
    /// Refuses overflow/corruption without truncating tiers/nodes or mutating state.
    pub fn control_snapshot(&self) -> Result<ControlSnapshot> {
        if self.owner.len() > 256 {
            return Err(Error::Corrupt);
        }
        let tx = rusqlite::Transaction::new_unchecked(&self.conn, TransactionBehavior::Deferred)?;
        let mut nodes = Vec::new();
        {
            let mut query = tx.prepare("SELECT uid,tier,instance_id,phase,created_at FROM gpu_node ORDER BY created_at,uid LIMIT 257")?;
            let mut rows = query.query([])?;
            while let Some(row) = rows.next()? {
                if nodes.len() == 256 {
                    return Err(Error::Corrupt);
                }
                let uid = text(row.get_ref(0)?, 256)?;
                if uid.is_empty() {
                    return Err(Error::Corrupt);
                }
                let tier = tier(row.get_ref(1)?)?;
                let instance_id = match row.get_ref(2)? {
                    ValueRef::Null => None,
                    value => Some(text(value, 256)?),
                };
                let phase: Phase =
                    serde_json::from_str(text(row.get_ref(3)?, 32)?).map_err(|_| Error::Corrupt)?;
                let created_at = integer(row.get_ref(4)?)?;
                nodes.push(GpuNode {
                    uid: uid.into(),
                    tier: tier.into(),
                    owner: self.owner.clone(),
                    instance_id: instance_id.map(str::to_owned),
                    phase,
                    created_at,
                });
            }
        }
        let mut tiers = Vec::new();
        {
            let mut query = tx.prepare("SELECT name,desired FROM tier ORDER BY name LIMIT 257")?;
            let mut rows = query.query([])?;
            while let Some(row) = rows.next()? {
                if tiers.len() == 256 {
                    return Err(Error::Corrupt);
                }
                let name = tier(row.get_ref(0)?)?;
                let desired = u32::try_from(integer(row.get_ref(1)?)?)
                    .ok()
                    .filter(|n| *n <= 16)
                    .ok_or(Error::Corrupt)?;
                tiers.push((name.into(), desired));
            }
        }
        let mut events = Vec::new();
        {
            let mut query = tx.prepare("SELECT message FROM (SELECT seq,message FROM event ORDER BY seq DESC LIMIT 200) ORDER BY seq")?;
            let mut rows = query.query([])?;
            while let Some(row) = rows.next()? {
                events.push(text(row.get_ref(0)?, 4096)?.to_owned());
            }
        }
        tx.commit()?;
        Ok(ControlSnapshot {
            nodes,
            tiers,
            events,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, params};
    // Isolated in-memory projection tables deliberately permit malformed values.
    // No production constructor/schema or authority is added by this fixture.
    fn store() -> Store {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE gpu_node(uid,tier,instance_id,phase,created_at); CREATE TABLE tier(name,desired); CREATE TABLE event(seq INTEGER PRIMARY KEY,at,message);").unwrap();
        Store {
            conn,
            _private: None,
            owner: "synthetic".into(),
            origin: std::sync::Arc::new(crate::StoreOrigin),
        }
    }
    fn node(s: &Store, uid: &str) {
        s.conn
            .execute(
                "INSERT INTO gpu_node VALUES (?,'t',NULL,'\"Ready\"',42)",
                [uid],
            )
            .unwrap();
    }
    fn changes(s: &Store) -> i64 {
        s.conn
            .query_row("SELECT total_changes()", [], |r| r.get(0))
            .unwrap()
    }
    #[test]
    fn managed_view_refuses_unmanaged_without_selecting_a_domain() {
        let s = store();
        s.conn.execute_batch("CREATE TABLE execution_domain(owner TEXT, mode TEXT); CREATE TABLE create_operation(x); CREATE TABLE budget_account(x); CREATE TABLE budget_reservation(x); CREATE TABLE budget_period(x); CREATE TABLE vast_launch(x); CREATE TABLE vast_removal(x);").unwrap();
        let before = changes(&s);
        assert!(s.managed_control_snapshot().is_err());
        assert_eq!(changes(&s), before);
        s.conn
            .execute(
                "INSERT INTO execution_domain VALUES ('synthetic','managed')",
                [],
            )
            .unwrap();
        let before = changes(&s);
        assert!(s.managed_control_snapshot().unwrap().nodes.is_empty());
        assert_eq!(changes(&s), before);
    }
    #[test]
    fn fitting_snapshot_matches_legacy_order_values_and_is_read_only() {
        let s = store();
        node(&s, "z");
        node(&s, "a");
        s.conn
            .execute(
                "UPDATE gpu_node SET instance_id='instance',created_at=-1 WHERE uid='z'",
                [],
            )
            .unwrap();
        s.conn.execute_batch("INSERT INTO tier VALUES ('z',0),('t',16); INSERT INTO event VALUES (2,0,'second'),(1,0,'first');").unwrap();
        let before = changes(&s);
        let snapshot = s.control_snapshot().unwrap();
        assert_eq!(snapshot.nodes, s.nodes().unwrap());
        assert_eq!(snapshot.tiers, s.tiers().unwrap());
        assert_eq!(snapshot.events, s.logs().unwrap());
        assert_eq!(snapshot.nodes[1].phase, Phase::Ready); // stored metadata only
        assert_eq!(changes(&s), before);
        assert!(s.conn.is_autocommit());
        s.conn.execute_batch("BEGIN DEFERRED").unwrap();
        assert!(s.control_snapshot().is_err());
        assert!(!s.conn.is_autocommit());
        assert_eq!(changes(&s), before);
        s.conn.execute_batch("ROLLBACK").unwrap();
        assert!(s.control_snapshot().is_ok());
    }
    #[test]
    fn exact_node_and_tier_caps_refuse_extra_without_truncation_or_writes() {
        let s = store();
        for i in 0..256 {
            node(&s, &format!("n{i:03}"));
            s.conn
                .execute("INSERT INTO tier VALUES (?,16)", [format!("t{i:03}")])
                .unwrap();
        }
        let snapshot = s.control_snapshot().unwrap();
        assert_eq!(snapshot.nodes.len(), 256);
        assert_eq!(snapshot.tiers.len(), 256);
        node(&s, "extra");
        let before = changes(&s);
        assert!(s.control_snapshot().is_err());
        assert_eq!(changes(&s), before);
        assert!(s.conn.is_autocommit());
        s.conn
            .execute("DELETE FROM gpu_node WHERE uid='extra'", [])
            .unwrap();
        assert!(s.control_snapshot().is_ok());
        s.conn
            .execute("INSERT INTO tier VALUES ('extra',0)", [])
            .unwrap();
        let before = changes(&s);
        assert!(s.control_snapshot().is_err());
        assert_eq!(changes(&s), before);
    }
    #[test]
    fn text_byte_limits_and_owner_are_checked_before_collection() {
        let mut s = store();
        node(&s, &"é".repeat(128));
        s.owner = "é".repeat(128);
        s.conn
            .execute(
                "UPDATE gpu_node SET instance_id=?,tier=?",
                params!["é".repeat(128), "t".repeat(64)],
            )
            .unwrap();
        s.conn
            .execute("INSERT INTO tier VALUES (?,16)", ["t".repeat(64)])
            .unwrap();
        assert!(s.control_snapshot().is_ok());
        s.owner.push('x');
        assert!(s.control_snapshot().is_err());
        s.owner.pop();
        for sql in [
            "UPDATE gpu_node SET uid=uid||'x'",
            "UPDATE gpu_node SET instance_id=instance_id||'x'",
            "UPDATE gpu_node SET tier=tier||'x'",
        ] {
            s.conn.execute_batch("SAVEPOINT malformed").unwrap();
            s.conn.execute_batch(sql).unwrap();
            // Avoid nested-transaction rejection as the oracle: release first.
            s.conn.execute_batch("RELEASE malformed").unwrap();
            assert!(s.control_snapshot().is_err());
            s.conn
                .execute(
                    "UPDATE gpu_node SET uid=?,instance_id=?,tier=?",
                    params!["é".repeat(128), "é".repeat(128), "t".repeat(64)],
                )
                .unwrap();
            assert!(s.control_snapshot().is_ok());
        }
    }
    #[test]
    fn malformed_sqlite_node_and_tier_values_refuse_without_repair() {
        for sql in [
            "UPDATE gpu_node SET uid=x'ff'",
            "UPDATE gpu_node SET uid=CAST(x'ff' AS TEXT)",
            "UPDATE gpu_node SET uid=NULL",
            "UPDATE gpu_node SET uid=''",
            "UPDATE gpu_node SET tier='bad.id'",
            "UPDATE gpu_node SET tier=1",
            "UPDATE gpu_node SET instance_id=x'61'",
            "UPDATE gpu_node SET instance_id=CAST(x'ff' AS TEXT)",
            "UPDATE gpu_node SET phase=NULL",
            "UPDATE gpu_node SET phase=x'226122'",
            "UPDATE gpu_node SET phase='\"Unknown\"'",
            "UPDATE gpu_node SET phase='invalid'",
            "UPDATE gpu_node SET phase=printf('%040d',0)",
            "UPDATE gpu_node SET created_at='42'",
            "UPDATE gpu_node SET created_at=1.5",
            "UPDATE tier SET name=NULL",
            "UPDATE tier SET name='bad.id'",
            "UPDATE tier SET name=x'61'",
            "UPDATE tier SET desired=17",
            "UPDATE tier SET desired=-1",
            "UPDATE tier SET desired=1.5",
            "UPDATE tier SET desired='1'",
        ] {
            let s = store();
            node(&s, "n");
            s.conn
                .execute("INSERT INTO tier VALUES ('t',1)", [])
                .unwrap();
            assert!(s.control_snapshot().is_ok());
            s.conn.execute_batch(sql).unwrap();
            let before = changes(&s);
            assert!(s.control_snapshot().is_err(), "{sql}");
            assert_eq!(changes(&s), before);
            assert!(s.conn.is_autocommit());
        }
    }
    #[test]
    fn event_window_retains_only_latest_two_hundred_and_checks_selected_bytes() {
        let s = store();
        s.conn
            .execute("INSERT INTO event VALUES (0,0,x'ff')", [])
            .unwrap();
        for i in 1..=200 {
            s.conn
                .execute(
                    "INSERT INTO event VALUES (?,0,?)",
                    params![i, format!("event{i:03}")],
                )
                .unwrap();
        }
        let snapshot = s.control_snapshot().unwrap();
        assert_eq!(snapshot.events, s.logs().unwrap());
        assert_eq!(snapshot.events.len(), 200);
        assert_eq!(snapshot.events[0], "event001");
        assert_eq!(snapshot.events[199], "event200");
        s.conn
            .execute(
                "UPDATE event SET message=? WHERE seq=200",
                ["é".repeat(2048)],
            )
            .unwrap();
        assert!(s.control_snapshot().is_ok());
        for bad in [
            "NULL",
            "x'61'",
            "CAST(x'ff' AS TEXT)",
            "printf('%04097d',0)",
        ] {
            s.conn
                .execute_batch(&format!("UPDATE event SET message={bad} WHERE seq=200"))
                .unwrap();
            let before = changes(&s);
            assert!(s.control_snapshot().is_err());
            assert_eq!(changes(&s), before);
        }
    }
}
