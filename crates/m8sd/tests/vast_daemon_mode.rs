//! Synthetic state only: no provider, network, real credential or paid resource.
//! The recovery daemon must fail closed unless a managed Store already exists.
#![cfg(all(unix, feature = "prototype-vast"))]
use m8s_api::{Request, call};
use m8s_provider::vast::readonly::{launch::LaunchRequest, recovery::RecoveryIntent};
use m8sd::{
    Store,
    budget::*,
    operation::{CreateState, vast::VastExecutor},
};
use std::{
    fs,
    num::NonZeroU64,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    daemon: Option<Child>,
}
impl Fixture {
    fn new() -> Self {
        // Darwin's per-user temp directory leaves too little sun_path space.
        let root = PathBuf::from("/tmp").canonicalize().unwrap().join(format!(
            "m8s-vast-daemon-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("state"))
            .unwrap();
        let credential = root.join("credential.env");
        fs::write(&credential, "VAST_API_KEY=synthetic-not-a-key\n").unwrap();
        fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
        Self { root, daemon: None }
    }
    fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_m8sd"));
        c.args(["--vast-daemon", "--state-dir"])
            .arg(self.state())
            .args(["--owner", "owner", "--credential-file"])
            .arg(self.root.join("credential.env"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        c
    }
    fn refuses(&self, mut command: Command) {
        let mut child = command.spawn().unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(!status.success());
                assert!(!self.state().join("control.sock").exists());
                return;
            }
            if Instant::now() >= until {
                let _ = child.kill();
                let _ = child.wait();
                panic!("startup did not refuse");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// A real prepared (never claimed) managed launch, built with public APIs.
    fn prepare_managed(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let period = CalendarPeriod {
            id: "synthetic-month".into(),
            starts_at: now - 100,
            ends_at: now - 100 + 28 * 86400,
        };
        let policy = AdminBudget {
            owner: "owner".into(),
            approval_id: "synthetic-only".into(),
            baseline: AccountBaseline {
                prior_lifetime_spent: 0,
                prior_current_period_spent: 0,
                initial_period: period.clone(),
                external_liability: ExternalLiability::NoneConfirmed,
                observed_at: now,
                status: EvidenceStatus::Complete,
                provenance: "synthetic".into(),
            },
            valid_from: now - 100,
            valid_until: now + 86400,
            trial_all_in: 6_000_000,
            monthly_all_in: 6_000_000,
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
                observed_at: now,
                valid_until: now + 2000,
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
        let image = format!("example.invalid/mesh@sha256:{}", "a".repeat(64));
        let identity = RecoveryIntent::new("owner", "node", "smoke", &image).unwrap();
        let key = format!(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI{}",
            "A".repeat(68 - "AAAAC3NzaC1lZDI1NTE5AAAAI".len())
        );
        let executor = VastExecutor::new(
            LaunchRequest::new(NonZeroU64::new(42).unwrap(), identity, key, 10).unwrap(),
            "owner",
            "node",
            "smoke",
            NonZeroU64::new(7).unwrap(),
        )
        .unwrap();
        let store = Store::open(self.state().join("state.db"), "owner").unwrap();
        store.scale("smoke", 1).unwrap();
        executor
            .prepare_launch(&store, &policy, &period, &request, "operation")
            .unwrap();
    }
    fn start(&mut self) {
        self.daemon = Some(self.command().spawn().unwrap());
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if call(&self.state().join("control.sock"), &Request::Status {}).is_ok() {
                return;
            }
            assert!(self.daemon.as_mut().unwrap().try_wait().unwrap().is_none());
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn stop(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn refuses_empty_unmanaged_or_unselected_state_without_creating_anything() {
    let f = Fixture::new();
    // Fresh Store, no managed history: this mode must not create a domain.
    f.refuses(f.command());
    drop(Store::open(f.state().join("state.db"), "owner").unwrap());
    f.refuses(f.command());
    // Legacy mock history is not interpreted as managed either.
    let store = Store::open(f.state().join("state.db"), "owner").unwrap();
    store.scale("smoke", 1).unwrap();
    drop(store);
    f.refuses(f.command());
}

#[test]
fn refuses_unsafe_or_incomplete_arguments_and_credentials() {
    let mut f = Fixture::new();
    f.prepare_managed();
    let cred = f.root.join("credential.env");
    fs::set_permissions(&cred, fs::Permissions::from_mode(0o644)).unwrap();
    f.refuses(f.command());
    fs::set_permissions(&cred, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&cred, "OTHER=1\n").unwrap();
    f.refuses(f.command());
    fs::write(&cred, "VAST_API_KEY=synthetic-not-a-key\n").unwrap();
    for args in [
        vec!["--vast-daemon"],
        vec![
            "--vast-daemon",
            "--state-dir",
            "relative",
            "--owner",
            "owner",
            "--credential-file",
            "/nonexistent",
        ],
    ] {
        let mut c = Command::new(env!("CARGO_BIN_EXE_m8sd"));
        c.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        f.refuses(c);
    }
    let mut extra = f.command();
    extra.arg("--surprise");
    f.refuses(extra);
    let missing = f.root.join("never-created");
    let mut c = Command::new(env!("CARGO_BIN_EXE_m8sd"));
    c.args(["--vast-daemon", "--state-dir"])
        .arg(&missing)
        .args(["--owner", "owner", "--credential-file"])
        .arg(&cred)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    f.refuses(c);
    assert!(!missing.exists());
    // The default mock daemon still refuses managed state.
    let mut mock = Command::new(env!("CARGO_BIN_EXE_m8sd"));
    mock.arg(f.state())
        .arg("owner")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    f.refuses(mock);
    f.start();
}

#[test]
fn serves_status_refuses_launch_and_never_consumes_the_claim() {
    let mut f = Fixture::new();
    f.prepare_managed();
    f.start();
    let socket = f.state().join("control.sock");
    let status = call(&socket, &Request::Status {}).unwrap();
    assert!(status.ok);
    assert_eq!(status.mode, m8s_api::VAST_DEMO_MODE);
    assert_eq!(status.secure_bootstrap, m8s_api::VAST_DEMO_SECURITY);
    assert_eq!(status.nodes.len(), 1);
    for request in [
        Request::Up {
            tier: "smoke".into(),
        },
        Request::Scale {
            tier: "smoke".into(),
            replicas: 2,
        },
        Request::Shutdown {},
    ] {
        assert!(!call(&socket, &request).unwrap().ok);
    }
    let down = call(&socket, &Request::Down {}).unwrap();
    assert!(down.ok);
    assert_eq!(down.tiers, [("smoke".into(), 0)]);
    f.stop();
    // Restarting the daemon reconciled nothing it was not entitled to: the
    // prepared operation is still unclaimed, and no row was removed.
    let store = Store::open(f.state().join("state.db"), "owner").unwrap();
    let operations = store.create_operations().unwrap();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].state, CreateState::Prepared);
    assert_eq!(store.nodes().unwrap().len(), 1);
    assert!(Path::new(&f.state().join("daemon.lock")).exists());
}
