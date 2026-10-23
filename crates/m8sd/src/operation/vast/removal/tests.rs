//! SQLite-only fault injection; private effect input, never a public proof forge.
use super::*;
use crate::operation::vast::tests::{executor, setup};

fn bind(store: &Store, executor: &VastExecutor) -> CreateIntent {
    let intent = executor.intent("run", "request", "operation").unwrap();
    executor.prepare_at(store, &intent, || Ok(100)).unwrap();
    let claim = store.claim_create("operation", 100).unwrap();
    assert!(
        start_bound_invocation(
            store,
            &claim,
            InvocationClock::Explicit(100),
            "smoke",
            false,
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
                provenance: "synthetic-exact-observation".into(),
                observed_at: 101,
                status: EvidenceStatus::Complete,
            }),
            101,
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE gpu_node SET instance_id='42',phase='\"Booting\"' WHERE uid='node'",
            [],
        )
        .unwrap();
    intent
}

#[test]
fn removal_is_atomic_keeps_claim_and_liability_and_survives_reopen() {
    let (store, directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = bind(&store, &executor);
    let original = store.budget_reservations().unwrap().remove(0);
    let record = executor
        .finalize_removal_at(&store, &intent, "42", 102, &original.period, 102)
        .unwrap();
    assert!(store.nodes().unwrap().is_empty());
    let held = store.budget_reservations().unwrap().remove(0);
    assert_eq!(held.state, BudgetState::BillingUnknown);
    assert_eq!(held.reserved_all_in, original.reserved_all_in);
    assert!(store.claim_create("operation", 103).is_err());
    assert_eq!(store.vast_removals().unwrap(), vec![record.clone()]);
    drop(store);
    let reopened = Store::open(directory.0.join("state.db"), "owner").unwrap();
    assert!(reopened.nodes().unwrap().is_empty());
    assert_eq!(reopened.vast_removals().unwrap(), vec![record.clone()]);
    assert_eq!(
        executor
            .finalize_removal_at(&reopened, &intent, "42", 103, &original.period, 103)
            .unwrap(),
        record
    );
    assert_eq!(reopened.budget_reservations().unwrap(), vec![held]);
    let status = reopened.vast_audit().unwrap().attempts.remove(0);
    assert_eq!(status.stage, super::super::AttemptStage::RemovalRecorded);
    assert_eq!(status.billing_state, BudgetState::BillingUnknown);
}

#[test]
fn wrong_resource_stale_future_or_unbound_observation_never_removes() {
    let (store, _directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = bind(&store, &executor);
    let original = store.budget_reservations().unwrap().remove(0);
    for (id, observed, now) in [
        ("43", 102, 102),
        ("42", 102, 133),
        ("42", 104, 103),
        ("42", 99, 102),
    ] {
        assert!(
            executor
                .finalize_removal_at(&store, &intent, id, observed, &original.period, now)
                .is_err()
        );
        assert_eq!(store.nodes().unwrap().len(), 1);
        assert_eq!(store.budget_reservations().unwrap(), vec![original.clone()]);
        assert!(store.vast_removals().unwrap().is_empty());
    }
}

#[test]
fn receipt_persistence_failure_rolls_back_node_and_budget_together() {
    let (store, _directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = bind(&store, &executor);
    let original = store.budget_reservations().unwrap().remove(0);
    store.conn.execute_batch("CREATE TRIGGER fail_removal BEFORE INSERT ON vast_removal BEGIN SELECT RAISE(ABORT,'synthetic'); END;").unwrap();
    assert!(
        executor
            .finalize_removal_at(&store, &intent, "42", 102, &original.period, 102)
            .is_err()
    );
    assert_eq!(store.nodes().unwrap().len(), 1);
    assert_eq!(store.budget_reservations().unwrap(), vec![original]);
    assert!(store.vast_removals().unwrap().is_empty());
    assert!(
        !store
            .conn
            .query_row("SELECT deleting FROM gpu_node WHERE uid='node'", [], |r| {
                r.get::<_, bool>(0)
            })
            .unwrap()
    );
}

#[test]
fn duplicate_receipt_does_not_downgrade_final_billing() {
    let (store, _directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = bind(&store, &executor);
    let original = store.budget_reservations().unwrap().remove(0);
    executor
        .finalize_removal_at(&store, &intent, "42", 102, &original.period, 102)
        .unwrap();
    let held = store.budget_reservations().unwrap().remove(0);
    let mut settlement = held.settlement.unwrap();
    settlement.observed_at = 103;
    settlement.billing = FinalBilling::Known {
        actual_all_in: 75,
        provenance: "synthetic-final-invoice".into(),
    };
    let settled = store
        .settle_budget("request", &original.period, &settlement, 103)
        .unwrap();
    executor
        .finalize_removal_at(&store, &intent, "42", 104, &original.period, 104)
        .unwrap();
    assert_eq!(store.budget_reservations().unwrap(), vec![settled]);
}

#[test]
fn legacy_mock_reconcile_cannot_delete_managed_nodes_or_launch_replacements() {
    let (mut store, _directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = bind(&store, &executor);
    let mut mock = m8s_provider::Mock::default();
    mock.faults.push(m8s_provider::Fault::List);
    assert!(store.reconcile(&mut mock, 102).is_err());
    assert_eq!(mock.faults, vec![m8s_provider::Fault::List]);
    assert_eq!(store.nodes().unwrap().len(), 1);
    let original = store.budget_reservations().unwrap().remove(0);
    executor
        .finalize_removal_at(&store, &intent, "42", 102, &original.period, 102)
        .unwrap();
    assert!(store.reconcile(&mut mock, 103).is_err());
    assert_eq!(mock.faults, vec![m8s_provider::Fault::List]);
    assert!(store.nodes().unwrap().is_empty());
    assert!(mock.destroy_calls.is_empty());
}

#[test]
fn corrupt_removal_record_is_not_reported_as_success() {
    let (store, _directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = bind(&store, &executor);
    let original = store.budget_reservations().unwrap().remove(0);
    let mut record = executor
        .finalize_removal_at(&store, &intent, "42", 102, &original.period, 102)
        .unwrap();
    record.instance_id = "43".into();
    store
        .conn
        .execute(
            "UPDATE vast_removal SET record=?",
            [encode(&record).unwrap()],
        )
        .unwrap();
    assert!(store.vast_removals().is_err());
    assert!(
        executor
            .finalize_removal_at(&store, &intent, "42", 103, &original.period, 103)
            .is_err()
    );
    assert_eq!(
        store.budget_reservations().unwrap()[0].state,
        BudgetState::BillingUnknown
    );
}
