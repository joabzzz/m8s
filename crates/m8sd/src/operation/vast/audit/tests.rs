use super::*;
use crate::operation::vast::tests::{executor, setup};

#[test]
fn one_snapshot_does_not_mix_a_concurrently_consumed_claim_and_old_budget() {
    let (store, directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = executor.intent("run", "request", "operation").unwrap();
    executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
    let writer = Store::open(directory.0.join("state.db"), "owner").unwrap();
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Deferred).unwrap();
    owned(&tx, "owner").unwrap(); // Pin the same read snapshot as the public method.
    writer.claim_create("operation", 101).unwrap();
    let old = store.read_vast_audit(&tx).unwrap();
    assert_eq!(old.attempts[0].stage, AttemptStage::Prepared);
    assert_eq!(old.attempts[0].billing_state, BudgetState::Reserved);
    tx.commit().unwrap();
    let current = store.vast_audit().unwrap();
    assert_eq!(current.attempts[0].stage, AttemptStage::ConsumedNotInvoked);
    assert_eq!(current.attempts[0].billing_state, BudgetState::Uncertain);
}

#[test]
fn settled_marker_without_matching_final_evidence_refuses() {
    let (store, _directory) = setup(6_000_000, 1);
    let mut budget = store.budget_reservations().unwrap().remove(0);
    budget.state = BudgetState::Settled;
    store
        .conn
        .execute(
            "UPDATE budget_reservation SET record=?",
            [encode(&budget).unwrap()],
        )
        .unwrap();
    assert!(store.vast_audit().is_err());
}

#[test]
fn projects_reservation_preparation_consumption_and_invocation_without_effects() {
    let (store, _directory) = setup(6_000_000, 1);
    let status = store.vast_audit().unwrap().attempts.remove(0);
    assert_eq!(status.stage, AttemptStage::ReservationOnly);
    assert_eq!(status.metadata, RecoveryMetadata::NoOperation);
    let executor = executor(42);
    let intent = executor.intent("run", "request", "operation").unwrap();
    executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
    let status = store.vast_audit().unwrap().attempts.remove(0);
    assert_eq!(status.stage, AttemptStage::Prepared);
    assert_eq!(
        status.metadata,
        RecoveryMetadata::Verified {
            cleanup_starts_at: 160,
            cleanup_deadline: 400
        }
    );
    let claim = store.claim_create("operation", 101).unwrap();
    assert_eq!(
        store.vast_audit().unwrap().attempts[0].stage,
        AttemptStage::ConsumedNotInvoked
    );
    assert!(
        start_bound_invocation(
            &store,
            &claim,
            InvocationClock::Explicit(102),
            "smoke",
            false
        )
        .unwrap()
    );
    let before = store.conn.total_changes();
    let snapshot = store.vast_audit().unwrap();
    assert_eq!(
        snapshot.attempts[0].stage,
        AttemptStage::InvocationUnresolved
    );
    assert_eq!(snapshot.attempts[0].billing_state, BudgetState::Uncertain);
    assert_eq!(store.conn.total_changes(), before);
    assert!(store.claim_create("operation", 103).is_err());
}

#[test]
fn cancellation_is_separate_from_the_last_observed_effect_stage() {
    for consume in [false, true] {
        let (store, _directory) = setup(6_000_000, 1);
        let executor = executor(42);
        let intent = executor.intent("run", "request", "operation").unwrap();
        executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
        if consume {
            store.claim_create("operation", 101).unwrap();
        }
        store.cancel_create("operation", 102).unwrap();
        let status = store.vast_audit().unwrap().attempts.remove(0);
        assert!(status.cancelled);
        assert_eq!(
            status.stage,
            if consume {
                AttemptStage::ConsumedNotInvoked
            } else {
                AttemptStage::CancelledBeforeDispatch
            }
        );
        assert_eq!(store.nodes().unwrap().len(), 1);
    }
}

