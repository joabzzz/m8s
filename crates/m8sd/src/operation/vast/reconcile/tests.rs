//! Deterministic fault matrix for restart reconciliation. SQLite and a scripted
//! port only: no HTTPS client, credential or clock other than the injected one.
use super::*;
use crate::operation::vast::tests::{Directory, executor, setup};
use std::collections::VecDeque;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Prepared,
    Consumed,
    Invoked,
    Bound,
}

fn instance(n: u64) -> NonZeroU64 {
    NonZeroU64::new(n).unwrap()
}
fn unavailable<T>() -> m8s_provider::Result<T> {
    Err(m8s_provider::Error::Unavailable)
}

/// Scripted provider. An exhausted queue means "unresolved": lookup finds
/// nothing, destroy fails and absence is not confirmed.
#[derive(Default)]
struct Fake {
    lookup: VecDeque<m8s_provider::Result<Option<u64>>>,
    destroy: VecDeque<m8s_provider::Result<()>>,
    confirm: VecDeque<m8s_provider::Result<Option<(u64, bool)>>>,
    calls: Vec<&'static str>,
    /// Injected clock; a fresh absence is observed at the pass time.
    at: i64,
}
impl RestartPort for Fake {
    fn lookup(&mut self, _: &VastRecovery) -> m8s_provider::Result<Option<NonZeroU64>> {
        self.calls.push("lookup");
        self.lookup
            .pop_front()
            .unwrap_or(Ok(None))
            .map(|v| v.map(instance))
    }
    fn destroy(&mut self, _: &VastRecovery, _: NonZeroU64) -> m8s_provider::Result<()> {
        self.calls.push("destroy");
        self.destroy.pop_front().unwrap_or_else(unavailable)
    }
    fn confirm_absent(
        &mut self,
        _: &VastRecovery,
        _: NonZeroU64,
    ) -> m8s_provider::Result<Option<Absence>> {
        self.calls.push("confirm");
        self.confirm.pop_front().unwrap_or(Ok(None)).map(|v| {
            let at = self.at;
            v.map(|(id, matches)| {
                Absence(AbsenceInner::Synthetic {
                    instance: instance(id),
                    observed_at: at,
                    matches,
                })
            })
        })
    }
}
/// Everything succeeds: found as 42, destroy acknowledged, exact absence.
fn healthy() -> Fake {
    Fake {
        lookup: [Ok(Some(42))].into(),
        destroy: [Ok(())].into(),
        confirm: [Ok(Some((42, true)))].into(),
        calls: vec![],
        at: 0,
    }
}

fn staged(stage: Stage) -> (Store, Directory, VastExecutor) {
    let (store, directory) = setup(6_000_000, 1);
    let executor = executor(42);
    let intent = executor.intent("run", "request", "operation").unwrap();
    executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
    if stage != Stage::Prepared {
        let claim = store.claim_create("operation", 100).unwrap();
        if stage != Stage::Consumed {
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
        }
    }
    if stage == Stage::Bound {
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
    }
    (store, directory, executor)
}

fn pass(
    store: &Store,
    port: &mut Fake,
    trigger: CleanupTrigger,
    now: i64,
) -> Vec<(String, Outcome)> {
    port.at = now;
    store
        .reconcile_vast(port, trigger, || Ok(now))
        .unwrap()
        .into_iter()
        .map(|r| (r.operation_id, r.outcome))
        .collect()
}
fn only(report: Vec<(String, Outcome)>) -> Outcome {
    assert_eq!(report.len(), 1);
    report[0].1
}
fn reopen(store: Store, directory: &Directory) -> Store {
    drop(store);
    Store::open(directory.0.join("state.db"), "owner").unwrap()
}

/// What a restart must never change once a claim has been consumed.
#[derive(Debug, PartialEq, Eq)]
struct Frozen {
    operations: usize,
    dispatched_at: Option<i64>,
    invocation_started_at: Option<i64>,
    reserved_at: i64,
    reserved_all_in: u64,
    cleanup_deadline: i64,
}
fn frozen(store: &Store) -> Frozen {
    let operations = store.create_operations().unwrap();
    let budget = store.budget_reservations().unwrap().remove(0);
    let recovery = store.recover_vast("operation").unwrap();
    Frozen {
        operations: operations.len(),
        dispatched_at: operations[0].dispatched_at,
        invocation_started_at: operations[0].invocation_started_at,
        reserved_at: budget.reserved_at,
        reserved_all_in: budget.reserved_all_in,
        cleanup_deadline: recovery.cleanup_deadline().unwrap(),
    }
}
fn fail(store: &Store, trigger: &str) {
    store.conn.execute_batch(trigger).unwrap();
}
fn heal(store: &Store) {
    store
        .conn
        .execute_batch("DROP TRIGGER IF EXISTS injected;")
        .unwrap();
}
fn node_present(store: &Store) -> bool {
    !store.nodes().unwrap().is_empty()
}
fn billing(store: &Store) -> BudgetState {
    store.budget_reservations().unwrap().remove(0).state
}

