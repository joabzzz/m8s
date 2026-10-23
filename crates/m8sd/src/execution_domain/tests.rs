use super::*;
use std::os::unix::fs::DirBuilderExt;
use std::{
    path::PathBuf,
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "m8sd-domain-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
    fn open(&self) -> Store {
        Store::open(self.0.join("state.db"), "owner").unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn select(store: &Store, domain: Domain) -> Result<()> {
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    claim(&tx, "owner", domain)?;
    tx.commit()?;
    Ok(())
}

#[test]
fn selection_survives_reopen_is_idempotent_and_never_converts() {
    for domain in [Domain::Managed, Domain::Unmanaged] {
        let directory = Directory::new();
        let store = directory.open();
        let before = store.conn.total_changes();
        store.require_unmanaged_state().unwrap();
        assert_eq!(store.conn.total_changes(), before);
        select(&store, domain).unwrap();
        let before = store.conn.total_changes();
        select(&store, domain).unwrap();
        assert_eq!(store.conn.total_changes(), before);
        drop(store);
        let store = directory.open();
        assert!(recorded(&store.conn, "owner").unwrap() == Some(domain));
        let opposite = if domain == Domain::Managed {
            Domain::Unmanaged
        } else {
            Domain::Managed
        };
        assert!(select(&store, opposite).is_err());
        assert_eq!(
            store.require_unmanaged_state().is_ok(),
            domain == Domain::Unmanaged
        );
    }
}

#[test]
fn concurrent_domain_selection_has_one_durable_winner() {
    let directory = Directory::new();
    drop(directory.open());
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [Domain::Managed, Domain::Unmanaged]
        .into_iter()
        .map(|domain| {
            let path = directory.0.join("state.db");
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let store = Store::open(path, "owner").unwrap();
                barrier.wait();
                (domain, select(&store, domain).is_ok())
            })
        })
        .collect();
    let outcomes: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|(_, ok)| *ok).count(), 1);
    let winner = outcomes.iter().find(|(_, ok)| *ok).unwrap().0;
    assert!(recorded(&directory.open().conn, "owner").unwrap() == Some(winner));
}

#[test]
fn legacy_unmanaged_nodes_or_attempts_never_become_managed_implicitly() {
    for node in [true, false] {
        let directory = Directory::new();
        let store = directory.open();
        if node {
            store
                .conn
                .execute(
                    "INSERT INTO gpu_node(uid,tier,instance_id,phase,created_at,orphan) VALUES ('node','tier','42','\"Booting\"',0,0)",
                    [],
                )
                .unwrap();
        } else {
            store
                .conn
                .execute("INSERT INTO launch_attempt VALUES ('node','tier',0)", [])
                .unwrap();
        }
        let before = store.conn.total_changes();
        assert!(select(&store, Domain::Managed).is_err());
        assert_eq!(store.conn.total_changes(), before);
        assert!(recorded(&store.conn, "owner").unwrap().is_none());
        store.claim_unmanaged_execution().unwrap();
    }
}

#[test]
fn legacy_managed_or_orphan_history_blocks_mock_before_provider_access() {
    for insert in [
        "INSERT INTO budget_account VALUES ('owner','{}')",
        "INSERT INTO budget_reservation VALUES ('owner','request','node','{}')",
        "INSERT INTO create_operation VALUES ('owner','operation','request','node','{}')",
        "INSERT INTO vast_launch VALUES ('owner','operation','tier',X'7b7d')",
        "INSERT INTO vast_removal VALUES ('owner','operation','{}')",
    ] {
        let directory = Directory::new();
        let mut store = directory.open();
        store.conn.execute(insert, []).unwrap();
        assert!(store.require_unmanaged_state().is_err());
        let mut mock = m8s_provider::Mock::default();
        mock.faults.push(m8s_provider::Fault::List);
        assert!(store.reconcile(&mut mock, 100).is_err());
        assert!(mock.destroy_calls.is_empty());
        assert_eq!(mock.faults, vec![m8s_provider::Fault::List]);
        select(&store, Domain::Managed).unwrap();
    }
}

#[test]
fn failed_transaction_does_not_leave_a_domain_or_bypass_later_selection() {
    let directory = Directory::new();
    let store = directory.open();
    {
        let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate).unwrap();
        claim(&tx, "owner", Domain::Managed).unwrap();
        // Simulate a later failed write: dropping the transaction rolls back.
    }
    assert!(recorded(&store.conn, "owner").unwrap().is_none());
    store.claim_unmanaged_execution().unwrap();
    assert!(select(&store, Domain::Managed).is_err());
}

#[test]
fn cross_owner_or_multiple_domain_records_refuse_without_repair() {
    let directory = Directory::new();
    let store = directory.open();
    store
        .conn
        .execute(
            "INSERT INTO execution_domain VALUES ('other','unmanaged')",
            [],
        )
        .unwrap();
    assert!(store.require_unmanaged_state().is_err());
    assert!(select(&store, Domain::Managed).is_err());
    store
        .conn
        .execute(
            "INSERT INTO execution_domain VALUES ('owner','unmanaged')",
            [],
        )
        .unwrap();
    let before = store.conn.total_changes();
    assert!(store.claim_unmanaged_execution().is_err());
    assert_eq!(store.conn.total_changes(), before);
}
