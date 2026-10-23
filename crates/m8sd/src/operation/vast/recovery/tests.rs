use super::*;
use crate::operation::vast::tests::{executor, setup};

#[test]
fn reopen_recovers_exact_identity_and_original_deadlines_without_create_authority() {
    let (store, directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = executor.intent("run", "request", "operation").unwrap();
    executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
    drop(store);
    let store = Store::open(directory.0.join("state.db"), "owner").unwrap();
    let recovered = store.recover_vast("operation").unwrap();
    assert_eq!(recovered.operation().intent, intent);
    assert_eq!(recovered.identity().label(), executor.identity.label());
    assert_eq!(recovered.identity().image(), executor.identity.image());
    assert_eq!(recovered.tier(), "smoke");
    assert_eq!(recovered.cleanup_starts_at().unwrap(), 160);
    assert_eq!(recovered.cleanup_deadline().unwrap(), 400);
    assert!(recovered.executor.request.is_none());
    assert!(recovered.executor.claim(&store, "operation").is_err());
    assert_eq!(
        store.create_operations().unwrap()[0].state,
        CreateState::Prepared
    );
    store.claim_create("operation", 101).unwrap();
    let recovered = store.recover_vast("operation").unwrap();
    assert_eq!(recovered.operation().state, CreateState::DispatchConsumed);
    assert!(recovered.executor.request.is_none());
    assert!(recovered.executor.claim(&store, "operation").is_err());
    assert_eq!(recovered.cleanup_deadline().unwrap(), 400);
}

#[test]
fn changed_offer_or_tier_cannot_be_recovered_under_original_fingerprint() {
    for field in ["offer", "tier", "image", "node"] {
        let (store, _directory) = setup(6_000_000, 1);
        let executor = executor(42);
        let intent = executor.intent("run", "request", "operation").unwrap();
        executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
        let mut canonical: serde_json::Value = serde_json::from_slice(&executor.canonical).unwrap();
        match field {
            "offer" => canonical["offer"] = serde_json::json!(43),
            "image" => {
                canonical["image"] =
                    serde_json::json!(format!("example.invalid/mesh@sha256:{}", "b".repeat(64)))
            }
            "node" => canonical["label"] = serde_json::json!("m8s:v1:owner:other:smoke"),
            _ => {
                store
                    .conn
                    .execute("UPDATE vast_launch SET tier='other'", [])
                    .unwrap();
            }
        }
        store
            .conn
            .execute(
                "UPDATE vast_launch SET canonical=?",
                [serde_json::to_vec(&canonical).unwrap()],
            )
            .unwrap();
        assert!(store.recover_vast("operation").is_err());
        assert_eq!(
            store.create_operations().unwrap()[0].state,
            CreateState::Prepared
        );
        assert_eq!(store.nodes().unwrap().len(), 1);
    }
}

#[test]
fn failed_metadata_write_rolls_back_launch_and_missing_legacy_metadata_is_not_invented() {
    let (store, _directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = executor.intent("run", "request", "operation").unwrap();
    store.conn.execute_batch("CREATE TRIGGER fail_capsule BEFORE INSERT ON vast_launch BEGIN SELECT RAISE(ABORT,'synthetic'); END;").unwrap();
    assert!(executor.prepare_at(&store, &intent, || Ok(100)).is_err());
    assert!(store.nodes().unwrap().is_empty());
    assert!(store.create_operations().unwrap().is_empty());
    assert_eq!(store.budget_reservations().unwrap().len(), 1);
    store
        .conn
        .execute_batch("DROP TRIGGER fail_capsule;")
        .unwrap();
    executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
    store.conn.execute("DELETE FROM vast_launch", []).unwrap();
    assert!(store.recover_vast("operation").is_err());
    assert!(executor.prepare_at(&store, &intent, || Ok(101)).is_err());
    assert_eq!(store.create_operations().unwrap().len(), 1);
}

#[test]
fn unbounded_or_unknown_canonical_record_refuses_without_state_changes() {
    let (store, _directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = executor.intent("run", "request", "operation").unwrap();
    executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
    for bytes in [vec![b'x'; 4097], b"{}".to_vec(), b"[]".to_vec()] {
        store
            .conn
            .execute("UPDATE vast_launch SET canonical=?", [bytes])
            .unwrap();
        let before = store.conn.total_changes();
        assert!(store.recover_vast("operation").is_err());
        assert_eq!(store.conn.total_changes(), before);
    }
}
