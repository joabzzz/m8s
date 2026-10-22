//! Whole-config public CLI regression using only shipped/synthetic explicit files.
use serde_json::Value;
use std::{
    ffi::OsString,
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
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
        let config = root.join("sensitive-path-config");
        let catalog = root.join("sensitive-path-catalog");
        fs::write(&config, m8s_config::EXAMPLE).unwrap();
        fs::write(&catalog, m8s_models::EXAMPLE_CATALOG).unwrap();
        Self {
            _dir: dir,
            config,
            catalog,
        }
    }
    fn args(&self) -> Vec<OsString> {
        vec![
            "config".into(),
            "validate".into(),
            "--config".into(),
            self.config.clone().into(),
            "--catalog".into(),
            self.catalog.clone().into(),
            "--json".into(),
        ]
    }
    fn run(&self) -> Output {
        let before = (
            fs::read(&self.config).unwrap(),
            fs::read(&self.catalog).unwrap(),
        );
        let out = invoke(&self.args());
        assert_eq!(
            (
                fs::read(&self.config).unwrap(),
                fs::read(&self.catalog).unwrap()
            ),
            before
        );
        out
    }
}
fn invoke(args: &[OsString]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_m8s"))
        .env_clear()
        .env("PATH", "/synthetic/no-executables")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn success(out: &Output) -> Value {
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty());
    assert!(out.stdout.len() <= 65536);
    serde_json::from_slice(&out.stdout).unwrap()
}
fn refused(out: &Output, error: &str) {
    assert!(!out.status.success());
    assert_eq!(out.stderr, format!("m8s: {error}\n").as_bytes());
    if !out.stdout.is_empty() {
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["schema"], "m8s.config-validation/v1");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], error);
    }
    for bytes in [&out.stdout, &out.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains("sensitive-"));
    }
}
#[test]
fn help_is_effect_free_and_discoverable() {
    for args in [
        vec!["config".into(), "--help".into()],
        vec!["config".into(), "validate".into(), "--help".into()],
        vec!["--help".into()],
    ] {
        let out = invoke(&args);
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        assert!(
            String::from_utf8(out.stdout)
                .unwrap()
                .contains("config validate")
        );
    }
}
#[cfg(unix)]
#[test]
fn shipped_whole_configuration_is_sorted_honest_deterministic_and_read_only() {
    let f = Fixture::new();
    let first = f.run();
    let v = success(&first);
    assert_eq!(first.stdout, f.run().stdout);
    assert_eq!(v["status"], "intent-validated-not-authorized");
    assert_eq!(v["configuration_schema"], 1);
    assert_eq!(v["tier_count"], 2);
    assert_eq!(v["catalog_model_count"], 2);
    assert_eq!(v["referenced_model_count"], 2);
    assert_eq!(
        v["tiers"],
        serde_json::json!([{"id":"large","model":"example-70b-q4km"},{"id":"small","model":"example-27b-q4km"}])
    );
    for key in [
        "effects_performed",
        "local_execution_supported",
        "inference_served",
    ] {
        assert_eq!(v[key], false);
    }
    for (key, expected) in [
        ("authorization", "not_checked"),
        ("billing", "unknown"),
        ("readiness", "not_assessed"),
        ("requirements", "not_verified"),
    ] {
        assert_eq!(v[key], expected);
    }
    let limits = m8s_config::ValidationLimits::default();
    assert_eq!(
        v["validation_limits_micro_usd"]["maximum_instance_hourly"],
        limits.maximum_instance_hourly_micro_usd
    );
    assert_eq!(v["validation_limits_micro_usd"]["approval_evidence"], false);
    assert_eq!(
        v["budget_ceilings_micro_usd"]["max_instance_hourly_micro_usd"],
        1_000_000
    );
    let mut text_args = f.args();
    text_args.pop();
    let text = invoke(&text_args);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    let (header, projection) = text.split_once('\n').unwrap();
    assert!(header.contains("NOT authorized or executable"));
    assert_eq!(serde_json::from_str::<Value>(projection).unwrap(), v);
    let args = f.args();
    assert_eq!(
        invoke(&[
            args[0].clone(),
            args[6].clone(),
            args[4].clone(),
            args[5].clone(),
            args[2].clone(),
            args[3].clone(),
            args[1].clone()
        ])
        .stdout,
        first.stdout
    );
    let (prefix, tiers) = m8s_config::EXAMPLE.split_once("[tier.small]").unwrap();
    let (small, large) = tiers.split_once("[tier.large]").unwrap();
    fs::write(
        &f.config,
        format!("{prefix}[tier.large]{large}\n[tier.small]{small}"),
    )
    .unwrap();
    assert_eq!(f.run().stdout, first.stdout);
    // Neither catalog free text nor owner/schedule descriptions enter the summary.
    let mut catalog: m8s_models::ModelCatalog =
        toml::from_str(m8s_models::EXAMPLE_CATALOG).unwrap();
    for m in &mut catalog.models {
        m.source = "https://sensitive-source.invalid".into();
        m.license = "sensitive-license\u{1b}".into();
        m.quant = "sensitive-quant\n".into();
    }
    fs::write(&f.catalog, toml::to_string(&catalog).unwrap()).unwrap();
    assert_eq!(f.run().stdout, first.stdout);
    for field in [
        "owner",
        "source",
        "license",
        "quant",
        "launch_fingerprint",
        "reservation",
        "claim",
    ] {
        assert!(v.get(field).is_none());
    }
}
#[cfg(unix)]
#[test]
fn invalid_unselected_tiers_catalog_and_budget_are_not_hidden() {
    let f = Fixture::new();
    for extra in [
        "\n[tier.unused]\nmodel='absent'\n",
        "\n[tier.unused]\nmodel='example-27b-q4km'\nreplicas={min=2,max=1}\n",
    ] {
        fs::write(&f.config, format!("{}{extra}", m8s_config::EXAMPLE)).unwrap();
        refused(&f.run(), "invalid-config-document");
    }
    fs::write(
        &f.config,
        m8s_config::EXAMPLE.replace(
            "monthly_micro_usd = 20000000",
            "monthly_micro_usd = 20000001",
        ),
    )
    .unwrap();
    refused(&f.run(), "invalid-config-document");
    fs::write(&f.config, m8s_config::EXAMPLE).unwrap();
    let mut catalog: m8s_models::ModelCatalog =
        toml::from_str(m8s_models::EXAMPLE_CATALOG).unwrap();
    let mut unused = catalog.models[0].clone();
    unused.id = "unused".into();
    unused.weights_bytes = 0;
    catalog.models.push(unused);
    fs::write(&f.catalog, toml::to_string(&catalog).unwrap()).unwrap();
    refused(&f.run(), "invalid-config-catalog");
    fs::write(&f.catalog, "sensitive-raw-catalog = [invalid").unwrap();
    refused(&f.run(), "invalid-config-catalog");
    fs::write(&f.catalog, m8s_models::EXAMPLE_CATALOG).unwrap();
    fs::write(&f.config, "sensitive-raw-config = [invalid").unwrap();
    refused(&f.run(), "invalid-config-document");
}
#[cfg(unix)]
#[test]
fn strict_grammar_redaction_and_shared_file_boundaries() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let f = Fixture::new();
    let good = f.args();
    for remove in [1..2, 2..4, 4..6] {
        let mut args = good.clone();
        args.drain(remove);
        refused(&invoke(&args), "invalid-config-arguments");
    }
    for extra in [
        vec!["validate"],
        vec!["--json"],
        vec!["--config", "/sensitive-duplicate"],
        vec!["--catalog", "/sensitive-duplicate"],
        vec!["--tier", "sensitive-argument"],
        vec!["--execute"],
        vec!["--socket", "/sensitive-path"],
        vec!["--help"],
    ] {
        let mut args = good.clone();
        args.extend(extra.into_iter().map(OsString::from));
        refused(&invoke(&args), "invalid-config-arguments");
    }
    let mut text_bad = good.clone();
    text_bad.drain(2..4);
    text_bad.pop();
    refused(&invoke(&text_bad), "invalid-config-arguments");
    let mut args = good.clone();
    args.push(OsString::from_vec(vec![255]));
    let out = invoke(&args);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, b"m8s: invalid arguments\n");
    for (index, category) in [
        (3, "invalid-config-document"),
        (5, "invalid-config-catalog"),
    ] {
        let mut args = good.clone();
        args[index] = "relative-sensitive-path".into();
        refused(&invoke(&args), "invalid-config-arguments");
        let missing = f.config.with_file_name("sensitive-missing");
        args[index] = missing.clone().into();
        refused(&invoke(&args), category);
        assert!(!missing.exists());
        args[index] = f.config.parent().unwrap().to_path_buf().into();
        refused(&invoke(&args), category);
        let link = f.config.with_file_name(format!("sensitive-link-{index}"));
        symlink(&f.config, &link).unwrap();
        args[index] = link.into();
        refused(&invoke(&args), category);
        let oversized = f
            .config
            .with_file_name(format!("sensitive-oversized-{index}"));
        fs::write(&oversized, vec![b'x'; 65537]).unwrap();
        args[index] = oversized.clone().into();
        refused(&invoke(&args), category);
        assert_eq!(fs::read(oversized).unwrap(), vec![b'x'; 65537]);
        let fifo = f.config.with_file_name(format!("sensitive-fifo-{index}"));
        nix::unistd::mkfifo(
            &fifo,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        args[index] = fifo.into();
        refused(&invoke(&args), category);
    }
    assert_eq!(fs::read(&f.config).unwrap(), m8s_config::EXAMPLE.as_bytes());
    assert_eq!(
        fs::read(&f.catalog).unwrap(),
        m8s_models::EXAMPLE_CATALOG.as_bytes()
    );
}
#[cfg(unix)]
#[test]
fn maximum_tier_structure_stays_bounded_and_counts_references() {
    let f = Fixture::new();
    let mut config = String::from("schema=1\n");
    let mut catalog: m8s_models::ModelCatalog =
        toml::from_str(m8s_models::EXAMPLE_CATALOG).unwrap();
    catalog.models[0].id = format!("m{}", "x".repeat(95));
    for n in 0..m8s_config::MAX_TIERS {
        config.push_str(&format!(
            "[tier.t{n:047}]\nmodel='{}'\n",
            catalog.models[0].id
        ));
    }
    fs::write(&f.config, config).unwrap();
    fs::write(&f.catalog, toml::to_string(&catalog).unwrap()).unwrap();
    let v = success(&f.run());
    assert_eq!(v["tier_count"], m8s_config::MAX_TIERS);
    assert_eq!(v["referenced_model_count"], 1);
    let mut args = f.args();
    args.pop();
    let text = invoke(&args);
    assert!(text.status.success());
    assert!(text.stdout.len() <= 65536);
}
