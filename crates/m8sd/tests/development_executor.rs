//! Independent PUBLIC-API checks for the sealed synthetic executor.
//! Direct SQL only seeds an owned Launching fixture; it never mints claims.
//! No private hooks, providers, processes, networking, TLS or real evidence.
#![cfg(unix)]
use m8sd::{
    Store,
    budget::*,
    operation::{
        CreateIntent, CreateOperation, CreateState,
        development::{DevelopmentExecutor, DevelopmentOutcome, SyntheticPlan},
    },
};
use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const OWNER: &str = "synthetic-owner";
const OP: &str = "operation-a";
const RESERVED: u64 = 688;
const HOURLY: u64 = 123840;
static NEXT: AtomicU64 = AtomicU64::new(0);
fn period() -> CalendarPeriod {
    CalendarPeriod {
        id: "synthetic-month".into(),
        starts_at: 0,
        ends_at: 28 * 86400,
    }
}
fn policy() -> AdminBudget {
    AdminBudget {
        owner: OWNER.into(),
        approval_id: "synthetic-not-real-approval".into(),
        baseline: AccountBaseline {
            prior_lifetime_spent: 0,
            prior_current_period_spent: 0,
            initial_period: period(),
            external_liability: ExternalLiability::NoneConfirmed,
            observed_at: 100,
            status: EvidenceStatus::Complete,
            provenance: "synthetic-complete-baseline".into(),
        },
        valid_from: 0,
        valid_until: 86400,
        trial_all_in: 10000,
        monthly_all_in: 10000,
        total_hourly: 2_000_000,
        instance_hourly: 1_000_000,
        max_instances: 2,
        max_runtime_seconds: 1000,
        minimum_cleanup_seconds: 60,
        minimum_contingency: 100,
    }
}
fn request(id: &str) -> BudgetRequest {
    BudgetRequest {
        owner: OWNER.into(),
        run_id: "synthetic-run".into(),
        request_id: format!("request-{id}"),
        node_uid: format!("node-{id}"),
        quote: CostQuote {
            provenance: "synthetic-normalized-costs".into(),
            observed_at: 100,
            valid_until: 2000,
            status: EvidenceStatus::Complete,
            compute_unit: RateUnit::MicroUsdPerHour,
            storage_unit: RateUnit::MicroUsdPerHour,
            totals_unit: TotalUnit::MicroUsd,
            compute_hourly: 3600,
            storage_hourly: 360,
            transfer_total: 500,
            runtime_seconds: 20,
            cleanup_seconds: 60,
            contingency: 100,
        },
    }
}
fn unsigned_intent() -> CreateIntent {
    CreateIntent {
        owner: OWNER.into(),
        run_id: "synthetic-run".into(),
        request_id: "request-a".into(),
        node_uid: "node-a".into(),
        operation_id: OP.into(),
        launch_fingerprint: [0; 32],
    }
}
fn plan() -> SyntheticPlan {
    SyntheticPlan::new("namespace", "target", "tier").unwrap()
}
fn intent() -> CreateIntent {
    let mut i = unsigned_intent();
    i.launch_fingerprint = plan().fingerprint(&i).unwrap();
    i
}
fn executor() -> DevelopmentExecutor {
    DevelopmentExecutor::new_synthetic(plan()).unwrap()
}
struct Fixture {
    dir: PathBuf,
    db: PathBuf,
    policy: AdminBudget,
}
impl Fixture {
    fn new(policy: AdminBudget) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "m8sd-development-public-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        Self {
            db: dir.join("state.db"),
            dir,
            policy,
        }
    }
    fn open(&self) -> Store {
        Store::open(&self.db, OWNER).unwrap()
    }
    fn seeded(&self) -> Store {
        let store = self.open();
        store
            .initialize_budget(&self.policy, &period(), 100)
            .unwrap();
        store.scale("tier", 1).unwrap();
        // Synthetic fixture only. No production launch/admission API is claimed.
        Connection::open(&self.db).unwrap().execute(
            "INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES ('node-a','tier','\"Launching\"',100)", [],
        ).unwrap();
        let r = store
            .reserve_budget(&self.policy, &period(), &request("a"), 100)
            .unwrap();
        assert_eq!((r.reserved_all_in, r.reserved_hourly), (RESERVED, HOURLY));
        store
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn operation(store: &Store) -> CreateOperation {
    store
        .create_operations()
        .unwrap()
        .into_iter()
        .find(|r| r.intent.operation_id == OP)
        .unwrap()
}
fn reservation(store: &Store) -> BudgetReservation {
    store
        .budget_reservations()
        .unwrap()
        .into_iter()
        .find(|r| r.request.request_id == "request-a")
        .unwrap()
}
fn independent_operation(path: &Path) -> CreateOperation {
    let read = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let json: String = read
        .query_row(
            "SELECT record FROM create_operation WHERE owner=? AND operation_id=?",
            params![OWNER, OP],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&json).unwrap() // Recovery metadata, not an opaque claim.
}
fn independent_reservation(path: &Path) -> BudgetReservation {
    let read = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let json: String = read
        .query_row(
            "SELECT record FROM budget_reservation WHERE owner=? AND request_id='request-a'",
            [OWNER],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&json).unwrap()
}
fn no_invocation(e: &DevelopmentExecutor, store: &Store) {
    assert_eq!(e.invocations(), 0);
    assert!(e.resources().is_empty());
    assert!(operation(store).invocation_started_at.is_none());
    assert!(operation(store).result.is_none());
    let b = reservation(store);
    assert_eq!(b.state, BudgetState::Uncertain);
    assert_eq!(b.reserved_all_in, RESERVED);
    assert!(b.settlement.is_none());
}

#[test]
fn origin_substitution_rejects_same_database_separate_identical_database_and_reopen() {
    for case in ["second-handle", "separate-identical", "reopened"] {
        let f = Fixture::new(policy());
        let other = Fixture::new(policy());
        let mut original = Some(f.seeded());
        let mut e = executor();
        e.prepare(original.as_ref().unwrap(), &intent(), 100)
            .unwrap();
        let claim = e.claim(original.as_ref().unwrap(), OP, 101).unwrap();
        let before = operation(original.as_ref().unwrap());
        let substitute = match case {
            "separate-identical" => {
                let store = other.seeded();
                e.prepare(&store, &intent(), 100).unwrap();
                // Match durable consumed state via REAL public issuance, so a
                // Prepared-vs-consumed mismatch cannot hide missing origin checks.
                drop(e.claim(&store, OP, 101).unwrap());
                store
            }
            "reopened" => {
                drop(original.take());
                f.open()
            }
            _ => f.open(),
        };
        assert_eq!(operation(&substitute), before);
        assert!(e.execute(&substitute, claim, 102).is_err());
        no_invocation(&e, &substitute);
        assert!(e.claim(&substitute, OP, 103).is_err());
    }
}

#[test]
fn replacement_identical_executor_and_generic_unbound_claim_are_not_authority() {
    for generic in [false, true] {
        let f = Fixture::new(policy());
        let store = f.seeded();
        let issuer = executor();
        let mut replacement = executor();
        issuer.prepare(&store, &intent(), 100).unwrap();
        let claim = if generic {
            store.claim_create(OP, 101).unwrap()
        } else {
            issuer.claim(&store, OP, 101).unwrap()
        };
        if generic {
            // Even the correct plan's executor rejects a normal unbound claim.
            let mut issuer = issuer;
            assert!(issuer.execute(&store, claim, 102).is_err());
            no_invocation(&issuer, &store);
        } else {
            assert!(replacement.execute(&store, claim, 102).is_err());
            no_invocation(&replacement, &store);
            assert_eq!(issuer.invocations(), 0);
        }
        assert!(replacement.claim(&store, OP, 103).is_err());
    }
}

#[test]
fn canonical_fingerprint_is_length_prefixed_and_wrong_plan_or_computed_wrong_tier_refuses() {
    let i = unsigned_intent();
    let a = SyntheticPlan::new("ab", "c", "tier").unwrap();
    let b = SyntheticPlan::new("a", "bc", "tier").unwrap();
    assert_ne!(a.fingerprint(&i).unwrap(), b.fingerprint(&i).unwrap());
    // Independent literal preimage, not the production encoder reused as oracle.
    let mut golden = Sha256::new();
    golden.update(b"m8s-development-plan\0");
    golden.update([0, 1]);
    for field in [
        "SyntheticV1",
        "namespace",
        "target",
        "tier",
        "synthetic-owner",
        "synthetic-run",
        "request-a",
        "node-a",
        "operation-a",
    ] {
        golden.update(u32::try_from(field.len()).unwrap().to_be_bytes());
        golden.update(field.as_bytes());
    }
    let expected: [u8; 32] = golden.finalize().into();
    assert_eq!(plan().fingerprint(&i).unwrap(), expected);
    assert_eq!(
        plan().fingerprint(&intent()).unwrap(),
        expected,
        "output fingerprint is not recursively hashed"
    );
    for values in [
        ("namespace-2", "target", "tier"),
        ("namespace", "target-2", "tier"),
        ("namespace", "target", "wrong-tier"),
    ] {
        let f = Fixture::new(policy());
        let store = f.seeded();
        let good = executor();
        good.prepare(&store, &intent(), 100).unwrap();
        let wrong = DevelopmentExecutor::new_synthetic(
            SyntheticPlan::new(values.0, values.1, values.2).unwrap(),
        )
        .unwrap();
        assert!(wrong.prepare(&store, &intent(), 100).is_err());
        assert!(wrong.claim(&store, OP, 101).is_err());
        assert_eq!(wrong.invocations(), 0);
        assert_eq!(operation(&store).state, CreateState::Prepared);
        assert_eq!(reservation(&store).state, BudgetState::Reserved);
    }
    let f = Fixture::new(policy());
    let store = f.seeded();
    let wrong_tier = SyntheticPlan::new("namespace", "target", "wrong-tier").unwrap();
    let mut wrong_intent = i;
    wrong_intent.launch_fingerprint = wrong_tier.fingerprint(&wrong_intent).unwrap();
    let wrong = DevelopmentExecutor::new_synthetic(wrong_tier).unwrap();
    assert!(wrong.prepare(&store, &wrong_intent, 100).is_err());
    assert!(store.create_operations().unwrap().is_empty());
    assert_eq!(wrong.invocations(), 0);
    assert!(SyntheticPlan::new("", "target", "tier").is_err());
    assert!(SyntheticPlan::new("namespace", "target\0", "tier").is_err());
    assert!(SyntheticPlan::new(&"n".repeat(65), "target", "tier").is_err());
}

#[test]
fn correct_executor_invokes_once_and_persists_marker_binding_and_full_uncertain_hold() {
    let f = Fixture::new(policy());
    let store = f.seeded();
    let mut e = executor();
    e.prepare(&store, &intent(), 100).unwrap();
    let claim = e.claim(&store, OP, 101).unwrap();
    assert_eq!(claim.serving_deadline(), 120);
    assert_eq!(claim.cleanup_deadline(), 180);
    let before = independent_operation(&f.db);
    let before_hold = independent_reservation(&f.db);
    assert_eq!(before_hold.state, BudgetState::Uncertain);
    assert_eq!(before_hold.reserved_all_in, RESERVED);
    assert_eq!(before.state, CreateState::DispatchConsumed);
    assert!(before.invocation_started_at.is_none());
    assert_eq!(reservation(&store).state, BudgetState::Uncertain);
    let DevelopmentOutcome::Bound(returned) = e.execute(&store, claim, 102).unwrap() else {
        panic!("expected synthetic bound result")
    };
    assert_eq!(e.invocations(), 1);
    assert_eq!(e.resources().len(), 1);
    let persisted = independent_operation(&f.db);
    assert_eq!(*returned, persisted);
    assert_eq!(persisted.invocation_started_at, Some(102));
    assert!(persisted.cleanup_required);
    assert_eq!(persisted.result.as_ref(), Some(e.resources()[0].binding()));
    let hold = independent_reservation(&f.db);
    assert_eq!(hold, reservation(&store));
    assert_eq!(hold.binding, persisted.result);
    assert_eq!(hold.state, BudgetState::Uncertain);
    assert_eq!(hold.reserved_all_in, RESERVED);
    assert!(store.claim_create(OP, 103).is_err());
    assert!(e.claim(&store, OP, 103).is_err());
    assert_eq!(e.prepare(&store, &intent(), 103).unwrap(), persisted);
    assert_eq!(e.invocations(), 1);
    assert_eq!(store.nodes().unwrap()[0].phase, m8s_core::Phase::Launching);
    // Public return-time visibility only; no cfg(test) hook is used to claim
    // independent observation between marker commit and backend entry.
}

#[test]
fn postclaim_preexecute_cancel_or_down_produces_zero_invocations_and_no_refund() {
    for down in [false, true] {
        let f = Fixture::new(policy());
        let store = f.seeded();
        let mut e = executor();
        e.prepare(&store, &intent(), 100).unwrap();
        let claim = e.claim(&store, OP, 101).unwrap();
        if down {
            store.down().unwrap();
            assert!(e.execute(&store, claim, 102).is_err());
            assert_eq!(store.tiers().unwrap()[0].1, 0);
        } else {
            store.cancel_create(OP, 102).unwrap();
            assert!(matches!(
                e.execute(&store, claim, 103).unwrap(),
                DevelopmentOutcome::NotInvoked
            ));
            assert_eq!(operation(&store).state, CreateState::Cancelled);
        }
        no_invocation(&e, &store);
        assert!(e.claim(&store, OP, 104).is_err());
    }
}

#[test]
fn serving_deadline_is_anchored_and_virtual_cleanup_is_not_provider_absence_or_refund() {
    for now in [119, 120] {
        let f = Fixture::new(policy());
        let store = f.seeded();
        let mut e = executor();
        e.prepare(&store, &intent(), 100).unwrap();
        let claim = e.claim(&store, OP, 101).unwrap();
        if now == 120 {
            // Quote age20 remains fresh, isolating serving equality rejection.
            assert!(e.execute(&store, claim, now).is_err());
            no_invocation(&e, &store);
            continue;
        }
        assert!(matches!(
            e.execute(&store, claim, now).unwrap(),
            DevelopmentOutcome::Bound(_)
        ));
        assert_eq!(e.invocations(), 1);
        let r = &e.resources()[0];
        assert_eq!(r.serving_deadline(), 120);
        assert_eq!(r.cleanup_deadline(), 180);
        assert!(!r.service_eligible(118));
        assert!(r.service_eligible(119));
        assert!(!r.service_eligible(120));
        assert!(!r.is_removed());
        e.advance(120).unwrap();
        assert!(!e.resources()[0].is_removed());
        assert!(e.advance(119).is_err(), "fake clock rollback must refuse");
        e.advance(179).unwrap();
        assert!(!e.resources()[0].is_removed());
        e.advance(180).unwrap();
        assert!(e.resources()[0].is_removed());
        assert!(!e.resources()[0].service_eligible(119));
        assert_eq!(reservation(&store).state, BudgetState::Uncertain);
        assert!(reservation(&store).settlement.is_none());
        assert_eq!(store.nodes().unwrap().len(), 1);
        assert!(operation(&store).cleanup_required);
        assert_eq!(e.invocations(), 1);
    }
}

#[test]
fn exact_cap_consumed_own_hold_is_charged_once_not_twice() {
    let mut caps = policy();
    caps.trial_all_in = RESERVED;
    caps.monthly_all_in = RESERVED;
    caps.total_hourly = HOURLY;
    caps.instance_hourly = HOURLY;
    caps.max_instances = 1;
    let f = Fixture::new(caps);
    let store = f.seeded();
    let mut e = executor();
    e.prepare(&store, &intent(), 100).unwrap();
    let claim = e.claim(&store, OP, 101).unwrap();
    assert!(matches!(
        e.execute(&store, claim, 102).unwrap(),
        DevelopmentOutcome::Bound(_)
    ));
    assert_eq!(e.invocations(), 1);
    assert_eq!(reservation(&store).state, BudgetState::Uncertain);
    assert_eq!(reservation(&store).reserved_all_in, RESERVED);
}

#[test]
fn other_unknown_or_billing_unknown_and_late_bill_cannot_be_omitted_by_own_exception() {
    for state in [
        "healthy-control",
        "uncertain",
        "billing-unknown",
        "late-lifetime",
        "late-monthly",
    ] {
        let mut caps = policy();
        if state == "late-lifetime" {
            caps.trial_all_in = 1500;
        }
        if state == "late-monthly" {
            caps.monthly_all_in = 1500;
        }
        let f = Fixture::new(caps);
        let store = f.seeded();
        let mut e = executor();
        store
            .reserve_budget(&f.policy, &period(), &request("b"), 100)
            .unwrap(); // 688+688<=1500
        e.prepare(&store, &intent(), 100).unwrap();
        let claim = e.claim(&store, OP, 101).unwrap();
        if state == "healthy-control" {
            assert!(matches!(
                e.execute(&store, claim, 102).unwrap(),
                DevelopmentOutcome::Bound(_)
            ));
            assert_eq!(e.invocations(), 1);
            continue;
        }
        if state == "uncertain" {
            store.mark_budget_uncertain("request-b", 102).unwrap();
        } else {
            // Public budget evidence is an explicit trusted synthetic assertion,
            // not fabricated TLS/claim authority or evidence of real B effects.
            let b = ResourceBinding {
                owner: OWNER.into(),
                run_id: "synthetic-run".into(),
                node_uid: "node-b".into(),
                instance_id: "synthetic-prior-b".into(),
                volumes: VolumeBinding::NoneAllocated,
                provenance: "synthetic-complete-prior-resource-binding".into(),
                observed_at: 102,
                status: EvidenceStatus::Complete,
            };
            store.bind_budget_resources("request-b", &b, 102).unwrap();
            let billing = if state == "billing-unknown" {
                FinalBilling::Unknown
            } else {
                FinalBilling::Known {
                    actual_all_in: 1000,
                    provenance: "synthetic-late-final-bill".into(),
                }
            };
            let evidence = SettlementEvidence {
                binding: b,
                observed_at: 103,
                status: EvidenceStatus::Complete,
                resource: ResourceOutcome::OwnedAbsent,
                volumes: VolumeOutcome::BoundVolumesAbsent,
                billing,
                provenance: "synthetic-exact-prior-resource-absence".into(),
            };
            store
                .settle_budget("request-b", &period(), &evidence, 103)
                .unwrap();
        }
        // For each isolated late-bill cap, 688+1000>1500 but1000 alone fits: omitting the
        // target's consumed hold would incorrectly allow this sealed invocation.
        assert!(e.execute(&store, claim, 104).is_err());
        no_invocation(&e, &store);
        assert!(e.claim(&store, OP, 105).is_err());
    }
}
