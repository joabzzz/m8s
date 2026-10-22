//! Authored offline replay only; no daemon, clocks, metrics or provider fixtures.
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    path::PathBuf,
    process::{Command, Output},
};
struct Fixture {
    _dir: tempfile::TempDir,
    config: PathBuf,
    catalog: PathBuf,
    samples: PathBuf,
}
impl Fixture {
    fn new(records: Value) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let f = Self {
            config: root.join("SECRET-config"),
            catalog: root.join("SECRET-catalog"),
            samples: root.join("SECRET-samples"),
            _dir: dir,
        };
        fs::write(
            &f.config,
            m8s_config::EXAMPLE
                .replace("\"5m\"", "\"4s\"")
                .replace("\"30m\"", "\"4s\""),
        )
        .unwrap();
        fs::write(&f.catalog, m8s_models::EXAMPLE_CATALOG).unwrap();
        f.records(records);
        f
    }
    fn records(&self, r: Value) {
        fs::write(
            &self.samples,
            serde_json::to_vec(&json!(["m8s.queue-samples/v1", r])).unwrap(),
        )
        .unwrap();
    }
    fn args(&self) -> Vec<OsString> {
        vec![
            "queue".into(),
            "replay".into(),
            "--config".into(),
            self.config.clone().into(),
            "--catalog".into(),
            self.catalog.clone().into(),
            "--tier".into(),
            "small".into(),
            "--samples".into(),
            self.samples.clone().into(),
            "--freshness-seconds".into(),
            "2".into(),
            "--max-gap-seconds".into(),
            "2".into(),
            "--json".into(),
        ]
    }
    fn run(&self) -> Output {
        let before = [&self.config, &self.catalog, &self.samples].map(|p| fs::read(p).unwrap());
        let out = invoke(&self.args());
        assert_eq!(
            before,
            [&self.config, &self.catalog, &self.samples].map(|p| fs::read(p).unwrap())
        );
        out
    }
}
fn invoke(args: &[OsString]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_m8s"))
        .args(args)
        .env_clear()
        .output()
        .unwrap()
}
fn value(out: &Output) -> Value {
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}
fn refusal(out: Output, code: &str) {
    assert!(!out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"schema":"m8s.queue-replay/v1","ok":false,"error":code})
    );
    assert!(!String::from_utf8_lossy(&out.stderr).contains("SECRET"));
    assert!(out.stderr.len() < 128);
}
#[test]
fn actual_kernel_proposals_counterfactual_determinism_text_and_no_input_changes() {
    let f = Fixture::new(json!([
        [0, 0, "enabled", [0, 2001, 0]],
        [2, 0, "enabled", [2, 0, 5]],
        [4, 0, "enabled", [4, 2001, 0]],
        [5, 0, "enabled", [5, 2001, 0]],
        [6, 2, "enabled", [6, 0, 0]],
        [8, 2, "enabled", [8, 0, 0]],
        [10, 2, "enabled", [10, 0, 0]]
    ]));
    let a = f.run();
    let v = value(&a);
    assert_eq!(f.run().stdout, a.stdout);
    let mut args = f.args();
    args.pop();
    assert_eq!(value(&invoke(&args)), v);
    assert_eq!(
        v["records"][2]["proposal"],
        json!({"target":1,"reason":"sustained-pressure"})
    );
    assert_eq!(v["records"][3]["proposal"], Value::Null); // no feedback, no immediate repeat
    assert_eq!(
        v["records"][6]["proposal"],
        json!({"target":1,"reason":"sustained-idle"})
    );
    assert_eq!(v["policy"]["scale_up_for_seconds"], 4);
    assert_eq!(v["simulation"], "authored-counterfactual-no-feedback");
    assert_eq!(v["authorization"], "proposals-not-authorized");
    assert!(!String::from_utf8_lossy(&a.stdout).contains("SECRET"));
}
#[test]
fn thresholds_unknown_pause_stale_gap_disabled_and_missing_queue() {
    let f = Fixture::new(json!([
        [0, 0, "enabled", [0, 2000, 4]],
        [2, 0, "enabled", [2, 2000, 4]],
        [4, 0, "enabled", [4, 2000, 4]],
        [5, 0, "paused", [5, 3000, 9]],
        [6, 0, "enabled", null],
        [10, 0, "enabled", [7, 3000, 9]],
        [11, 0, "enabled", [11, 3000, 9]],
        [14, 0, "enabled", [14, 3000, 9]],
        [16, 0, "enabled", [16, 3000, 9]]
    ]));
    assert!(
        value(&f.run())["records"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["proposal"].is_null())
    );
    let original = fs::read_to_string(&f.config).unwrap();
    fs::write(&f.config, original.replace("max = 2", "max = 0")).unwrap();
    f.records(json!([
        [0, 0, "enabled", [0, 3000, 9]],
        [2, 0, "enabled", [2, 3000, 9]],
        [4, 0, "enabled", [4, 3000, 9]]
    ]));
    assert!(
        value(&f.run())["records"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["proposal"].is_null())
    );
    let mut args = f.args();
    args[7] = "large".into();
    refusal(invoke(&args), "invalid-queue-tier");
}
#[test]
fn invalid_observation_fails_whole_command_and_whole_configuration_is_checked() {
    let f = Fixture::new(json!([[0, 0, "enabled", [1, 3000, 9]]]));
    refusal(f.run(), "invalid-queue-observation");
    for records in [
        json!([[2, 0, "enabled", null], [2, 0, "enabled", null]]),
        json!([[0, 3, "enabled", null]]),
        json!([
            [1, 0, "enabled", [1, 3000, 9]],
            [2, 0, "enabled", [1, 3000, 9]]
        ]),
    ] {
        f.records(records);
        refusal(f.run(), "invalid-queue-observation");
    }
    let original = fs::read_to_string(&f.config).unwrap();
    fs::write(&f.config, original.replace("max = 1", "max = 99")).unwrap();
    refusal(f.run(), "invalid-queue-configuration");
    fs::write(&f.config, original).unwrap();
    let catalog = fs::read_to_string(&f.catalog).unwrap();
    fs::write(
        &f.catalog,
        catalog.replace(
            "runtime_status = \"untested\"",
            "runtime_status = \"SECRET-invalid\"",
        ),
    )
    .unwrap();
    refusal(f.run(), "invalid-queue-configuration");
}
#[test]
fn strict_schema_caps_and_no_sensitive_payload_echo() {
    let f = Fixture::new(json!([]));
    for data in [
        b"SECRET".to_vec(),
        b"{\"SECRET\":1,\"SECRET\":2}".to_vec(),
        serde_json::to_vec(&json!(["m8s.queue-samples/v1", [[0, 0, "SECRET", null]]])).unwrap(),
        serde_json::to_vec(&json!([
            "m8s.queue-samples/v1",
            [[0, 0, "enabled", [0, 1, 2, 3]]]
        ]))
        .unwrap(),
        serde_json::to_vec(&json!([
            "m8s.queue-samples/v1",
            vec![json!([0, 0, "enabled", null]); 257]
        ]))
        .unwrap(),
        vec![b' '; 65537],
        b"[\"m8s.queue-samples/v1\",[[0.5,0,\"enabled\",null]]]".to_vec(),
        b"[\"m8s.queue-samples/v1\",[[-1,0,\"enabled\",null]]]".to_vec(),
        b"\xff".to_vec(),
    ] {
        fs::write(&f.samples, data).unwrap();
        refusal(f.run(), "invalid-queue-samples");
    }
    f.records(json!([]));
    refusal(f.run(), "invalid-queue-samples");
    f.records(json!(
        (0..256)
            .map(|t| json!([t, 0, "enabled", null]))
            .collect::<Vec<_>>()
    ));
    assert_eq!(value(&f.run())["records"].as_array().unwrap().len(), 256);
}
#[test]
fn strict_flags_effectfree_help_redaction_and_required_explicit_timing() {
    let f = Fixture::new(json!([[0, 0, "enabled", null]]));
    for extra in [
        vec!["--json"],
        vec!["--config", "SECRET"],
        vec!["SECRET"],
        vec!["--help"],
    ] {
        let mut args = f.args();
        args.extend(extra.into_iter().map(OsString::from));
        refusal(invoke(&args), "invalid-queue-arguments");
    }
    for bad in [
        "0",
        "-1",
        "+1",
        " 1",
        "1e2",
        "18446744073709551616",
        "9223372036854775808",
        "SECRET",
    ] {
        let mut args = f.args();
        args[11] = bad.into();
        refusal(invoke(&args), "invalid-queue-arguments");
    }
    let mut args = f.args();
    args.drain(10..12);
    refusal(invoke(&args), "invalid-queue-arguments");
    let mut args = f.args();
    args[3] = "SECRET-relative".into();
    refusal(invoke(&args), "invalid-queue-arguments");
    for args in [vec!["queue", "--help"], vec!["queue", "replay", "--help"]] {
        let out = invoke(&args.into_iter().map(OsString::from).collect::<Vec<_>>());
        assert!(out.status.success());
        assert!(
            String::from_utf8(out.stdout)
                .unwrap()
                .contains("no feedback")
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let mut args = f.args();
        args[3] = OsString::from_vec(vec![0xff]);
        let out = invoke(&args);
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert!(out.stderr.len() < 128);
    }
}
#[cfg(unix)]
#[test]
fn shared_nofollow_bounded_regular_file_loader_refusals() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new(json!([[0, 0, "enabled", null]]));
    let root = f.samples.parent().unwrap();
    let link = root.join("SECRET-link");
    symlink(&f.samples, &link).unwrap();
    let missing = root.join("SECRET-missing");
    let fifo = root.join("SECRET-fifo");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR).unwrap();
    for path in [&link, &missing, &root.to_path_buf(), &fifo] {
        let mut args = f.args();
        args[9] = path.clone().into();
        refusal(invoke(&args), "invalid-queue-samples");
    }
    assert!(!missing.exists());
}
