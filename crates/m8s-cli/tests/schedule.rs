//! Public offline command, only own explicit files and shipped catalog fixtures.
use serde_json::Value;
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
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let f = Self {
            config: root.join("SECRET-config"),
            catalog: root.join("SECRET-catalog"),
            _dir: dir,
        };
        fs::write(&f.config, m8s_config::EXAMPLE).unwrap();
        fs::write(&f.catalog, m8s_models::EXAMPLE_CATALOG).unwrap();
        f
    }
    fn args(&self) -> Vec<OsString> {
        vec![
            "schedule".into(),
            "preview".into(),
            "--config".into(),
            self.config.clone().into(),
            "--catalog".into(),
            self.catalog.clone().into(),
            "--tier".into(),
            "small".into(),
            "--from".into(),
            "1709553600".into(),
            "--hours".into(),
            "24".into(),
            "--limit".into(),
            "256".into(),
            "--json".into(),
        ]
    }
    fn run(&self) -> Output {
        let before = [&self.config, &self.catalog].map(|p| fs::read(p).unwrap());
        let out = invoke(&self.args());
        assert_eq!(
            before,
            [&self.config, &self.catalog].map(|p| fs::read(p).unwrap())
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
fn value(o: &Output) -> Value {
    assert!(o.status.success(), "{o:?}");
    assert!(o.stderr.is_empty());
    serde_json::from_slice(&o.stdout).unwrap()
}
fn refused(o: Output, code: &str) {
    assert!(!o.status.success());
    assert_eq!(o.stderr, format!("m8s: {code}\n").as_bytes());
    if !o.stdout.is_empty() {
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["error"], code);
    }
    assert!(!String::from_utf8_lossy(&o.stdout).contains("SECRET"));
}
#[test]
fn shipped_zoned_calendar_is_deterministic_text_json_and_inputs_unchanged() {
    let f = Fixture::new();
    let out = f.run();
    let v = value(&out);
    assert_eq!(f.run().stdout, out.stdout);
    let mut args = f.args();
    args.pop();
    assert_eq!(value(&invoke(&args)), v);
    let events = v["preview"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["kind"], "open");
    assert_eq!(
        events[0]["owner_local_timestamp"],
        "2024-03-04T08:00:00-05:00"
    );
    assert_eq!(events[1]["kind"], "close");
    assert_eq!(v["effects_performed"], false);
    assert_eq!(v["semantics"], "matching-events-only");
    assert!(!String::from_utf8(out.stdout).unwrap().contains("SECRET"));
    let mut args = f.args();
    args[13] = "1".into();
    let v = value(&invoke(&args));
    assert_eq!(v["preview"]["events"].as_array().unwrap().len(), 1);
    assert_eq!(v["preview"]["has_more"], true);
}
#[test]
fn context_subset_and_full_configuration_refusals_are_opaque() {
    let f = Fixture::new();
    let mut args = f.args();
    args[7] = "unknown".into();
    refused(invoke(&args), "schedule-context-unavailable");
    args[7] = "large".into();
    refused(invoke(&args), "schedule-context-unavailable");
    fs::write(
        &f.config,
        "schema=1\n[tier.small]\nmodel='example-27b-q4km'\n",
    )
    .unwrap();
    refused(f.run(), "schedule-context-unavailable");
    fs::write(
        &f.config,
        m8s_config::EXAMPLE.replace("0 8 * * 1-5", "0 8 1 * 1-5"),
    )
    .unwrap();
    refused(f.run(), "unsupported-schedule-preview");
    fs::write(
        &f.config,
        m8s_config::EXAMPLE.replace("max = 1", "max = 99"),
    )
    .unwrap();
    refused(f.run(), "invalid-schedule-configuration");
    fs::write(&f.config, m8s_config::EXAMPLE).unwrap();
    fs::write(
        &f.catalog,
        m8s_models::EXAMPLE_CATALOG.replace(
            "runtime_status = \"untested\"",
            "runtime_status = \"SECRET-invalid\"",
        ),
    )
    .unwrap();
    refused(f.run(), "invalid-schedule-configuration");
}
#[test]
fn strict_grammar_ranges_utf8_files_and_help() {
    let f = Fixture::new();
    for extra in [
        vec!["--json"],
        vec!["--tier", "SECRET"],
        vec!["--help"],
        vec!["SECRET"],
    ] {
        let mut args = f.args();
        args.extend(extra.into_iter().map(OsString::from));
        refused(invoke(&args), "invalid-schedule-arguments");
    }
    for (i, bad) in [
        (9, "-1"),
        (9, "+1"),
        (9, " 1"),
        (9, "9223372036854775808"),
        (11, "0"),
        (11, "169"),
        (13, "0"),
        (13, "257"),
        (13, "1e1"),
        (13, "18446744073709551616"),
    ] {
        let mut args = f.args();
        args[i] = bad.into();
        refused(invoke(&args), "invalid-schedule-arguments");
    }
    let mut args = f.args();
    args[9] = i64::MAX.to_string().into();
    refused(invoke(&args), "schedule-range-unavailable");
    let mut args = f.args();
    args.drain(10..12);
    refused(invoke(&args), "invalid-schedule-arguments");
    let mut args = f.args();
    args[3] = "SECRET-relative".into();
    refused(invoke(&args), "invalid-schedule-arguments");
    let mut args = f.args();
    args[3] = f.config.with_file_name("SECRET-missing").into();
    refused(invoke(&args), "invalid-schedule-configuration");
    #[cfg(unix)]
    {
        use std::os::unix::{ffi::OsStringExt, fs::symlink};
        let link = f.config.with_file_name("SECRET-link");
        symlink(&f.config, &link).unwrap();
        let mut args = f.args();
        args[3] = link.into();
        refused(invoke(&args), "invalid-schedule-configuration");
        let mut args = f.args();
        args[3] = OsString::from_vec(vec![255]);
        let out = invoke(&args);
        assert!(!out.status.success() && out.stdout.is_empty());
        assert_eq!(out.stderr, b"m8s: invalid arguments\n");
    }
    fs::write(&f.config, vec![b'x'; 65537]).unwrap();
    refused(f.run(), "invalid-schedule-configuration");
    for args in [
        vec!["schedule", "--help"],
        vec!["schedule", "preview", "--help"],
    ] {
        let out = invoke(&args.into_iter().map(OsString::from).collect::<Vec<_>>());
        assert!(out.status.success());
        assert!(
            String::from_utf8(out.stdout)
                .unwrap()
                .contains("no real clock")
        );
    }
}
