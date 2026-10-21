//! Test-only scripted effects, not a production provider/executor or permission API.
//! Launching rows below are explicit owned SQLite fixtures, NOT admission proof.
//! No processes, sockets, TLS, credentials, provider clients or actual spending.
#![cfg(unix)]
use m8sd::{
    Store,
    budget::*,
    operation::{CreateClaim, CreateIntent, CreateObservation, CreateOperation, CreateState},
};
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const OWNER: &str = "synthetic-owner";
const RUN: &str = "synthetic-run";
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
        total_hourly: 20000,
        instance_hourly: 10000,
        max_instances: 2,
        max_runtime_seconds: 1000,
        minimum_cleanup_seconds: 60,
        minimum_contingency: 100,
    }
}
fn request(id: &str) -> BudgetRequest {
    BudgetRequest {
        owner: OWNER.into(),
        run_id: RUN.into(),
        request_id: format!("request-{id}"),
        node_uid: format!("node-{id}"),
        quote: CostQuote {
            provenance: "synthetic-normalized-all-components".into(),
            observed_at: 100,
            valid_until: 2000,
            status: EvidenceStatus::Complete,
            compute_unit: RateUnit::MicroUsdPerHour,
            storage_unit: RateUnit::MicroUsdPerHour,
            totals_unit: TotalUnit::MicroUsd,
            compute_hourly: 3600,
            storage_hourly: 360,
            transfer_total: 500,
            runtime_seconds: 1000,
            cleanup_seconds: 60,
            contingency: 100,
        },
    }
}
fn intent(id: &str) -> CreateIntent {
    let r = request(id);
    CreateIntent {
        owner: r.owner,
        run_id: r.run_id,
        request_id: r.request_id,
        node_uid: r.node_uid,
        operation_id: format!("operation-{id}"),
        launch_fingerprint: [7; 32],
    }
}
struct Fixture {
    dir: PathBuf,
    db: PathBuf,
    fake: PathBuf,
    policy: AdminBudget,
}
impl Fixture {
    fn new(policy: AdminBudget) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "m8sd-operation-fake-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        Self {
            db: dir.join("state.db"),
            fake: dir.join("fake.json"),
            dir,
            policy,
        }
    }
    fn open(&self) -> Store {
        Store::open(&self.db, OWNER).unwrap()
    }
    fn seed(&self, store: &Store, id: &str, now: i64) {
        store.scale("synthetic", 1).unwrap();
        // Only fixture creation uses direct SQL. All operation/budget transitions
        // go through public Store APIs, never private record/capability minting.
        Connection::open(&self.db).unwrap().execute(
            "INSERT INTO gpu_node(uid,tier,phase,created_at) VALUES (?,'synthetic','\"Launching\"',?)",
            params![request(id).node_uid, now],
        ).unwrap();
        assert_eq!(
            store
                .reserve_budget(&self.policy, &period(), &request(id), now)
                .unwrap()
                .reserved_all_in,
            1766
        );
    }
    fn prepared(&self) -> Store {
        let store = self.open();
        self.seed(&store, "a", 100);
        store.prepare_create(&intent("a"), 100).unwrap();
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
        .find(|r| r.intent == intent("a"))
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

#[derive(Default, Serialize, Deserialize)]
struct FakeState {
    // Incremented/persisted BEFORE resource persistence; no idempotent dedupe.
    calls: Vec<CreateIntent>,
    resources: BTreeMap<String, FakeResource>,
    cleanup_calls: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct FakeResource {
    binding: ResourceBinding,
    instance_present: bool,
    volumes: BTreeMap<String, bool>,
}
struct Fake {
    path: PathBuf,
    state: FakeState,
}
#[derive(Clone, Copy)]
enum Reply {
    Bound,
    Lost,
}
impl Fake {
    fn open(path: &Path) -> Self {
        let state = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => FakeState::default(),
            Err(_) => panic!("synthetic state read failed"),
        };
        Self {
            path: path.into(),
            state,
        }
    }
    fn save(&self) {
        let tmp = self.path.with_extension("new");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .unwrap();
        file.write_all(&serde_json::to_vec(&self.state).unwrap())
            .unwrap();
        file.sync_all().unwrap();
        drop(file);
        fs::rename(tmp, &self.path).unwrap();
        fs::File::open(self.path.parent().unwrap())
            .unwrap()
            .sync_all()
            .unwrap();
    }
    fn execute(
        &mut self,
        claim: CreateClaim,
        db: &Path,
        now: i64,
        reply: Reply,
    ) -> Result<ResourceBinding, &'static str> {
        // Real opaque claim is moved into this executor. Its immutable original
        // deadlines are enforced, never extended by delayed dispatch or recovery.
        assert_eq!(claim.serving_deadline(), 1100);
        assert_eq!(claim.cleanup_deadline(), 1160);
        if now >= claim.serving_deadline() {
            return Err("synthetic deadline elapsed");
        }
        let intent = claim.intent().clone();
        // Independent connection observes COMMITTED dispatch+budget before any
        // fake effect. Deserialize recovery metadata, never a CreateClaim.
        let read = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let json: String = read
            .query_row(
                "SELECT record FROM create_operation WHERE owner=? AND operation_id=?",
                params![intent.owner, intent.operation_id],
                |r| r.get(0),
            )
            .unwrap();
        let op: CreateOperation = serde_json::from_str(&json).unwrap();
        assert_eq!(op.intent, intent);
        assert!(op.dispatched_at.is_some());
        assert!(matches!(
            op.state,
            CreateState::DispatchConsumed | CreateState::Cancelled
        ));
        assert!(op.cleanup_required);
        let json: String = read
            .query_row(
                "SELECT record FROM budget_reservation WHERE owner=? AND request_id=?",
                params![intent.owner, intent.request_id],
                |r| r.get(0),
            )
            .unwrap();
        let budget: BudgetReservation = serde_json::from_str(&json).unwrap();
        assert_eq!(budget.state, BudgetState::Uncertain);
        assert_eq!(budget.request.node_uid, intent.node_uid);
        assert_eq!(budget.request.run_id, intent.run_id);
        assert_eq!(budget.reserved_all_in, 1766);
        drop(read);
        // A second writer can acquire IMMEDIATE while in the executor callback:
        // no operation transaction remains open across even this synthetic I/O.
        let write = Connection::open(db).unwrap();
        write.busy_timeout(Duration::ZERO).unwrap();
        write.execute_batch("BEGIN IMMEDIATE; ROLLBACK;").unwrap();
        drop(write);
        self.state.calls.push(intent.clone());
        self.save();
        let number = self.state.calls.len();
        let binding = ResourceBinding {
            owner: intent.owner,
            run_id: intent.run_id,
            node_uid: intent.node_uid,
            instance_id: format!("synthetic-instance-{number}"),
            volumes: VolumeBinding::Ids(vec![
                format!("synthetic-volume-{number}-a"),
                format!("synthetic-volume-{number}-b"),
            ]),
            provenance: "synthetic-complete-resource-manifest".into(),
            observed_at: now,
            status: EvidenceStatus::Complete,
        };
        self.insert(binding.clone());
        self.save();
        match reply {
            Reply::Bound => Ok(binding),
            Reply::Lost => Err("synthetic response lost"),
        }
    }
    fn insert(&mut self, binding: ResourceBinding) {
        let VolumeBinding::Ids(ids) = &binding.volumes else {
            panic!("fixture requires explicit IDs")
        };
        self.state.resources.insert(
            binding.instance_id.clone(),
            FakeResource {
                volumes: ids.iter().cloned().map(|id| (id, true)).collect(),
                binding,
                instance_present: true,
            },
        );
    }
    fn recover(&self, intent: &CreateIntent) -> ResourceBinding {
        let found: Vec<_> = self
            .state
            .resources
            .values()
            .filter(|r| {
                r.binding.owner == intent.owner
                    && r.binding.run_id == intent.run_id
                    && r.binding.node_uid == intent.node_uid
            })
            .collect();
        assert_eq!(found.len(), 1);
        found[0].binding.clone()
    }
    // These methods are a scripted fake world, NOT production cleanup authority.
    // ACKs do not change presence. Explicit scoped transitions simulate later
    // complete observations, not successful provider deletion or billing proof.
    fn resource(&mut self, binding: &ResourceBinding) -> &mut FakeResource {
        let r = self.state.resources.get_mut(&binding.instance_id).unwrap();
        assert_eq!(&r.binding, binding);
        r
    }
    fn acknowledge(&mut self, binding: &ResourceBinding) {
        self.resource(binding);
        self.state.cleanup_calls.push(binding.instance_id.clone());
        self.save();
    }
    fn instance_absent(&mut self, binding: &ResourceBinding) {
        self.resource(binding).instance_present = false;
        self.save();
    }
    fn volume_absent(&mut self, binding: &ResourceBinding, id: &str) {
        *self.resource(binding).volumes.get_mut(id).unwrap() = false;
        self.state.cleanup_calls.push(id.into());
        self.save();
    }
    fn evidence(
        &self,
        binding: &ResourceBinding,
        billing: FinalBilling,
        now: i64,
    ) -> SettlementEvidence {
        let r = &self.state.resources[&binding.instance_id];
        assert_eq!(&r.binding, binding);
        SettlementEvidence {
            binding: binding.clone(),
            observed_at: now,
            status: EvidenceStatus::Complete,
            resource: if r.instance_present {
                ResourceOutcome::Unknown
            } else {
                ResourceOutcome::OwnedAbsent
            },
            volumes: if r.volumes.values().all(|present| !present) {
                VolumeOutcome::BoundVolumesAbsent
            } else {
                VolumeOutcome::Unknown
            },
            billing,
            provenance: "synthetic-exact-owned-absence-observation".into(),
        }
    }
}
fn known(actual_all_in: u64) -> FinalBilling {
    FinalBilling::Known {
        actual_all_in,
        provenance: "synthetic-final-all-in-bill".into(),
    }
}

