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
const CATALOG: &str = include_str!("../../m8s-models/models/examples.toml");
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
