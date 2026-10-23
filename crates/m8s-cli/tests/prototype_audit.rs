#![cfg(all(unix, feature = "prototype-vast"))]
//! Real CLI over synthetic local SQLite only. No credentials/provider/native IO.
use m8sd::{Store, budget::*};
use serde_json::Value;
use std::os::unix::fs::DirBuilderExt;
use std::{
    path::Path,
    process::{Command, Output},
};

fn private_directory() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    (root, path)
}

fn audit(directory: &Path, owner: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_m8s"))
        .env_clear()
        .args(["prototype", "audit", "--directory"])
        .arg(directory)
        .args(["--owner", owner])
        .output()
        .unwrap()
}

#[test]
fn empty_journal_is_a_stored_snapshot_not_provider_absence() {
    let (_root, directory) = private_directory();
    let store = Store::open(directory.as_path().join("state.db"), "owner").unwrap();
    let output = audit(directory.as_path(), "owner");
    assert!(output.status.success(), "audit refused synthetic fixture");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "m8s.prototype-attempt-audit/v1");
    assert_eq!(value["scope"], "stored-journal-snapshot");
    assert_eq!(value["provider_calls"], false);
    for collection in ["operations", "budgets", "removals", "attempts"] {
        assert!(value[collection].as_array().unwrap().is_empty());
    }
    assert!(value.get("provider_confirmed_absent").is_none());
    store.require_unmanaged_state().unwrap(); // Audit did not claim an execution domain.
    let wrong = audit(directory.as_path(), "other");
    assert!(!wrong.status.success());
    assert!(wrong.stdout.is_empty());
    let missing = tempfile::tempdir().unwrap();
    assert!(!audit(missing.path(), "owner").status.success());
    assert!(!missing.path().join("state.db").exists());
}

#[test]
fn reservation_only_audit_retains_cost_and_cannot_claim_or_dispatch() {
    let (_root, directory) = private_directory();
    let store = Store::open(directory.as_path().join("state.db"), "owner").unwrap();
    let period = CalendarPeriod {
        id: "synthetic-month".into(),
        starts_at: 0,
        ends_at: 28 * 86400,
    };
    let policy = AdminBudget {
        owner: "owner".into(),
        approval_id: "synthetic-only".into(),
        baseline: AccountBaseline {
            prior_lifetime_spent: 0,
            prior_current_period_spent: 0,
            initial_period: period.clone(),
            external_liability: ExternalLiability::NoneConfirmed,
            observed_at: 100,
            status: EvidenceStatus::Complete,
            provenance: "synthetic".into(),
        },
        valid_from: 0,
        valid_until: 86400,
        trial_all_in: 6000000,
        monthly_all_in: 6000000,
        total_hourly: 30000,
        instance_hourly: 30000,
        max_instances: 1,
        max_runtime_seconds: 600,
        minimum_cleanup_seconds: 240,
        minimum_contingency: 100,
    };
    let request = BudgetRequest {
        owner: "owner".into(),
        run_id: "run".into(),
        request_id: "request".into(),
        node_uid: "node".into(),
        quote: CostQuote {
            provenance: "synthetic".into(),
            observed_at: 100,
            valid_until: 2000,
            status: EvidenceStatus::Complete,
            compute_unit: RateUnit::MicroUsdPerHour,
            storage_unit: RateUnit::MicroUsdPerHour,
            totals_unit: TotalUnit::MicroUsd,
            compute_hourly: 3600,
            storage_hourly: 0,
            transfer_total: 0,
            runtime_seconds: 60,
            cleanup_seconds: 240,
            contingency: 100,
        },
    };
    let before = store
        .reserve_budget(&policy, &period, &request, 100)
        .unwrap();
    let output = audit(directory.as_path(), "owner");
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let attempt = &value["attempts"][0];
    assert_eq!(attempt["stage"], "reservation_only");
    assert_eq!(attempt["billing_state"], "Reserved");
    assert_eq!(attempt["reserved_micro_usd"], before.reserved_all_in);
    assert_eq!(attempt["metadata"]["status"], "no_operation");
    assert!(attempt["operation_id"].is_null());
    assert_eq!(store.budget_reservations().unwrap(), vec![before]);
    assert!(store.create_operations().unwrap().is_empty());
    assert!(store.nodes().unwrap().is_empty());
}