#[test]
fn undispatched_and_uninvoked_operations_take_no_provider_effect_or_write() {
    for (stage, expected) in [
        (Stage::Prepared, Outcome::NotDispatched),
        (Stage::Consumed, Outcome::ConsumedNotInvoked),
    ] {
        let (store, _directory, _) = staged(stage);
        let before = store.conn.total_changes();
        let mut port = healthy();
        assert_eq!(
            only(pass(&store, &mut port, CleanupTrigger::Immediate, 200)),
            expected
        );
        assert!(port.calls.is_empty(), "{stage:?}");
        assert_eq!(store.conn.total_changes(), before);
        assert!(node_present(&store));
    }
}

#[test]
fn unresolved_lookup_is_never_absence_and_never_creates() {
    let (store, directory, _) = staged(Stage::Invoked);
    let original = frozen(&store);
    let mut store = store;
    for round in 0..4 {
        let mut port = Fake {
            lookup: [if round % 2 == 0 {
                Ok(None)
            } else {
                unavailable()
            }]
            .into(),
            // Anything after an unresolved lookup would be an effect on a guess.
            destroy: [Ok(())].into(),
            confirm: [Ok(Some((42, true)))].into(),
            ..Fake::default()
        };
        assert_eq!(
            only(pass(
                &store,
                &mut port,
                CleanupTrigger::Immediate,
                200 + round
            )),
            Outcome::CreateUnresolved
        );
        assert_eq!(port.calls, ["lookup"]);
        assert!(node_present(&store));
        assert!(store.vast_removals().unwrap().is_empty());
        assert!(store.nodes().unwrap()[0].instance_id.is_none());
        let op = store.create_operations().unwrap().remove(0);
        assert!(op.uncertainty_observed && op.result.is_none() && op.cleanup_required);
        assert_ne!(billing(&store), BudgetState::Settled);
        assert_eq!(frozen(&store), original);
        assert!(store.claim_create("operation", 300).is_err());
        store = reopen(store, &directory);
    }
}

#[test]
fn exact_lookup_binds_resource_and_node_together_then_cleans_up() {
    let (store, directory, _) = staged(Stage::Invoked);
    let original = frozen(&store);
    let mut port = healthy();
    assert_eq!(
        only(pass(&store, &mut port, CleanupTrigger::Immediate, 200)),
        Outcome::Removed
    );
    assert_eq!(port.calls, ["lookup", "destroy", "confirm"]);
    assert!(!node_present(&store));
    assert_eq!(billing(&store), BudgetState::BillingUnknown);
    assert_eq!(store.vast_removals().unwrap().len(), 1);
    assert_eq!(frozen(&store), original);
    let store = reopen(store, &directory);
    assert!(store.claim_create("operation", 300).is_err());
    let mut again = healthy();
    assert_eq!(
        only(pass(&store, &mut again, CleanupTrigger::Immediate, 300)),
        Outcome::Finished
    );
    assert!(again.calls.is_empty());
}

#[test]
fn lookup_binding_is_atomic_with_the_node_row() {
    let (store, directory, _) = staged(Stage::Invoked);
    fail(
        &store,
        "CREATE TRIGGER injected BEFORE UPDATE ON gpu_node BEGIN SELECT RAISE(ABORT,'x'); END;",
    );
    let mut port = healthy();
    assert_eq!(
        only(pass(&store, &mut port, CleanupTrigger::Immediate, 200)),
        Outcome::Refused
    );
    assert_eq!(port.calls, ["lookup"]);
    let op = store.create_operations().unwrap().remove(0);
    assert!(
        op.result.is_none(),
        "binding must roll back with the node update"
    );
    assert!(store.nodes().unwrap()[0].instance_id.is_none());
    assert!(store.budget_reservations().unwrap()[0].binding.is_none());
    heal(&store);
    let store = reopen(store, &directory);
    let mut port = healthy();
    assert_eq!(
        only(pass(&store, &mut port, CleanupTrigger::Immediate, 201)),
        Outcome::Removed
    );
}