#[test]
fn committed_claim_and_uncertain_budget_precede_fake_effect_without_open_transaction() {
    let f = Fixture::new(policy());
    let store = f.prepared();
    let mut fake = Fake::open(&f.fake);
    let claim = store.claim_create("operation-a", 101).unwrap();
    let binding = fake.execute(claim, &f.db, 102, Reply::Bound).unwrap();
    store
        .record_create_result(
            "operation-a",
            &CreateObservation::Bound(binding.clone()),
            103,
        )
        .unwrap();
    assert_eq!(operation(&store).result, Some(binding.clone()));
    assert_eq!(reservation(&store).binding, Some(binding));
    assert_eq!(reservation(&store).state, BudgetState::Uncertain);
    // Replay metadata is obtainable, but no new opaque claim is minted.
    assert_eq!(
        store.prepare_create(&intent("a"), 104).unwrap(),
        operation(&store)
    );
    assert_eq!(
        store
            .reserve_budget(&f.policy, &period(), &request("a"), 104)
            .unwrap(),
        reservation(&store)
    );
    assert!(store.claim_create("operation-a", 104).is_err());
    assert_eq!(Fake::open(&f.fake).state.calls, vec![intent("a")]);
}

#[test]
fn lost_response_and_restart_recover_exact_binding_but_never_remint() {
    // Healthy control proves default synthetic caps have room for B; uncertainty
    // below, not a one-instance or monetary cap, freezes the new reservation.
    let control = Fixture::new(policy());
    let healthy = control.prepared();
    assert!(
        healthy
            .reserve_budget(&control.policy, &period(), &request("b"), 101)
            .is_ok()
    );
    let f = Fixture::new(policy());
    let store = f.prepared();
    let mut fake = Fake::open(&f.fake);
    let claim = store.claim_create("operation-a", 101).unwrap();
    assert!(fake.execute(claim, &f.db, 102, Reply::Lost).is_err());
    store
        .record_create_result("operation-a", &CreateObservation::Ambiguous, 103)
        .unwrap();
    drop(store);
    drop(fake);
    let store = f.open();
    let fake = Fake::open(&f.fake);
    assert_eq!(fake.state.calls.len(), 1);
    assert_eq!(fake.state.resources.len(), 1);
    assert!(operation(&store).uncertainty_observed);
    assert!(store.claim_create("operation-a", 104).is_err());
    assert_eq!(
        store.prepare_create(&intent("a"), 104).unwrap().state,
        CreateState::DispatchConsumed
    );
    let binding = fake.recover(&intent("a"));
    store
        .record_create_result(
            "operation-a",
            &CreateObservation::Bound(binding.clone()),
            104,
        )
        .unwrap();
    assert_eq!(reservation(&store).binding, Some(binding));
    assert_eq!(reservation(&store).state, BudgetState::Uncertain);
    assert!(
        store
            .reserve_budget(&f.policy, &period(), &request("b"), 105)
            .is_err()
    );
    assert_eq!(Fake::open(&f.fake).state.calls.len(), 1);
}

