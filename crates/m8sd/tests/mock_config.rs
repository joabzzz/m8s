#![cfg(unix)]
//! Real hardwired Mock daemon and public bounded Unix control, own state only.
use m8s_api::{Request, Response};
use std::{
    ffi::OsString,
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::PathBuf,
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
const OWNER: &str = "synthetic-configured-mock-owner";
const CATALOG: &str = include_str!("../../m8s-core/models/examples.toml");
const CONFIG: &str = "schema=1\n[tier.bounded]\nmodel='example-27b-q4km'\nreplicas={min=2,max=3}\n[tier.optional]\nmodel='example-27b-q4km'\nreplicas={min=0,max=1}\n[tier.disabled]\nmodel='example-27b-q4km'\nreplicas={min=0,max=0}\n";
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        // Keep BSD sockaddr_un below its short limit. Only this new owned root.
        let root = fs::canonicalize("/tmp").unwrap().join(format!(
            "m8mc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::write(root.join("config.toml"), CONFIG).unwrap();
        fs::write(root.join("catalog.toml"), CATALOG).unwrap();
        Self { root }
    }
    fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    fn args(&self, configured: bool) -> Vec<OsString> {
        let mut args = vec![self.state().into(), OWNER.into()];
        if configured {
            args.extend([
                "--config".into(),
                self.root.join("config.toml").into(),
                "--catalog".into(),
                self.root.join("catalog.toml").into(),
            ]);
        }
        args
    }
    fn start(&self, configured: bool) -> Daemon {
        let mut daemon = Daemon::spawn(&self.args(configured), false);
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            assert!(
                daemon.child().try_wait().unwrap().is_none(),
                "daemon exited during startup"
            );
            if let Ok(response) =
                m8s_api::call(&self.state().join("control.sock"), &Request::Status {})
            {
                assert!(response.ok);
                break;
            }
            assert!(Instant::now() < deadline, "mock startup deadline exceeded");
            std::thread::sleep(Duration::from_millis(10));
        }
        daemon
    }
    fn call(&self, request: Request) -> Response {
        // Public API applies three-second read/write timeouts, fixed local peer.
        let response = m8s_api::call(&self.state().join("control.sock"), &request).unwrap();
        assert_eq!(response.mode, "mock-only");
        assert_eq!(
            response.secure_bootstrap,
            "unavailable; nodes cannot become Ready"
        );
        response
    }
    fn store_snapshot(&self) -> (Vec<(String, u32)>, Vec<u8>, Vec<String>) {
        let store = m8sd::Store::open(self.state().join("state.db"), OWNER).unwrap();
        (
            store.tiers().unwrap(),
            serde_json::to_vec(&store.nodes().unwrap()).unwrap(),
            store.logs().unwrap(),
        )
    }
    fn inventory(&self) -> Vec<(OsString, u64, Vec<u8>)> {
        let mut files = Vec::new();
        for entry in fs::read_dir(self.state()).unwrap() {
            let e = entry.unwrap();
            let m = e.metadata().unwrap();
            files.push((
                e.file_name(),
                m.ino(),
                if m.is_file() {
                    fs::read(e.path()).unwrap()
                } else {
                    Vec::new()
                },
            ));
        }
        files.sort();
        files
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
struct Daemon(Option<Child>);
impl Daemon {
    fn spawn(args: &[OsString], capture: bool) -> Self {
        Self(Some(
            Command::new(env!("CARGO_BIN_EXE_m8sd"))
                .env_clear()
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(if capture {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .spawn()
                .unwrap(),
        ))
    }
    fn child(&mut self) -> &mut Child {
        self.0.as_mut().unwrap()
    }
    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            if let Some(status) = self.child().try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "owned daemon exit deadline exceeded"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn stop(mut self) {
        self.child().kill().unwrap();
        self.wait();
    }
    fn refusal(mut self) -> Output {
        assert!(!self.wait().success());
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn up(tier: &str) -> Request {
    Request::Up { tier: tier.into() }
}
fn scale(tier: &str, replicas: u32) -> Request {
    Request::Scale {
        tier: tier.into(),
        replicas,
    }
}
#[test]
fn configured_commands_admit_bounds_but_never_auto_start_or_block_stop() {
    let f = Fixture::new();
    let d = f.start(true);
    assert!(f.call(Request::Status {}).tiers.is_empty());
    for req in [
        up("unknown"),
        scale("unknown", 0),
        scale("bounded", 1),
        scale("bounded", 4),
        up("disabled"),
    ] {
        let response = f.call(req);
        assert!(!response.ok);
        assert!(response.tiers.is_empty());
        assert_eq!(response.error.as_deref(), Some("control request refused"));
    }
    assert_eq!(f.call(up("bounded")).tiers, vec![("bounded".into(), 2)]);
    assert_eq!(
        f.call(scale("bounded", 3)).tiers,
        vec![("bounded".into(), 3)]
    );
    assert_eq!(
        f.call(scale("bounded", 0)).tiers,
        vec![("bounded".into(), 0)]
    );
    assert_eq!(
        f.call(up("optional")).tiers,
        vec![("bounded".into(), 0), ("optional".into(), 1)]
    );
    assert!(f.call(up("bounded")).ok);
    assert!(f.call(Request::Down {}).tiers.iter().all(|(_, n)| *n == 0));
    assert!(f.call(scale("disabled", 0)).ok);
    d.stop();
    let d = f.start(true);
    assert!(
        f.call(Request::Status {})
            .tiers
            .iter()
            .all(|(_, n)| *n == 0)
    );
    d.stop();
}
#[test]
fn persisted_positive_desired_survives_restart_and_incompatible_reopen_is_retained() {
    let f = Fixture::new();
    let d = f.start(true);
    assert!(f.call(scale("bounded", 3)).ok);
    let deadline = Instant::now() + Duration::from_secs(6);
    while f.call(Request::Status {}).nodes.is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    // Files are not live-reloaded: old max3 still governs this running snapshot.
    fs::write(f.root.join("config.toml"), CONFIG.replace("max=3", "max=2")).unwrap();
    assert!(f.call(scale("bounded", 3)).ok);
    fs::write(f.root.join("config.toml"), CONFIG).unwrap();
    d.stop();
    let before = f.store_snapshot();
    assert_ne!(before.1, b"[]");
    assert!(!before.2.is_empty());
    let d = f.start(true);
    assert_eq!(
        f.call(Request::Status {}).tiers,
        vec![("bounded".into(), 3)]
    );
    d.stop();
    let before = f.store_snapshot();
    let files = f.inventory();
    for config in [
        CONFIG.replace("max=3", "max=2"),
        CONFIG.replace("tier.bounded", "tier.renamed"),
    ] {
        fs::write(f.root.join("config.toml"), config).unwrap();
        let out = Daemon::spawn(&f.args(true), true).refusal();
        assert_eq!(
            out.stderr,
            b"m8sd: persisted mock desired state incompatible with configuration; state retained\n"
        );
        assert_eq!(f.inventory(), files);
        assert_eq!(f.store_snapshot(), before);
    }
}
#[test]
fn legacy_unknown_tier_cannot_be_silently_adopted_even_at_zero() {
    let f = Fixture::new();
    let d = f.start(false);
    assert!(f.call(up("legacy-arbitrary")).ok);
    assert!(f.call(Request::Down {}).ok);
    d.stop();
    let before = f.store_snapshot();
    let files = f.inventory();
    let out = Daemon::spawn(&f.args(true), true).refusal();
    assert_eq!(
        out.stderr,
        b"m8sd: persisted mock desired state incompatible with configuration; state retained\n"
    );
    assert_eq!(f.inventory(), files);
    assert_eq!(f.store_snapshot(), before);
}
#[test]
fn invalid_arguments_or_full_configuration_refuse_before_creating_state() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let base = f.args(true);
    let mut cases = Vec::new();
    for remove in [2..4, 4..6] {
        let mut args = base.clone();
        args.drain(remove);
        cases.push(args);
    }
    for extra in [
        vec!["--config", "/sensitive-duplicate"],
        vec!["--unknown"],
        vec!["extra"],
    ] {
        let mut args = base.clone();
        args.extend(extra.into_iter().map(OsString::from));
        cases.push(args);
    }
    let mut args = base.clone();
    args[3] = "relative-sensitive-path".into();
    cases.push(args);
    let mut args = base.clone();
    args.push(OsString::from_vec(vec![255]));
    cases.push(args);
    for args in cases {
        let out = Daemon::spawn(&args, true).refusal();
        assert!(!String::from_utf8_lossy(&out.stderr).contains("sensitive-"));
        assert!(!f.state().exists());
    }
    for config in [
        "schema=1\n".to_owned(),
        CONFIG.replace("tier.bounded", "tier.'with.dot'"),
        CONFIG.replace("max=3", "max=17"),
        CONFIG.replace("example-27b-q4km", "sensitive-unknown-model"),
    ] {
        fs::write(f.root.join("config.toml"), &config).unwrap();
        let out = Daemon::spawn(&base, true).refusal();
        if config.contains("'with.dot'") {
            assert_eq!(
                out.stderr,
                b"m8sd: mock configuration has unsupported tier bounds or identifiers\n"
            );
        }
        assert!(!String::from_utf8_lossy(&out.stderr).contains("sensitive-"));
        assert!(!f.state().exists());
        assert_eq!(
            fs::read_to_string(f.root.join("config.toml")).unwrap(),
            config
        );
    }
    fs::write(f.root.join("config.toml"), CONFIG).unwrap();
    let (prefix, suffix) = CATALOG
        .rsplit_once("runtime_status = \"untested\"")
        .unwrap();
    fs::write(
        f.root.join("catalog.toml"),
        format!("{prefix}runtime_status = \"sensitive-invalid\"{suffix}"),
    )
    .unwrap();
    let out = Daemon::spawn(&base, true).refusal();
    assert_eq!(
        out.stderr,
        b"m8sd: mock configuration unavailable or invalid\n"
    );
    assert!(!f.state().exists());
}

#[test]
fn standalone_help_is_fixed_effect_free_and_rejects_combinations() {
    let f = Fixture::new();
    let inventory = || {
        let mut entries: Vec<_> = fs::read_dir(&f.root)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let metadata = entry.metadata().unwrap();
                (
                    entry.file_name(),
                    metadata.ino(),
                    metadata.mode(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        entries.sort();
        entries
    };
    let before = inventory();
    let help = || {
        Command::new(env!("CARGO_BIN_EXE_m8sd"))
            .env_clear()
            .arg("--help")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let out = help();
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert_eq!(out.stdout, help().stdout);
    let text = String::from_utf8(out.stdout).unwrap();
    for required in [
        "m8sd <private-state-dir>",
        "--config ABS --catalog ABS",
        "max(1,min)",
        "scale0",
        "emergency down",
        "disabled",
        "Restart retains desired",
        "hardwired Mock",
        "no Ready",
        "no automatic",
    ] {
        assert!(
            text.to_lowercase().contains(&required.to_lowercase()),
            "missing help boundary"
        );
    }
    assert!(!text.contains(f.root.to_str().unwrap()));
    assert!(!text.contains("example://"));
    for args in [
        vec!["--help".into(), f.state().into()],
        vec![f.state().into(), "--help".into()],
        {
            let mut args = f.args(true);
            args.push("--help".into());
            args
        },
    ] {
        let out = Daemon::spawn(&args, true).refusal();
        assert_eq!(out.stderr,b"m8sd: invalid daemon arguments; expected state-dir [owner] with paired --config ABS --catalog ABS\n");
        assert!(!f.state().exists());
        assert_eq!(inventory(), before);
    }
    assert!(!f.state().exists());
    assert_eq!(inventory(), before);
}

#[test]
fn help_broken_stdout_is_nonzero_with_static_error() {
    use std::os::{fd::OwnedFd, unix::net::UnixStream};
    // Peer is closed BEFORE spawn: deterministic EPIPE, no timing/signal race.
    let (reader, writer) = UnixStream::pair().unwrap();
    drop(reader);
    let fd: OwnedFd = writer.into();
    let out = Command::new(env!("CARGO_BIN_EXE_m8sd"))
        .env_clear()
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::from(fd))
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(out.stderr, b"m8sd: daemon-help-output-unavailable\n");
}

#[test]
fn empty_shutdown_exits_normally_but_down_does_not() {
    let f = Fixture::new();
    let mut d = f.start(false);
    assert!(f.call(Request::Down {}).ok);
    assert!(d.child().try_wait().unwrap().is_none());
    assert!(f.call(Request::Status {}).ok);
    let response = f.call(Request::Shutdown {});
    assert!(response.ok);
    assert!(response.nodes.is_empty());
    assert!(response.tiers.iter().all(|(_, n)| *n == 0));
    assert!(d.wait().success());
    assert!(f.state().join("control.sock").exists());
}

fn wait_for_mock_nodes(f: &Fixture) {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        if !f.call(Request::Status {}).nodes.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "mock creation deadline exceeded");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn assert_stopped_and_restart_zero(f: &Fixture) {
    use m8s_core::provider::Provider;
    let mut provider =
        m8sd::mock_provider::Mock::persistent(f.state().join("mock-provider.json")).unwrap();
    assert!(provider.list_owned(OWNER).unwrap().is_empty());
    let (tiers, nodes, events) = f.store_snapshot();
    assert_eq!(tiers, vec![("bounded".into(), 0)]);
    assert_eq!(nodes, b"[]");
    assert!(
        events
            .iter()
            .any(|e| e.contains("provider confirmed absence"))
    );
    let mut restarted = f.start(true);
    let response = f.call(Request::Status {});
    assert_eq!(response.tiers, tiers);
    assert!(response.nodes.is_empty());
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(restarted.wait().success());
}

#[test]
fn active_shutdown_confirms_mock_absence_and_restart_retains_zero() {
    let f = Fixture::new();
    let mut d = f.start(true);
    assert!(f.call(up("bounded")).ok);
    wait_for_mock_nodes(&f);
    let response = f.call(Request::Shutdown {});
    assert!(response.ok);
    assert!(response.nodes.is_empty());
    assert_eq!(response.tiers, vec![("bounded".into(), 0)]);
    assert!(d.wait().success());
    assert_stopped_and_restart_zero(&f);
}

#[test]
fn disconnected_response_reader_cannot_undo_committed_shutdown() {
    use std::{io::Write, net::Shutdown, os::unix::net::UnixStream};
    let f = Fixture::new();
    let mut d = f.start(true);
    assert!(f.call(up("bounded")).ok);
    wait_for_mock_nodes(&f);
    let mut stream = UnixStream::connect(f.state().join("control.sock")).unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    // Close the response side, while retaining the request connection until
    // normal exit. Unlike a pre-accept full close, request delivery is explicit.
    stream.shutdown(Shutdown::Read).unwrap();
    serde_json::to_writer(&mut stream, &Request::Shutdown {}).unwrap();
    stream.write_all(b"\n").unwrap();
    assert!(d.wait().success());
    drop(stream);
    assert_stopped_and_restart_zero(&f);
}

#[test]
fn legacy_reload_refuses_without_adopting_fixture_configuration() {
    let f = Fixture::new();
    let mut d = f.start(false);
    // Files exist but were NOT selected at startup; no implicit discovery.
    fs::write(f.root.join("config.toml"), "sensitive-invalid-document").unwrap();
    let response = f.call(Request::ReloadConfig {});
    assert!(!response.ok);
    assert_eq!(response.error.as_deref(), Some("control request refused"));
    assert!(response.tiers.is_empty());
    assert!(d.child().try_wait().unwrap().is_none());
    assert!(f.call(up("legacy-unconfigured")).ok);
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(d.wait().success());
}

#[test]
fn explicit_reload_publishes_only_compatible_policy_and_restart_uses_files() {
    let f = Fixture::new();
    let mut d = f.start(true);
    assert!(f.call(up("bounded")).ok); // desired2 under min2/max3
    fs::write(f.root.join("config.toml"), CONFIG.replace("max=3", "max=2")).unwrap();
    // Editing alone has no effect; old max3 still admits3.
    assert!(f.call(scale("bounded", 3)).ok);
    let refused = f.call(Request::ReloadConfig {}); // desired3 incompatible
    assert!(!refused.ok);
    assert_eq!(refused.tiers, vec![("bounded".into(), 3)]);
    assert!(f.call(scale("bounded", 3)).ok); // old snapshot remains active
    assert!(f.call(Request::Down {}).ok);
    let accepted = f.call(Request::ReloadConfig {});
    assert!(accepted.ok);
    assert_eq!(accepted.tiers, vec![("bounded".into(), 0)]);
    assert!(!f.call(scale("bounded", 3)).ok); // NEW max2 now enforced
    assert!(f.call(up("bounded")).ok);
    let raised = CONFIG.replace("min=2,max=3", "min=3,max=4");
    fs::write(f.root.join("config.toml"), &raised).unwrap();
    assert!(!f.call(scale("bounded", 4)).ok); // file edits do not raise max
    assert!(!f.call(Request::ReloadConfig {}).ok); // desired2 below new min3
    assert!(f.call(scale("bounded", 2)).ok); // previous policy retained
    assert!(f.call(Request::Down {}).ok);
    assert!(f.call(Request::ReloadConfig {}).ok);
    assert_eq!(f.call(up("bounded")).tiers, vec![("bounded".into(), 3)]);
    assert!(!f.call(scale("bounded", 2)).ok);
    assert!(f.call(scale("bounded", 4)).ok);
    assert!(f.call(scale("bounded", 0)).ok); // explicit stop even min3
    assert!(!f.call(up("disabled")).ok);
    assert!(f.call(scale("disabled", 0)).ok);
    assert!(!f.call(scale("unknown", 0)).ok);
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(d.wait().success());
    let mut restarted = f.start(true);
    assert!(
        f.call(Request::Status {})
            .tiers
            .iter()
            .all(|(_, n)| *n == 0)
    );
    assert_eq!(
        f.call(up("bounded")).tiers,
        vec![("bounded".into(), 3), ("disabled".into(), 0)]
    );
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(restarted.wait().success());
    assert_eq!(
        fs::read_to_string(f.root.join("config.toml")).unwrap(),
        raised
    );
}

#[test]
fn failed_full_reload_preserves_old_admission_and_durable_desired() {
    let f = Fixture::new();
    let mut d = f.start(true);
    assert!(f.call(up("bounded")).ok);
    // Retain a zero tier too: it cannot disappear via reload or adoption.
    assert!(f.call(scale("optional", 0)).ok);
    let expected = vec![("bounded".into(), 2), ("optional".into(), 0)];
    let (prefix, suffix) = CATALOG
        .rsplit_once("runtime_status = \"untested\"")
        .unwrap();
    let invalid_catalog = format!("{prefix}runtime_status = \"sensitive-invalid\"{suffix}");
    let cases = [
        ("sensitive-invalid-TOML".to_owned(), CATALOG.to_owned()),
        (CONFIG.replace("max=3", "max=17"), CATALOG.to_owned()),
        (
            CONFIG.replace("tier.bounded", "tier.'with.dot'"),
            CATALOG.to_owned(),
        ),
        (
            CONFIG.replace("min=2,max=3", "min=3,max=2"),
            CATALOG.to_owned(),
        ),
        (
            CONFIG.replace("tier.optional", "tier.renamed"),
            CATALOG.to_owned(),
        ),
        (CONFIG.to_owned(), invalid_catalog), // UNUSED model still validated
    ];
    for (config, catalog) in cases {
        fs::write(f.root.join("config.toml"), &config).unwrap();
        fs::write(f.root.join("catalog.toml"), &catalog).unwrap();
        let response = f.call(Request::ReloadConfig {});
        assert!(!response.ok);
        assert_eq!(response.error.as_deref(), Some("control request refused"));
        assert_eq!(response.tiers, expected);
        assert_eq!(f.call(Request::Status {}).tiers, expected);
        assert!(d.child().try_wait().unwrap().is_none());
        assert!(f.call(scale("bounded", 3)).ok); // old max3 still works
        assert!(f.call(scale("bounded", 2)).ok);
        assert_eq!(
            fs::read_to_string(f.root.join("config.toml")).unwrap(),
            config
        );
        assert_eq!(
            fs::read_to_string(f.root.join("catalog.toml")).unwrap(),
            catalog
        );
    }
    fs::write(f.root.join("config.toml"), CONFIG).unwrap();
    fs::write(f.root.join("catalog.toml"), CATALOG).unwrap();
    fs::remove_file(f.root.join("catalog.toml")).unwrap();
    assert!(!f.call(Request::ReloadConfig {}).ok);
    assert_eq!(f.call(Request::Status {}).tiers, expected);
    assert!(f.call(scale("bounded", 3)).ok);
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(d.wait().success());
}

#[test]
fn config_status_is_actual_active_policy_not_files_or_desired() {
    let f = Fixture::new();
    let mut d = f.start(true);
    let status = || {
        let response = m8s_api::call_config_status(&f.state().join("control.sock")).unwrap();
        assert!(response.ok && response.configured);
        assert_eq!(response.mode, "mock-only");
        assert_eq!(response.scope, "mock-control-admission-only");
        assert_eq!(
            response
                .tiers
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["bounded", "disabled", "optional"]
        );
        let wire = serde_json::to_string(&response).unwrap();
        for marker in [
            "config.toml",
            "catalog.toml",
            "example-27b",
            OWNER,
            "source",
            "schedule",
            "fingerprint",
        ] {
            assert!(!wire.contains(marker));
        }
        response.tiers
    };
    let initial = status();
    assert_eq!((initial[0].min, initial[0].max), (2, 3));
    assert_eq!((initial[1].min, initial[1].max), (0, 0));
    assert!(f.call(Request::Status {}).tiers.is_empty()); // no invented desired
    assert!(f.call(up("bounded")).ok);
    let desired = f.call(Request::Status {}).tiers;
    fs::write(f.root.join("config.toml"), CONFIG.replace("max=3", "max=4")).unwrap();
    assert_eq!(status(), initial); // no implicit reload
    assert!(f.call(Request::ReloadConfig {}).ok);
    let changed = status();
    assert_eq!((changed[0].min, changed[0].max), (2, 4));
    fs::write(f.root.join("config.toml"), "sensitive-invalid-config").unwrap();
    assert!(!f.call(Request::ReloadConfig {}).ok);
    fs::remove_file(f.root.join("config.toml")).unwrap();
    fs::remove_file(f.root.join("catalog.toml")).unwrap();
    assert_eq!(status(), changed); // status cannot require/reread either file
    assert_eq!(f.call(Request::Status {}).tiers, desired);
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(d.wait().success());

    let f = Fixture::new();
    let mut d = f.start(false);
    assert!(f.call(up("legacy-unconfigured")).ok);
    fs::remove_file(f.root.join("config.toml")).unwrap();
    fs::remove_file(f.root.join("catalog.toml")).unwrap();
    let legacy = m8s_api::call_config_status(&f.state().join("control.sock")).unwrap();
    assert!(legacy.ok && !legacy.configured);
    assert!(legacy.tiers.is_empty());
    assert_eq!(
        f.call(Request::Status {}).tiers,
        vec![("legacy-unconfigured".into(), 1)]
    );
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(d.wait().success());
}

#[test]
fn check_only_candidate_never_publishes_and_refusals_retain_policy() {
    let f = Fixture::new();
    let mut d = f.start(true);
    let check = || m8s_api::call_reload_check(&f.state().join("control.sock")).unwrap();
    let initial = check();
    assert!(initial.ok);
    assert_eq!(initial.bounds_changed, Some(false));
    assert!(f.call(up("bounded")).ok);
    fs::write(f.root.join("config.toml"), CONFIG.replace("max=3", "max=4")).unwrap();
    let checked = check();
    assert!(
        checked.ok && checked.check_only && !checked.applied && !checked.later_reload_guaranteed
    );
    assert_eq!(checked.bounds_changed, Some(true));
    assert_eq!(
        checked
            .active
            .iter()
            .find(|t| t.id == "bounded")
            .unwrap()
            .max,
        3
    );
    assert_eq!(
        checked
            .candidate
            .iter()
            .find(|t| t.id == "bounded")
            .unwrap()
            .max,
        4
    );
    assert!(!f.call(scale("bounded", 4)).ok);
    assert_eq!(
        f.call(Request::Status {}).tiers,
        vec![("bounded".into(), 2)]
    );
    assert!(f.call(Request::ReloadConfig {}).ok);
    assert!(f.call(scale("bounded", 4)).ok);
    assert_eq!(check().bounds_changed, Some(false));
    for bad in [
        "sensitive-invalid".to_string(),
        CONFIG.replace("max=3", "max=2"),
        CONFIG.replace("tier.optional", "tier.unsupported.id"),
    ] {
        fs::write(f.root.join("config.toml"), bad).unwrap();
        let r = check();
        assert!(!r.ok);
        assert!(r.active.is_empty() && r.candidate.is_empty());
        assert_eq!(r.error.as_deref(), Some("reload-check-refused"));
        assert_eq!(
            f.call(Request::Status {}).tiers,
            vec![("bounded".into(), 4)]
        );
        assert!(f.call(scale("bounded", 4)).ok); // old snapshot, desired4 retained
    }
    // Retained zero membership is also checked, never adopted/deleted.
    assert!(f.call(scale("optional", 0)).ok);
    let removed = CONFIG
        .replace(
            "[tier.optional]\nmodel='example-27b-q4km'\nreplicas={min=0,max=1}\n",
            "",
        )
        .replace("max=3", "max=4");
    fs::write(f.root.join("config.toml"), removed).unwrap();
    assert!(!check().ok);
    fs::write(f.root.join("config.toml"), CONFIG.replace("max=3", "max=4")).unwrap();
    fs::write(
        f.root.join("catalog.toml"),
        CATALOG.replace(
            "runtime_status = \"untested\"",
            "runtime_status = \"sensitive\"",
        ),
    )
    .unwrap();
    assert!(!check().ok);
    assert!(f.call(scale("bounded", 4)).ok);
    fs::write(f.root.join("catalog.toml"), CATALOG).unwrap();
    fs::remove_file(f.root.join("config.toml")).unwrap();
    assert!(!check().ok);
    assert!(f.call(scale("bounded", 4)).ok);
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(d.wait().success());
}

#[test]
fn legacy_check_refuses_without_paths_or_adoption() {
    let f = Fixture::new();
    let mut d = f.start(false);
    fs::remove_file(f.root.join("config.toml")).unwrap();
    fs::remove_file(f.root.join("catalog.toml")).unwrap();
    let r = m8s_api::call_reload_check(&f.state().join("control.sock")).unwrap();
    assert!(!r.ok && r.active.is_empty() && r.candidate.is_empty());
    assert!(f.call(up("legacy")).ok);
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(d.wait().success());
}

#[test]
fn configured_and_legacy_batch_admission_is_all_or_nothing() {
    let f = Fixture::new();
    let mut d = f.start(true);
    let batch = |pairs: &[(&str, u32)]| Request::ScaleBatch {
        targets: pairs
            .iter()
            .map(|(id, n)| m8s_api::ScaleTarget {
                tier: (*id).into(),
                replicas: *n,
            })
            .collect(),
    };
    assert!(!f.call(up("disabled")).ok);
    assert!(
        f.call(batch(&[("bounded", 2), ("optional", 1), ("disabled", 0)]))
            .ok
    );
    let expected = vec![
        ("bounded".into(), 2),
        ("disabled".into(), 0),
        ("optional".into(), 1),
    ];
    assert_eq!(f.call(Request::Status {}).tiers, expected);
    for pairs in [
        vec![("bounded", 3), ("unknown", 0)],
        vec![("bounded", 3), ("optional", 2)],
        vec![("optional", 0), ("bounded", 1)],
    ] {
        assert!(!f.call(batch(&pairs)).ok);
        assert_eq!(f.call(Request::Status {}).tiers, expected);
    }
    assert!(
        m8s_api::call(
            &f.state().join("control.sock"),
            &batch(&[("bounded", 0), ("bounded", 2)])
        )
        .is_err()
    );
    assert_eq!(f.call(Request::Status {}).tiers, expected);
    assert!(f.call(batch(&[("bounded", 0), ("optional", 0)])).ok);
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(d.wait().success());
    let mut restarted = f.start(true);
    assert!(
        f.call(Request::Status {})
            .tiers
            .iter()
            .all(|(_, n)| *n == 0)
    );
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(restarted.wait().success());
    let mut legacy = f.start(false);
    assert!(f.call(batch(&[("legacy_a", 2), ("legacy_b", 0)])).ok);
    let before = f.call(Request::Status {}).tiers;
    assert!(
        m8s_api::call(
            &f.state().join("control.sock"),
            &batch(&[("legacy_a", 1), ("bad.id", 0)])
        )
        .is_err()
    );
    assert_eq!(f.call(Request::Status {}).tiers, before);
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(legacy.wait().success());
}

#[test]
fn tier_explanation_is_stored_snapshot_with_active_policy_context_only() {
    let f = Fixture::new();
    let mut d = f.start(true);
    let socket = f.state().join("control.sock");
    let before = m8s_api::call_explain_tier(&socket, "bounded").unwrap();
    assert!(before.ok && before.configured);
    assert_eq!(before.configured_tier.unwrap().min, 2);
    assert_eq!(before.data.unwrap().desired, None);
    let absent = m8s_api::call_explain_tier(&socket, "unknown").unwrap();
    assert!(absent.configured && absent.configured_tier.is_none());
    assert_eq!(absent.data.unwrap().desired, None);
    assert!(f.call(up("bounded")).ok);
    // Wait for this own no-fault fixture to retain both nodes; no more launches needed.
    let end = Instant::now() + Duration::from_secs(6);
    while f.call(Request::Status {}).nodes.len() != 2 {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
    // Independent timer is not globally frozen, but this stable fixture must not gain attempts.

    let store = m8sd::Store::open(f.state().join("state.db"), OWNER).unwrap();
    let attempts = store.launch_count("bounded", 0).unwrap();
    drop(store);
    let explained = m8s_api::call_explain_tier(&socket, "bounded").unwrap();
    let data = explained.data.unwrap();
    assert_eq!(data.desired, Some(2));
    assert!(data.stored_node_count <= 2);
    assert_eq!(
        data.stored_phases.iter().map(|p| p.count).sum::<u32>(),
        data.stored_node_count
    );
    assert_eq!(
        f.call(Request::Status {}).tiers,
        vec![("bounded".into(), 2)]
    );
    let store = m8sd::Store::open(f.state().join("state.db"), OWNER).unwrap();
    assert!(store.launch_count("bounded", 0).unwrap() >= attempts);
    drop(store); // independent timer, not global freeze
    use std::{io::Write, os::unix::net::UnixStream};
    let mut raw = UnixStream::connect(&socket).unwrap();
    raw.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    raw.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    raw.write_all(b"{\"command\":\"explain_tier\",\"tier\":\"bad.id\"}\n")
        .unwrap();
    let refused = m8s_api::decode_explain_tier(&m8s_api::read_frame(&mut raw).unwrap()).unwrap();
    assert!(!refused.ok && refused.data.is_none());
    drop(raw);
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(d.wait().success());
    let mut legacy = f.start(false);
    let r = m8s_api::call_explain_tier(&socket, "legacy").unwrap();
    assert!(!r.configured && r.configured_tier.is_none());
    assert_eq!(r.data.unwrap().desired, None);
    assert!(f.call(Request::Shutdown {}).ok);
    assert!(legacy.wait().success());
}

#[test]
fn bounded_control_snapshot_overflow_refuses_request_but_daemon_and_stop_remain_available() {
    if snapshot_reader_child() {
        return;
    }
    let f = Fixture::new();
    let mut d = f.start(false);
    assert!(f.call(up("active")).ok);
    wait_for_mock_nodes(&f);
    // Restore the ORIGINAL live external Store open/drop, not a pinned observer.
    let writer = m8sd::Store::open(f.state().join("state.db"), OWNER).unwrap();
    for i in 0..256 {
        writer.scale(&format!("tier{i:03}"), 0).unwrap();
    }
    drop(writer);
    let refused = f.call(Request::Status {});
    assert!(!refused.ok);
    assert!(refused.nodes.is_empty() && refused.tiers.is_empty() && refused.events.is_empty());
    assert_eq!(
        refused.error.as_deref(),
        Some("control response unavailable; outcome uncertain; empty fields are not absence")
    );
    let socket = f.state().join("control.sock");
    let config = m8s_api::call_config_status(&socket).unwrap();
    assert!(config.ok && !config.configured);
    assert!(!f.call(Request::Down {}).ok);
    assert!(d.child().try_wait().unwrap().is_none());
    assert_eq!(fresh_snapshot_counts(&f, "down").0, 0);
    let fresh = m8sd::Store::open(f.state().join("state.db"), OWNER).unwrap();
    let tiers = fresh.tiers().unwrap();
    assert_eq!(tiers.len(), 257);
    assert!(tiers.iter().all(|(_, n)| *n == 0));
    drop(fresh);
    assert!(!f.call(Request::Shutdown {}).ok);
    assert!(d.child().try_wait().unwrap().is_none());
    assert!(m8s_api::call_config_status(&socket).unwrap().ok);
    assert_eq!(fresh_snapshot_counts(&f, "shutdown"), (0, 0));
    d.stop();
    let reopened = m8sd::Store::open(f.state().join("state.db"), OWNER).unwrap();
    assert!(reopened.nodes().unwrap().is_empty());
    let tiers = reopened.tiers().unwrap();
    assert_eq!(tiers.len(), 257);
    assert!(tiers.iter().all(|(_, n)| *n == 0));
}

// Test-only separate-process raw reader. The same real test dispatches this
// branch only when its parent supplies its own explicit fresh fixture path.
fn snapshot_reader_child() -> bool {
    let Some(path) = std::env::var_os("M8S_TEST_SNAPSHOT_DB") else {
        return false;
    };
    let conn = rusqlite::Connection::open_with_flags(
        PathBuf::from(path),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    conn.busy_timeout(Duration::from_secs(2)).unwrap();
    let desired: i64 = conn
        .query_row("SELECT coalesce(sum(desired),0) FROM tier", [], |r| {
            r.get(0)
        })
        .unwrap();
    let nodes: i64 = conn
        .query_row("SELECT count(*) FROM gpu_node", [], |r| r.get(0))
        .unwrap();
    println!("SNAPSHOT_COUNTS={desired},{nodes}");
    true
}
fn fresh_snapshot_counts(f: &Fixture, label: &str) -> (i64, i64) {
    use std::io::Read;
    let capture = f.root.join(format!("reader-{label}"));
    let out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&capture)
        .unwrap();
    let mut child=Daemon(Some(Command::new(std::env::current_exe().unwrap())
        .args(["--exact","bounded_control_snapshot_overflow_refuses_request_but_daemon_and_stop_remain_available","--test-threads=1","--nocapture"])
        .env_clear().env("M8S_TEST_SNAPSHOT_DB",f.state().join("state.db"))
        .stdin(Stdio::null()).stdout(out).stderr(Stdio::null()).spawn().unwrap()));
    assert!(child.wait().success());
    let mut text = String::new();
    fs::File::open(capture)
        .unwrap()
        .take(4096)
        .read_to_string(&mut text)
        .unwrap();
    assert!(text.len() < 4096);
    let counts = text
        .lines()
        .find_map(|s| s.split_once("SNAPSHOT_COUNTS=").map(|(_, n)| n))
        .unwrap();
    let (desired, nodes) = counts.split_once(',').unwrap();
    (desired.parse().unwrap(), nodes.parse().unwrap())
}

#[test]
fn managed_validation_leases_share_aliases_and_survive_failed_and_overlapping_opens() {
    let f = Fixture::new();
    fs::DirBuilder::new().mode(0o700).create(f.state()).unwrap();
    let db = f.state().join("state.db");
    let first = m8sd::Store::open(&db, OWNER).unwrap();
    first.scale("active", 1).unwrap();
    let alias = PathBuf::from("/tmp")
        .join(f.root.file_name().unwrap())
        .join("state/state.db");
    let second = m8sd::Store::open(&alias, OWNER).unwrap();
    assert!(m8sd::Store::open(&db, "different-owner").is_err());
    assert!(
        std::panic::catch_unwind(|| {
            let _unwinding = m8sd::Store::open(&db, OWNER).unwrap();
            panic!("synthetic constructor-owner unwind");
        })
        .is_err()
    );
    drop(first);
    second.scale("active", 0).unwrap();
    assert_eq!(fresh_snapshot_counts(&f, "overlap"), (0, 0));
    drop(second);
    let reopened = m8sd::Store::open(&db, OWNER).unwrap();
    reopened.scale("active", 1).unwrap();
    assert_eq!(fresh_snapshot_counts(&f, "new-generation"), (1, 0));
    drop(reopened);
}