#[test]
fn missing_legacy_metadata_is_reported_but_corrupt_present_metadata_refuses() {
    let (store, _directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = executor.intent("run", "request", "operation").unwrap();
    executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
    store.conn.execute("DELETE FROM vast_launch", []).unwrap();
    let before = store.conn.total_changes();
    assert_eq!(
        store.vast_audit().unwrap().attempts[0].metadata,
        RecoveryMetadata::MissingLegacy
    );
    assert_eq!(store.conn.total_changes(), before);
    store
        .conn
        .execute(
            "INSERT INTO vast_launch VALUES ('owner','operation','smoke',?)",
            [b"{}".as_slice()],
        )
        .unwrap();
    assert!(store.vast_audit().is_err());
}

#[test]
fn bounded_input_refuses_overflow_instead_of_truncating() {
    for table in [
        "create_operation",
        "budget_reservation",
        "vast_removal",
        "vast_launch",
    ] {
        let (store, _directory) = setup(6_000_000, 1);
        let executor = executor(42);
        let intent = executor.intent("run", "request", "operation").unwrap();
        executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
        if table == "vast_removal" {
            store
                .conn
                .execute(
                    "INSERT INTO vast_removal VALUES ('owner','operation','{}')",
                    [],
                )
                .unwrap();
        }
        let column = if table == "vast_launch" {
            "canonical"
        } else {
            "record"
        };
        store
            .conn
            .execute(
                &format!("UPDATE {table} SET {column}=?"),
                ["x".repeat(MAX_RECORD_BYTES + 1)],
            )
            .unwrap();
        let before = store.conn.total_changes();
        assert!(store.vast_audit().is_err());
        assert_eq!(store.conn.total_changes(), before);
    }
    let (store, _directory) = setup(6_000_000, 1);
    // Fill 129 reservation rows; preflight refuses before trying to decode any.
    for i in 0..MAX_ROWS {
        store.conn.execute("INSERT INTO budget_reservation(owner,request_id,node_uid,record) VALUES ('owner',?,?, '{}')", params![format!("request-{i}"), format!("node-{i}")]).unwrap();
    }
    assert!(store.vast_audit().is_err());
}

#[test]
fn mismatched_rows_and_orphaned_capsules_cannot_look_like_empty_state() {
    for corruption in ["budget", "operation", "capsule"] {
        let (store, _directory) = setup(6_000_000, 1);
        let executor = executor(42);
        let intent = executor.intent("run", "request", "operation").unwrap();
        executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
        match corruption {
            "budget" => {
                store
                    .conn
                    .execute("UPDATE budget_reservation SET node_uid='other'", [])
                    .unwrap();
            }
            "operation" => {
                store
                    .conn
                    .execute("DELETE FROM budget_reservation", [])
                    .unwrap();
            }
            _ => {
                store
                    .conn
                    .execute("UPDATE vast_launch SET operation_id='orphan'", [])
                    .unwrap();
            }
        }
        let before = store.conn.total_changes();
        assert!(store.vast_audit().is_err());
        assert_eq!(store.conn.total_changes(), before);
    }
}

#[test]
fn stored_binding_is_not_current_presence_or_removal() {
    let (store, _directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = executor.intent("run", "request", "operation").unwrap();
    executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
    let claim = store.claim_create("operation", 100).unwrap();
    assert!(
        start_bound_invocation(
            &store,
            &claim,
            InvocationClock::Explicit(100),
            "smoke",
            false
        )
        .unwrap()
    );
    store
        .record_create_result(
            "operation",
            &CreateObservation::Bound(ResourceBinding {
                owner: "owner".into(),
                run_id: "run".into(),
                node_uid: "node".into(),
                instance_id: "42".into(),
                volumes: VolumeBinding::NoneAllocated,
                provenance: "synthetic".into(),
                observed_at: 101,
                status: EvidenceStatus::Complete,
            }),
            101,
        )
        .unwrap();
    let snapshot = store.vast_audit().unwrap();
    assert_eq!(snapshot.attempts[0].stage, AttemptStage::BindingRecorded);
    assert!(snapshot.removals.is_empty());
    assert_ne!(snapshot.attempts[0].billing_state, BudgetState::Settled);
}
