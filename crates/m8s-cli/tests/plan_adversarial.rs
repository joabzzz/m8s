//! Independent public CLI seam tests. No backend, secret or model execution.
//! Watched-fixture negatives are observations, not global effect-freedom proof.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};
const SENTINEL: &str = "synthetic-private-sentinel";
struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    catalog: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let dir = root.path().canonicalize().unwrap();
        let f = Self {
            config: dir.join(format!("{SENTINEL}-config")),
            catalog: dir.join(format!("{SENTINEL}-catalog")),
            root,
        };
        fs::write(&f.config, m8s_config::EXAMPLE).unwrap();
        fs::write(&f.catalog, m8s_core::models::EXAMPLE_CATALOG).unwrap();
        f
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
    fn run(&self, args: &[OsString]) -> Output {
        let stdout = tempfile::tempfile().unwrap();
        let stderr = tempfile::tempfile().unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_m8s"))
            .env_clear()
            .env("PATH", "/synthetic/no-executables")
            .env("HOME", self.root.path())
            .env("XDG_CONFIG_HOME", self.root.path().join("absent-config"))
            .env("XDG_DATA_HOME", self.root.path().join("absent-data"))
            .current_dir(self.root.path())
            .args(args)
            .stdin(Stdio::null())
            .stdout(stdout.try_clone().unwrap())
            .stderr(stderr.try_clone().unwrap())
            .spawn()
            .unwrap();
        let mut child = Reap(child);
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "bounded CLI timed out; Drop kills and reaps"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        Output {
            status,
            stdout: capture(stdout),
            stderr: capture(stderr),
        }
    }
    fn json(&self) -> Value {
        success(self.run(&self.args()))
    }
}
struct Reap(Child);
impl Drop for Reap {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
fn capture(mut file: File) -> Vec<u8> {
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    file.take(65_537).read_to_end(&mut bytes).unwrap();
    assert!(bytes.len() <= 65_536, "bounded stream exceeded");
    bytes
}
fn no_echo(o: &Output) {
    for bytes in [&o.stdout, &o.stderr] {
        assert!(
            !String::from_utf8_lossy(bytes).contains(SENTINEL),
            "synthetic input leaked"
        );
    }
}
fn success(o: Output) -> Value {
    no_echo(&o);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(o.stderr.is_empty());
    serde_json::from_slice(&o.stdout).unwrap()
}
fn refusal(o: Output, category: &str, json: bool) {
    no_echo(&o);
    assert!(!o.status.success());
    assert_eq!(o.stderr, format!("m8s: {category}\n").as_bytes());
    if json {
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(
            v,
            json!({"schema":"m8s.local-intent-plan/v1", "ok":false,"error":category})
        );
    } else {
        assert!(o.stdout.is_empty());
    }
}
fn inventory(path: &Path) -> Vec<(PathBuf, u32, Vec<u8>)> {
    let mut out = Vec::new();
    for e in fs::read_dir(path).unwrap() {
        let e = e.unwrap();
        let m = e.metadata().unwrap();
        let p = e.path();
        out.push((
            p.clone(),
            m.permissions().mode(),
            if m.is_file() {
                fs::read(&p).unwrap()
            } else {
                vec![]
            },
        ));
        if m.is_dir() {
            out.extend(inventory(&p));
        }
    }
    out.sort();
    out
}
fn pipe(path: &Path) {
    nix::unistd::mkfifo(
        path,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
}
fn honest(v: &Value) {
    assert_eq!(v["schema"], "m8s.local-intent-plan/v1");
    assert_eq!(v["status"], "intent-validated-not-authorized");
    for k in [
        "effects_performed",
        "local_execution_supported",
        "inference_served",
    ] {
        assert_eq!(v[k], false);
    }
    assert_eq!(v["authorization"], "not_checked");
    assert_eq!(v["billing"], "unknown");
    assert_eq!(v["readiness"], "not_assessed");
    assert_eq!(v["requirements"], "not_verified");
    assert_eq!(v["validation_limits_micro_usd"]["approval_evidence"], false);
    assert_eq!(v["policy_requirements"]["verified"], false);
    assert_eq!(v["dms_duration_descriptions"]["watchdog_guarantee"], false);
    for k in [
        "uid",
        "launch_fingerprint",
        "claim",
        "reservation",
        "cost_quote",
        "offer",
        "serving_deadline",
    ] {
        assert!(v.get(k).is_none());
    }
}
#[test]
fn malformed_grammar_precedes_both_fifo_loaders_and_never_routes_socket() {
    let f = Fixture::new();
    fs::remove_file(&f.config).unwrap();
    fs::remove_file(&f.catalog).unwrap();
    pipe(&f.config);
    pipe(&f.catalog);
    let socket = f.root.path().join("unused.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    for extra in [
        vec![OsString::from("--execute")],
        vec!["--socket".into(), socket.into()],
        vec!["--tier".into(), "large".into()],
        vec!["--config".into(), f.config.clone().into()],
    ] {
        let mut args = f.args();
        args.extend(extra);
        refusal(f.run(&args), "invalid-plan-arguments", true);
    }
    // Missing-value and flag-as-value must also finish before any FIFO read.
    let mut args = f.args();
    args[7] = "--approve".into();
    refusal(f.run(&args), "invalid-plan-arguments", true);
    assert!(matches!(listener.accept(), Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
}
#[test]
fn whole_argv_invalid_utf8_and_legacy_json_placement_remain_opaque() {
    let f = Fixture::new();
    for position in [0, 3, 5, 7, 9] {
        let mut args = f.args();
        let bad = OsString::from_vec(
            format!("{SENTINEL}-")
                .into_bytes()
                .into_iter()
                .chain([255])
                .collect(),
        );
        if position == args.len() {
            args.push(bad);
        } else {
            args[position] = bad;
        }
        refusal(f.run(&args), "invalid arguments", false);
    }
    let mut args = f.args();
    args.insert(0, "--json".into());
    let o = f.run(&args);
    no_echo(&o);
    assert!(!o.status.success());
    assert!(o.stdout.is_empty());
    assert!(String::from_utf8_lossy(&o.stderr).starts_with("m8s: usage:"));
    let o = f.run(&["plan".into(), "--help".into()]);
    assert!(o.status.success());
    no_echo(&o);
    assert!(o.stderr.is_empty());
}
#[test]
fn loader_refusal_preserves_read_only_target_bytes_modes_and_accepts_exact_cap() {
    for catalog in [false, true] {
        let f = Fixture::new();
        let p = if catalog { &f.catalog } else { &f.config };
        let bytes = fs::read(p).unwrap();
        let target = f.root.path().join("readonly-target");
        fs::write(&target, &bytes).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o400)).unwrap();
        fs::remove_file(p).unwrap();
        symlink(&target, p).unwrap();
        let category = if catalog {
            "invalid-plan-catalog"
        } else {
            "invalid-plan-configuration"
        };
        refusal(f.run(&f.args()), category, true);
        assert_eq!(fs::read(&target).unwrap(), bytes);
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o400
        );
        fs::remove_file(p).unwrap();
        pipe(p);
        refusal(f.run(&f.args()), category, true);
        fs::remove_file(p).unwrap();
        let mut exact = bytes;
        exact.resize(65_536, b' ');
        fs::write(p, &exact).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o400)).unwrap();
        let before = inventory(f.root.path());
        honest(&f.json());
        assert_eq!(inventory(f.root.path()), before);
        fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
        exact.push(b' ');
        fs::write(p, &exact).unwrap();
        let before = inventory(f.root.path());
        refusal(f.run(&f.args()), category, true);
        assert_eq!(inventory(f.root.path()), before);
    }
}
#[test]
fn explicit_files_never_discover_expand_or_repair_ambient_paths() {
    let f = Fixture::new();
    fs::write(f.root.path().join("m8s.toml"), "invalid ambient config").unwrap();
    fs::write(f.root.path().join("models.toml"), "invalid ambient catalog").unwrap();
    let literal = f.root.path().join("$HOME-catalog");
    fs::rename(&f.catalog, &literal).unwrap();
    let mut args = f.args();
    args[5] = literal.clone().into();
    let before = inventory(f.root.path());
    honest(&success(f.run(&args)));
    assert_eq!(inventory(f.root.path()), before);
    for path in [
        OsString::from("~/models.toml"),
        OsString::from("$HOME/models.toml"),
        f.root.path().join("absent-parent/catalog").into(),
    ] {
        let mut a = args.clone();
        a[5] = path;
        let relative = !Path::new(&a[5]).is_absolute();
        refusal(
            f.run(&a),
            if relative {
                "invalid-plan-arguments"
            } else {
                "invalid-plan-catalog"
            },
            true,
        );
    }
    let mut omitted = args.clone();
    omitted.drain(4..6);
    refusal(f.run(&omitted), "invalid-plan-arguments", true);
    // Valid ambient candidates must NOT rescue an explicitly invalid catalog.
    fs::write(
        f.root.path().join("models.toml"),
        m8s_core::models::EXAMPLE_CATALOG,
    )
    .unwrap();
    fs::write(&literal, "invalid explicit catalog").unwrap();
    let before = inventory(f.root.path());
    refusal(f.run(&args), "invalid-plan-catalog", true);
    assert_eq!(inventory(f.root.path()), before);
    assert!(!f.root.path().join("absent-parent").exists());
    assert!(!f.root.path().join("absent-config").exists());
    assert!(!f.root.path().join("absent-data").exists());
}
#[test]
fn catalog_freetext_and_paths_never_echo_in_text_json_or_errors() {
    let f = Fixture::new();
    let catalog = m8s_core::models::EXAMPLE_CATALOG
        .replace("example://", &format!("https://{SENTINEL}/?not-a-token="))
        .replace("owner-must-verify", SENTINEL)
        .replace("Q4_K_M", SENTINEL)
        .replace("untested", "certified");
    fs::write(&f.catalog, &catalog).unwrap();
    for json in [false, true] {
        let mut args = f.args();
        if !json {
            args.pop();
        }
        let o = f.run(&args);
        no_echo(&o);
        assert!(o.status.success());
        assert!(o.stderr.is_empty());
        let value = if json {
            serde_json::from_slice(&o.stdout).unwrap()
        } else {
            let text = std::str::from_utf8(&o.stdout).unwrap();
            serde_json::from_str(text.split_once('\n').unwrap().1).unwrap()
        };
        honest(&value);
        args[7] = format!("{SENTINEL}-tier").into();
        refusal(f.run(&args), "unknown-plan-tier", json);
        fs::write(&f.config, format!("schema = '{SENTINEL}'\n")).unwrap();
        args[7] = "small".into();
        refusal(f.run(&args), "invalid-plan-configuration", json);
        fs::write(&f.config, m8s_config::EXAMPLE).unwrap();
        fs::write(&f.catalog, catalog.replace("certified", SENTINEL)).unwrap();
        refusal(f.run(&args), "invalid-plan-configuration", json);
        fs::write(&f.catalog, &catalog).unwrap();
    }
}
#[test]
fn reordered_catalog_tables_and_equivalent_duration_spelling_have_identical_projection() {
    let f = Fixture::new();
    let baseline = f.run(&f.args());
    assert!(baseline.status.success());
    let (_, models) = m8s_core::models::EXAMPLE_CATALOG
        .split_once("[[models]]")
        .unwrap();
    let (first, second) = models.split_once("[[models]]").unwrap();
    fs::write(&f.catalog, format!("[[models]]{second}\n[[models]]{first}")).unwrap();
    let (prefix, tiers) = m8s_config::EXAMPLE.split_once("[tier.small]").unwrap();
    let (small, large) = tiers.split_once("[tier.large]").unwrap();
    fs::write(
        &f.config,
        format!("{prefix}\n[tier.large]{large}\n[tier.small]{small}\n")
            .replace("ttl = \"1h\"", "ttl = \"60m\"")
            .replace("max_lifetime = \"12h\"", "max_lifetime = \"720m\"")
            .replace("timeout = \"15m\"", "timeout = \"900s\""),
    )
    .unwrap();
    let reordered = f.run(&f.args());
    assert!(reordered.status.success());
    assert_eq!(baseline.stdout, reordered.stdout);
    honest(&f.json());
}
#[test]
fn existing_catalog_shape_and_global_validation_are_not_replaced_by_selected_model_filtering() {
    let f = Fixture::new();
    let control = f.json();
    // Current ModelProfile deserialize intentionally ignores unknown fields.
    let with_extra = m8s_core::models::EXAMPLE_CATALOG.replace(
        "[[models]]",
        "[[models]]\nnon_authority = 'synthetic-private-sentinel'",
    );
    fs::write(&f.catalog, with_extra).unwrap();
    assert_eq!(f.json(), control);
    for invalid in [
        m8s_core::models::EXAMPLE_CATALOG.replace("[[models]]", "[[profiles]]"),
        m8s_core::models::EXAMPLE_CATALOG
            .replace("weights_bytes = 42000000000", "weights_bytes = 0"),
    ] {
        fs::write(&f.catalog, invalid).unwrap();
        let o = f.run(&f.args());
        no_echo(&o);
        assert!(!o.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&o.stdout).unwrap()["ok"],
            false
        );
    }
    // Valid selected small model cannot hide invalid unused large metadata.
    fs::write(
        &f.catalog,
        m8s_core::models::EXAMPLE_CATALOG
            .replace("max_context_tokens = 32768", "max_context_tokens = 0"),
    )
    .unwrap();
    refusal(f.run(&f.args()), "invalid-plan-configuration", true);
    fs::write(&f.catalog, m8s_core::models::EXAMPLE_CATALOG).unwrap();
    assert_eq!(f.json(), control);
}
#[test]
fn fixed_validation_envelope_is_not_approval_or_local_gpu_execution_support() {
    let f = Fixture::new();
    let changed = m8s_config::EXAMPLE
        .replace(
            "max_instance_hourly_micro_usd = 1000000",
            "max_instance_hourly_micro_usd = 3000000",
        )
        .replace("ttl = \"1h\"", "ttl = \"90m\"")
        .replace("max_lifetime = \"12h\"", "max_lifetime = \"24h\"");
    fs::write(&f.config, &changed).unwrap();
    let v = f.json();
    honest(&v);
    assert_eq!(
        v["validation_limits_micro_usd"],
        json!({"maximum_monthly":20_000_000,"maximum_total_hourly":3_000_000,"maximum_instance_hourly":3_000_000,"approval_evidence":false})
    );
    assert_eq!(
        v["budget_ceilings_micro_usd"]["max_instance_hourly_micro_usd"],
        3_000_000
    );
    assert_eq!(v["dms_duration_descriptions"]["ttl_seconds"], 5400);
    assert_eq!(
        v["dms_duration_descriptions"]["max_lifetime_seconds"],
        86400
    );
    assert_eq!(
        v["provider_requirements"],
        json!(["vast-secure", "vast-community"])
    );
    assert_eq!(v["gpu_requirements"]["count"], 1);
    for invalid in [
        changed.replace(
            "monthly_micro_usd = 20000000",
            "monthly_micro_usd = 20000001",
        ),
        changed.replace(
            "max_total_hourly_micro_usd = 3000000",
            "max_total_hourly_micro_usd = 3000001",
        ),
    ] {
        fs::write(&f.config, invalid).unwrap();
        refusal(f.run(&f.args()), "invalid-plan-configuration", true);
    }
}