#[test]
fn restart_after_consumed_claim_before_dispatch_does_not_recover_permission() {
    let f = Fixture::new(policy());
    let store = f.prepared();
    let claim = store.claim_create("operation-a", 101).unwrap();
    drop(claim);
    drop(store);
    let store = f.open();
    assert_eq!(operation(&store).state, CreateState::DispatchConsumed);
    assert!(operation(&store).cleanup_required);
    assert_eq!(reservation(&store).state, BudgetState::Uncertain);
    assert!(store.claim_create("operation-a", 102).is_err());
    assert!(
        store
            .reserve_budget(&f.policy, &period(), &request("b"), 102)
            .is_err()
    );
    assert!(Fake::open(&f.fake).state.calls.is_empty());
}

#[test]
fn cancel_or_down_before_claim_prevents_dispatch_but_does_not_refund() {
    for down in [false, true] {
        let f = Fixture::new(policy());
        let store = f.prepared();
        if down {
            store.down().unwrap();
        } else {
            store.cancel_create("operation-a", 101).unwrap();
        }
        assert!(store.claim_create("operation-a", 102).is_err());
        assert_eq!(reservation(&store).reserved_all_in, 1766);
        assert!(reservation(&store).settlement.is_none());
        assert!(Fake::open(&f.fake).state.calls.is_empty());
    }
}