#[test]
fn deadline_trigger_waits_for_original_deadline_or_down() {
    let (store, _directory, _) = staged(Stage::Bound);
    let mut port = healthy();
    assert_eq!(
        only(pass(&store, &mut port, CleanupTrigger::AtDeadline, 159)),
        Outcome::AwaitingCleanup
    );
    assert!(port.calls.is_empty());
    assert!(node_present(&store));
    // Down is a durable operator request, not a deadline renewal.
    store.scale("smoke", 0).unwrap();
    assert_eq!(
        only(pass(&store, &mut port, CleanupTrigger::AtDeadline, 159)),
        Outcome::Removed
    );
    assert_eq!(port.calls, ["destroy", "confirm"]);

    let (store, _directory, _) = staged(Stage::Bound);
    let mut port = healthy();
    assert_eq!(
        only(pass(&store, &mut port, CleanupTrigger::AtDeadline, 160)),
        Outcome::Removed
    );
}

#[test]
fn destroying_is_durable_before_any_provider_destroy() {
    let (store, directory, _) = staged(Stage::Bound);
    fail(
        &store,
        "CREATE TRIGGER injected BEFORE UPDATE ON gpu_node BEGIN SELECT RAISE(ABORT,'x'); END;",
    );
    let mut port = healthy();
    assert_eq!(
        only(pass(&store, &mut port, CleanupTrigger::Immediate, 200)),
        Outcome::Refused
    );
    assert!(port.calls.is_empty(), "no effect without durable custody");
    heal(&store);
    let store = reopen(store, &directory);
    let mut port = healthy();
    assert_eq!(
        only(pass(&store, &mut port, CleanupTrigger::Immediate, 201)),
        Outcome::Removed
    );
}

#[test]
fn destroy_and_absence_faults_retain_node_and_liability_until_exact_absence() {
    let (store, directory, _) = staged(Stage::Bound);
    let original = frozen(&store);
    let mut store = store;
    let script: Vec<(Fake, Outcome)> = vec![
        // destroy fails and nothing confirms absence
        (Fake::default(), Outcome::AbsenceUnconfirmed),
        // destroy acknowledged, still present/stopped
        (
            Fake {
                destroy: [Ok(())].into(),
                confirm: [Ok(None)].into(),
                ..Fake::default()
            },
            Outcome::AbsenceUnconfirmed,
        ),
        // destroy acknowledged, observation transport error
        (
            Fake {
                destroy: [Ok(())].into(),
                confirm: [unavailable()].into(),
                ..Fake::default()
            },
            Outcome::AbsenceUnconfirmed,
        ),
    ];
    for (mut port, expected) in script {
        assert_eq!(
            only(pass(&store, &mut port, CleanupTrigger::Immediate, 200)),
            expected
        );
        assert_eq!(port.calls, ["destroy", "confirm"]);
        assert!(node_present(&store));
        assert_eq!(store.nodes().unwrap()[0].phase, m8s_core::Phase::Destroying);
        assert!(store.vast_removals().unwrap().is_empty());
        assert_ne!(billing(&store), BudgetState::Settled);
        assert_ne!(billing(&store), BudgetState::BillingUnknown);
        assert_eq!(frozen(&store), original);
        store = reopen(store, &directory);
    }
    // A destroy error followed by exact absence is still absence.
    let mut port = Fake {
        confirm: [Ok(Some((42, true)))].into(),
        ..Fake::default()
    };
    assert_eq!(
        only(pass(&store, &mut port, CleanupTrigger::Immediate, 200)),
        Outcome::Removed
    );
    assert!(!node_present(&store));
}

