//! Public binary only; synthetic explicit files, no ambient secrets or services.
#![cfg(unix)]
use serde_json::Value;
use std::{
    ffi::OsString,
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
    },
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
struct Fixture {
    dir: tempfile::TempDir,
    config: PathBuf,
    catalog: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let config = root.join("sensitive-path-sentinel-config");
        let catalog = root.join("sensitive-path-sentinel-catalog");
        fs::write(&config, m8s_config::EXAMPLE).unwrap();
        fs::write(&catalog, m8s_core::models::EXAMPLE_CATALOG).unwrap();
        Self {
            dir,
            config,
            catalog,
        }
    }
    fn args(&self) -> Vec<OsString> {
        vec![
            "plan".into(),
            "--local-development".into(),
            "--config".into(),
            self.config.clone().into(),
            "--catalog".into(),
            self.catalog.clone().into(),
            "--tier".into(),
            "small".into(),
            "--json".into(),
        ]
    }
    fn run(&self) -> Output {
        invoke(&self.args())
    }
    fn json(&self) -> Value {
        let output = self.run();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert!(output.stdout.len() <= 65_536);
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
fn invoke(args: &[OsString]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_m8s"))
        .env_clear()
        .env("PATH", "/synthetic/no-executables")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("bounded plan invocation timed out");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    child.wait_with_output().unwrap()
}
fn refused(output: Output) {
    assert!(!output.status.success());
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        for sentinel in [
            "sensitive-path-sentinel",
            "sensitive-url-sentinel",
            "raw-toml-sentinel",
            "execute-sentinel",
        ] {
            assert!(!text.contains(sentinel), "input leaked");
        }
        assert!(!text.contains("intent-validated-not-authorized"));
    }
    if !output.stdout.is_empty() {
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ok"], false);
        assert!(
            value["error"]
                .as_str()
                .unwrap()
                .starts_with("invalid-plan-")
                || value["error"] == "unknown-plan-tier"
        );
    }
}
#[test]
fn shipped_intent_is_deterministic_honest_and_read_only() {
    let f = Fixture::new();
    let before = (fs::read(&f.config).unwrap(), fs::read(&f.catalog).unwrap());
    let first = f.run();
    assert_eq!(first.stdout, f.run().stdout);
    let v = f.json();
    assert_eq!(v["schema"], "m8s.local-intent-plan/v1");
    assert_eq!(v["status"], "intent-validated-not-authorized");
    for key in [
        "effects_performed",
        "local_execution_supported",
        "inference_served",
    ] {
        assert_eq!(v[key], false);
    }
    assert_eq!(v["authorization"], "not_checked");
    assert_eq!(v["billing"], "unknown");
    assert_eq!(v["readiness"], "not_assessed");
    assert_eq!(v["requirements"], "not_verified");
    for key in [
        "launch_fingerprint",
        "uid",
        "offer",
        "claim",
        "cost_quote",
        "reservation",
    ] {
        assert!(v.get(key).is_none());
    }
    let args = f.args();
    let reordered = [
        args[0].clone(),
        args[8].clone(),
        args[6].clone(),
        args[7].clone(),
        args[4].clone(),
        args[5].clone(),
        args[2].clone(),
        args[3].clone(),
        args[1].clone(),
    ];
    assert_eq!(first.stdout, invoke(&reordered).stdout);
    assert_eq!(v["gpu_requirements"]["count"], 1);
    assert_eq!(v["gpu_requirements"]["min_vram_bytes"], 24_000_000_000u64);
    assert_eq!(v["provider_requirements"][0], "vast-secure");
    assert_eq!(v["behavior_descriptions"]["descriptive_not_executed"], true);
    assert_eq!(
        v["behavior_descriptions"]["queue"]["scale_up_for_seconds"],
        300
    );
    assert_eq!(
        v["behavior_descriptions"]["schedules"][0]["open"],
        "0 8 * * 1-5"
    );
    let mut text = f.args();
    text.pop();
    let text = invoke(&text);
    assert!(text.status.success());
    assert!(
        String::from_utf8(text.stdout)
            .unwrap()
            .starts_with("Owner intent only:")
    );
    assert_eq!(
        before,
        (fs::read(&f.config).unwrap(), fs::read(&f.catalog).unwrap())
    );
    assert_eq!(fs::read_dir(f.dir.path()).unwrap().count(), 2);
    // Reverse the two tier tables and change whitespace; no output identity change.
    let (prefix, tiers) = m8s_config::EXAMPLE.split_once("[tier.small]").unwrap();
    let (small, large) = tiers.split_once("[tier.large]").unwrap();
    fs::write(
        &f.config,
        format!("{prefix}\n[tier.large]{large}\n[tier.small]{small}\n"),
    )
    .unwrap();
    assert_eq!(first.stdout, f.run().stdout);
}
#[test]
fn changed_valid_intent_and_descriptors_are_reflected_not_executed() {
    let f = Fixture::new();
    let changed = m8s_config::EXAMPLE
        .replace(
            "monthly_micro_usd = 20000000",
            "monthly_micro_usd = 19000000",
        )
        .replace("ttl = \"1h\"", "ttl = \"2h\"")
        .replace(
            "model = \"example-27b-q4km\"",
            "model = \"example-70b-q4km\"",
        )
        .replace("max = 2", "max = 3")
        .replace("count = 1", "count = 2")
        .replace("minimum_down_mbps = 1000", "minimum_down_mbps = 2000");
    fs::write(&f.config, changed).unwrap();
    let v = f.json();
    assert_eq!(v["model"], "example-70b-q4km");
    assert_eq!(v["replicas"]["max"], 3);
    assert_eq!(v["gpu_requirements"]["count"], 2);
    assert_eq!(
        v["budget_ceilings_micro_usd"]["monthly_micro_usd"],
        19_000_000
    );
    assert_eq!(
        v["validation_limits_micro_usd"]["maximum_monthly"],
        20_000_000
    );
    assert_eq!(v["dms_duration_descriptions"]["ttl_seconds"], 7200);
    assert_eq!(v["policy_requirements"]["minimum_down_mbps"], 2000);
    let mut args = f.args();
    args[7] = "large".into();
    let output = invoke(&args);
    assert!(output.status.success());
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        v["behavior_descriptions"]["cold"],
        serde_json::json!({"mode":"fallback","tier":"small"})
    );
    let catalog = m8s_core::models::EXAMPLE_CATALOG.replace("untested", "certified");
    fs::write(&f.catalog, catalog).unwrap();
    assert_eq!(f.json()["readiness"], "not_assessed");
}
#[test]
fn strict_grammar_refuses_before_files_and_help_is_effect_free() {
    let f = Fixture::new();
    fs::remove_file(&f.config).unwrap();
    fs::remove_file(&f.catalog).unwrap();
    for extra in [
        "--socket",
        "--execute",
        "--approve",
        "--apply",
        "--command",
        "--owner",
        "--budget-state",
        "--reset",
        "execute-sentinel",
        "--local-development",
        "--json",
        "--config",
        "--catalog",
        "--tier",
    ] {
        let mut args = f.args();
        args.push(extra.into());
        let o = invoke(&args);
        assert!(String::from_utf8_lossy(&o.stdout).contains("invalid-plan-arguments"));
        refused(o);
    }
    let base = f.args();
    for range in [1..2, 2..4, 4..6, 6..8] {
        let mut args = base.clone();
        args.drain(range);
        refused(invoke(&args));
    }
    let mut args = base.clone();
    args[3] = "relative".into();
    refused(invoke(&args));
    let mut args = base;
    args.push(OsString::from_vec(vec![255]));
    refused(invoke(&args));
    let o = invoke(&["plan".into(), "--help".into()]);
    assert!(o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("NOT")
            || String::from_utf8_lossy(&o.stdout).contains("not authorized")
    );
    refused(invoke(&[
        "plan".into(),
        "--help".into(),
        "sensitive-path-sentinel".into(),
    ]));
    assert_eq!(fs::read_dir(f.dir.path()).unwrap().count(), 0);
}
#[test]
fn invalid_config_catalog_and_references_are_opaque() {
    let f = Fixture::new();
    for config in [
        "raw-toml-sentinel",
        "schema = 1\nunknown = 'raw-toml-sentinel'",
        &m8s_config::EXAMPLE.replace(
            "monthly_micro_usd = 20000000",
            "monthly_micro_usd = 20000001",
        ),
        &m8s_config::EXAMPLE.replace("example-27b-q4km", "absent"),
        &m8s_config::EXAMPLE.replace(
            "cold = { mode = \"wait\", timeout = \"15m\" }",
            "cold = { mode = \"fallback\", tier = \"large\" }",
        ),
        &m8s_config::EXAMPLE.replace("schema = 1", "schema = 1\nunknown = true"),
    ] {
        fs::write(&f.config, config).unwrap();
        refused(f.run());
    }
    fs::write(&f.config, m8s_config::EXAMPLE).unwrap();
    for catalog in [
        "raw-toml-sentinel".to_owned(),
        format!(
            "{}\n{}",
            m8s_core::models::EXAMPLE_CATALOG,
            m8s_core::models::EXAMPLE_CATALOG
        ),
        m8s_core::models::EXAMPLE_CATALOG.replace("example-27b-q4km", "absent"),
        m8s_core::models::EXAMPLE_CATALOG.replace("untested", "crypto-looking-raw-toml-sentinel"),
    ] {
        fs::write(&f.catalog, catalog).unwrap();
        refused(f.run());
    }
    fs::write(
        &f.catalog,
        m8s_core::models::EXAMPLE_CATALOG
            .replace("untested", "certified")
            .replace("example://", "https://sensitive-url-sentinel/"),
    )
    .unwrap();
    let output = f.run();
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("sensitive-url-sentinel"));
    let mut args = f.args();
    args[7] = "absent".into();
    refused(invoke(&args));
    fs::remove_file(&f.catalog).unwrap();
    refused(f.run());
}
#[test]
fn both_loaders_refuse_special_symlink_and_cap_plus_one_without_blocking() {
    for catalog in [false, true] {
        let f = Fixture::new();
        let path = if catalog { &f.catalog } else { &f.config };
        let original = fs::read(path).unwrap();
        fs::remove_file(path).unwrap();
        fs::create_dir(path).unwrap();
        refused(f.run());
        fs::remove_dir(path).unwrap();
        let target = f.dir.path().join("target");
        fs::write(&target, &original).unwrap();
        symlink(&target, path).unwrap();
        refused(f.run());
        fs::remove_file(path).unwrap();
        let socket = std::os::unix::net::UnixListener::bind(path).unwrap();
        refused(f.run());
        drop(socket);
        fs::remove_file(path).unwrap();
        let cpath = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // Synthetic named pipe; no writer is ever opened.
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        refused(f.run());
        fs::remove_file(path).unwrap();
        fs::write(path, vec![b' '; 65_537]).unwrap();
        refused(f.run());
        let mut exact = original.clone();
        exact.resize(65_536, b' ');
        fs::write(path, exact).unwrap();
        assert!(f.run().status.success());
        fs::write(path, original).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(f.run().status.success());
    }
}