#[test]
fn postclaim_cancel_does_not_revoke_issued_claim_and_late_result_is_cleanup_only() {
    let f = Fixture::new(policy());
    let store = f.prepared();
    let mut fake = Fake::open(&f.fake);
    let claim = store.claim_create("operation-a", 101).unwrap();
    store.down().unwrap();
    store.cancel_create("operation-a", 102).unwrap();
    // Issued claim is not remotely/retractably revoked. Dispatch can still have
    // happened; record that exact late result for cleanup, never launch success.
    let binding = fake.execute(claim, &f.db, 103, Reply::Bound).unwrap();
    store
        .record_create_result(
            "operation-a",
            &CreateObservation::Bound(binding.clone()),
            104,
        )
        .unwrap();
    drop(store);
    let store = f.open();
    let op = operation(&store);
    assert_eq!(op.state, CreateState::Cancelled);
    assert!(op.cleanup_required);
    assert_eq!(op.result, Some(binding));
    assert_eq!(store.tiers().unwrap()[0].1, 0);
    assert_eq!(store.nodes().unwrap()[0].phase, m8s_core::Phase::Launching);
    let deleting: bool = Connection::open(&f.db)
        .unwrap()
        .query_row(
            "SELECT deleting FROM gpu_node WHERE uid='node-a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(deleting);
    assert!(store.claim_create("operation-a", 105).is_err());
    assert_eq!(reservation(&store).state, BudgetState::Uncertain);
    assert_eq!(fake.state.calls.len(), 1);
}

#[test]
fn bound_volume_cleanup_and_unknown_billing_cannot_refund_then_exact_final_bill_admits() {
    let mut caps = policy();
    caps.trial_all_in = 3000;
    caps.monthly_all_in = 3000;
    let f = Fixture::new(caps);
    let store = f.prepared();
    let mut fake = Fake::open(&f.fake);
    let binding = fake
        .execute(
            store.claim_create("operation-a", 101).unwrap(),
            &f.db,
            102,
            Reply::Bound,
        )
        .unwrap();
    store
        .record_create_result(
            "operation-a",
            &CreateObservation::Bound(binding.clone()),
            103,
        )
        .unwrap();
    // Pre-existing fake decoys, not extra claim-backed creates. Operation cleanup
    // must not become a broad owner-list/orphan cleanup. No real volume API exists.
    for (owner, run, uid, instance) in [
        ("foreign-owner", RUN, "node-a", "foreign-instance"),
        (OWNER, "different-run", "different-uid", "same-owner-decoy"),
    ] {
        let mut decoy = binding.clone();
        decoy.owner = owner.into();
        decoy.run_id = run.into();
        decoy.node_uid = uid.into();
        decoy.instance_id = instance.into();
        decoy.volumes = VolumeBinding::Ids(vec![format!("{instance}-volume")]);
        fake.insert(decoy);
    }
    fake.save();
    fake.acknowledge(&binding);
    let mut ack = fake.evidence(&binding, known(0), 104);
    ack.resource = ResourceOutcome::DestroyAcknowledged;
    ack.volumes = VolumeOutcome::DeleteAcknowledged;
    assert!(
        store
            .settle_budget("request-a", &period(), &ack, 104)
            .is_err()
    );
    fake.instance_absent(&binding);
    let VolumeBinding::Ids(ids) = &binding.volumes else {
        unreachable!()
    };
    fake.volume_absent(&binding, &ids[0]);
    assert!(
        store
            .settle_budget(
                "request-a",
                &period(),
                &fake.evidence(&binding, known(0), 105),
                105
            )
            .is_err()
    );
    assert_eq!(reservation(&store).state, BudgetState::Uncertain);
    assert_eq!(reservation(&store).reserved_all_in, 1766);
    fake.volume_absent(&binding, &ids[1]);
    let absent = fake.evidence(&binding, FinalBilling::Unknown, 106);
    let row = store
        .settle_budget("request-a", &period(), &absent, 106)
        .unwrap();
    assert_eq!(row.state, BudgetState::BillingUnknown);
    assert_eq!(row.reserved_all_in, 1766);
    drop(store);
    drop(fake);
    let store = f.open();
    let fake = Fake::open(&f.fake);
    assert_eq!(reservation(&store), row);
    assert!(
        store
            .reserve_budget(&f.policy, &period(), &request("b"), 107)
            .is_err()
    );
    let final_bill = fake.evidence(&binding, known(900), 108);
    let settled = store
        .settle_budget("request-a", &period(), &final_bill, 108)
        .unwrap();
    assert_eq!(settled.state, BudgetState::Settled);
    assert_eq!(
        store
            .settle_budget("request-a", &period(), &final_bill, 108)
            .unwrap(),
        settled
    );
    let mut changed = final_bill.clone();
    changed.billing = known(0);
    assert!(
        store
            .settle_budget("request-a", &period(), &changed, 109)
            .is_err()
    );
    // Only 900, not the full1766 and not zero, remains debited. 900+1766<=3000.
    f.seed(&store, "b", 110);
    store.prepare_create(&intent("b"), 110).unwrap();
    let next = store.claim_create("operation-b", 111).unwrap();
    drop(next);
    assert!(
        operation(&store).cleanup_required,
        "no production cleanup completion hook is implied"
    );
    assert_eq!(
        store.nodes().unwrap().len(),
        2,
        "budget settlement does not delete instance rows"
    );
    assert_eq!(fake.state.calls.len(), 1);
    for id in ["foreign-instance", "same-owner-decoy"] {
        let decoy = &fake.state.resources[id];
        assert!(decoy.instance_present);
        assert!(decoy.volumes.values().all(|present| *present));
        assert!(
            !fake
                .state
                .cleanup_calls
                .iter()
                .any(|call| call.starts_with(id))
        );
    }
}

#[test]
fn wrong_or_changed_bindings_and_incomplete_settlement_leave_liability_unchanged() {
    let f = Fixture::new(policy());
    let store = f.prepared();
    let mut fake = Fake::open(&f.fake);
    let binding = fake
        .execute(
            store.claim_create("operation-a", 101).unwrap(),
            &f.db,
            102,
            Reply::Bound,
        )
        .unwrap();
    for field in ["owner", "run", "uid", "stale", "future", "partial"] {
        let mut wrong = binding.clone();
        match field {
            "owner" => wrong.owner = "foreign-owner".into(),
            "run" => wrong.run_id = "foreign-run".into(),
            "uid" => wrong.node_uid = "foreign-uid".into(),
            "stale" => wrong.observed_at = 0,
            "future" => wrong.observed_at = 105,
            _ => wrong.status = EvidenceStatus::Partial,
        }
        assert!(
            store
                .record_create_result("operation-a", &CreateObservation::Bound(wrong), 103)
                .is_err()
        );
        assert!(reservation(&store).binding.is_none());
    }
    store
        .record_create_result(
            "operation-a",
            &CreateObservation::Bound(binding.clone()),
            103,
        )
        .unwrap();
    let original = reservation(&store);
    for changed_volume in [false, true] {
        let mut wrong = binding.clone();
        if changed_volume {
            wrong.volumes = VolumeBinding::NoneAllocated;
        } else {
            wrong.instance_id = "replacement".into();
        }
        assert!(
            store
                .record_create_result("operation-a", &CreateObservation::Bound(wrong.clone()), 104)
                .is_err()
        );
        let evidence = SettlementEvidence {
            binding: wrong,
            observed_at: 104,
            status: EvidenceStatus::Complete,
            resource: ResourceOutcome::OwnedAbsent,
            volumes: VolumeOutcome::BoundVolumesAbsent,
            billing: known(0),
            provenance: "synthetic-mismatch".into(),
        };
        assert!(
            store
                .settle_budget("request-a", &period(), &evidence, 104)
                .is_err()
        );
    }
    for resource in [
        ResourceOutcome::DestroyAcknowledged,
        ResourceOutcome::EmptyList,
        ResourceOutcome::Stopped,
        ResourceOutcome::ProcessDead,
        ResourceOutcome::Unknown,
        ResourceOutcome::Partial,
        ResourceOutcome::Error,
    ] {
        let mut evidence = fake.evidence(&binding, known(0), 104);
        evidence.resource = resource;
        evidence.volumes = VolumeOutcome::BoundVolumesAbsent;
        assert!(
            store
                .settle_budget("request-a", &period(), &evidence, 104)
                .is_err()
        );
    }
    for volumes in [
        VolumeOutcome::DeleteAcknowledged,
        VolumeOutcome::EmptyList,
        VolumeOutcome::Unknown,
        VolumeOutcome::Partial,
        VolumeOutcome::Error,
    ] {
        let mut evidence = fake.evidence(&binding, known(0), 104);
        evidence.resource = ResourceOutcome::OwnedAbsent;
        evidence.volumes = volumes;
        assert!(
            store
                .settle_budget("request-a", &period(), &evidence, 104)
                .is_err()
        );
    }
    drop(store);
    let store = f.open();
    assert_eq!(reservation(&store), original);
    assert_eq!(operation(&store).result, Some(binding));
    assert_eq!(fake.state.calls.len(), 1);
}