#[test]
fn finalization_failure_rolls_back_and_retries_after_restart() {
    let (store, directory, _) = staged(Stage::Bound);
    fail(
        &store,
        "CREATE TRIGGER injected BEFORE INSERT ON vast_removal BEGIN SELECT RAISE(ABORT,'x'); END;",
    );
    let mut port = healthy();
    assert_eq!(
        only(pass(&store, &mut port, CleanupTrigger::Immediate, 200)),
        Outcome::Refused
    );
    assert!(node_present(&store), "row stays until the receipt commits");
    assert!(store.vast_removals().unwrap().is_empty());
    assert_ne!(billing(&store), BudgetState::BillingUnknown);
    heal(&store);
    let store = reopen(store, &directory);
    let mut port = healthy();
    assert_eq!(
        only(pass(&store, &mut port, CleanupTrigger::Immediate, 201)),
        Outcome::Removed
    );
    assert_eq!(store.vast_removals().unwrap().len(), 1);
}

#[test]
fn wrong_instance_or_account_receipt_never_finalizes() {
    for receipt in [(43, true), (42, false)] {
        let (store, _directory, _) = staged(Stage::Bound);
        let mut port = Fake {
            confirm: [Ok(Some(receipt))].into(),
            ..Fake::default()
        };
        assert_eq!(
            only(pass(&store, &mut port, CleanupTrigger::Immediate, 200)),
            Outcome::Refused
        );
        assert!(node_present(&store));
        assert!(store.vast_removals().unwrap().is_empty());
        assert_ne!(billing(&store), BudgetState::BillingUnknown);
    }
}

#[test]
fn missing_recovery_metadata_refuses_without_effects() {
    let (store, _directory, _) = staged(Stage::Bound);
    store.conn.execute("DELETE FROM vast_launch", []).unwrap();
    let mut port = healthy();
    assert_eq!(
        only(pass(&store, &mut port, CleanupTrigger::Immediate, 200)),
        Outcome::Refused
    );
    assert!(port.calls.is_empty());
    assert!(node_present(&store));
}

/// Every persisted write in the reconcile path fails once, at both unresolved
/// and bound stages. Each case must retain custody, then converge after reopen
/// to exactly one removal without a new claim or a moved deadline.
#[test]
fn every_persisted_transition_fails_closed_and_converges_after_restart() {
    let faults = [
        ("create_operation", "UPDATE"),
        ("budget_reservation", "UPDATE"),
        ("budget_account", "UPDATE"),
        ("gpu_node", "UPDATE"),
        ("gpu_node", "DELETE"),
        ("event", "INSERT"),
        ("vast_removal", "INSERT"),
    ];
    let mut fired = std::collections::BTreeSet::new();
    for stage in [Stage::Invoked, Stage::Bound] {
        for (table, verb) in faults {
            let (store, directory, _) = staged(stage);
            let original = frozen(&store);
            fail(
                &store,
                &format!(
                    "CREATE TRIGGER injected BEFORE {verb} ON {table} BEGIN SELECT RAISE(ABORT,'x'); END;"
                ),
            );
            let mut port = healthy();
            let first = only(pass(&store, &mut port, CleanupTrigger::Immediate, 200));
            // Either the write path was not reached for this stage, or it failed closed.
            if first != Outcome::Removed {
                fired.insert((table, verb));
                assert!(
                    node_present(&store) && store.vast_removals().unwrap().is_empty(),
                    "{stage:?} {table} {verb}: row removed without a committed receipt"
                );
                assert_ne!(
                    billing(&store),
                    BudgetState::BillingUnknown,
                    "{stage:?} {table} {verb}"
                );
            }
            assert_eq!(frozen(&store), original, "{stage:?} {table} {verb}");
            heal(&store);
            let store = reopen(store, &directory);
            // The provider may have destroyed on the first pass: absence is still
            // required fresh, and CREATE is never offered to the port at all.
            let mut port = Fake {
                lookup: [Ok(Some(42))].into(),
                destroy: [Ok(())].into(),
                confirm: [Ok(Some((42, true)))].into(),
                ..Fake::default()
            };
            let second = only(pass(&store, &mut port, CleanupTrigger::Immediate, 201));
            assert!(
                matches!(second, Outcome::Removed | Outcome::Finished),
                "{stage:?} {table} {verb}: {second:?}"
            );
            assert!(!node_present(&store), "{stage:?} {table} {verb}");
            assert_eq!(store.vast_removals().unwrap().len(), 1);
            assert_eq!(billing(&store), BudgetState::BillingUnknown);
            assert_eq!(frozen(&store), original, "{stage:?} {table} {verb}");
            assert!(store.claim_create("operation", 300).is_err());
            assert!(!port.calls.contains(&"create"));
        }
    }
    assert_eq!(fired.len(), faults.len(), "a fault never fired: {fired:?}");
}
