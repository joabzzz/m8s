//! Public CLI metadata only; explicit shipped/synthetic files and owned children.
use serde_json::Value;
use std::{
    ffi::OsString,
    fs,
    io::Read,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};
const OUTPUT_CAP: usize = 256 * 1024;
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
fn invoke(args: &[OsString]) -> Output {
    let mut child = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_m8s"))
            .env_clear()
            .env("PATH", "/synthetic/no-executables")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    // Drain concurrently so valid output exceeding a pipe buffer cannot stall.
    let stdout = child.0.stdout.take().unwrap();
    let stderr = child.0.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(OUTPUT_CAP as u64 + 1)
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(65537).read_to_end(&mut bytes).unwrap();
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "owned models CLI invocation timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}
struct Fixture {
    dir: tempfile::TempDir,
    catalog: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("sensitive-path-marker-catalog");
        fs::write(&catalog, m8s_models::EXAMPLE_CATALOG).unwrap();
        Self { dir, catalog }
    }
    fn args(&self, action: &[&str]) -> Vec<OsString> {
        let mut args = vec![
            "models".into(),
            "--catalog".into(),
            self.catalog.clone().into(),
        ];
        args.extend(action.iter().map(OsString::from));
        args
    }
    fn invoke(&self, action: &[&str]) -> Output {
        let before = fs::read(&self.catalog).unwrap();
        let out = invoke(&self.args(action));
        assert_eq!(fs::read(&self.catalog).unwrap(), before);
        out
    }
    fn write(&self, catalog: &m8s_models::ModelCatalog) {
        fs::write(&self.catalog, toml::to_string(catalog).unwrap()).unwrap();
    }
}
fn catalog() -> m8s_models::ModelCatalog {
    toml::from_str(m8s_models::EXAMPLE_CATALOG).unwrap()
}
fn success(out: &Output) -> Value {
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty());
    assert!(out.stdout.len() <= OUTPUT_CAP);
    serde_json::from_slice(&out.stdout).unwrap()
}
fn refused(out: &Output, category: &str) {
    assert!(!out.status.success());
    for bytes in [&out.stdout, &out.stderr] {
        let text = String::from_utf8_lossy(bytes);
        for marker in [
            "sensitive-path-marker",
            "sensitive-source-marker",
            "sensitive-license-marker",
            "sensitive-quant-marker",
            "sensitive-argument-marker",
            "raw-toml-marker",
        ] {
            assert!(!text.contains(marker), "input echoed");
        }
    }
    assert_eq!(out.stderr, format!("m8s: {category}\n").as_bytes());
    if !out.stdout.is_empty() {
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["schema"], "m8s.model-catalog/v1");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], category);
    }
}
#[test]
fn standalone_help_and_top_level_guide_are_effect_free() {
    let f = Fixture::new();
    let before = fs::read(&f.catalog).unwrap();
    for args in [
        vec!["--help".into()],
        vec!["models".into(), "--help".into()],
    ] {
        let out = invoke(&args);
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("models"));
        assert!(text.contains("--catalog"));
    }
    let guide = invoke(&["--help".into()]);
    let text = String::from_utf8(guide.stdout).unwrap();
    for command in [
        "models",
        "plan",
        "local-demo",
        "local-inspect",
        "secrets",
        "Mock-only",
        "top",
        "default OFF",
    ] {
        assert!(text.contains(command));
    }
    assert_eq!(fs::read(&f.catalog).unwrap(), before);
}
#[cfg(unix)]
#[test]
fn shipped_list_show_sorted_deterministic_and_text_matches_json() {
    let f = Fixture::new();
    let first = f.invoke(&["list", "--json"]);
    let v = success(&first);
    assert_eq!(first.stdout, f.invoke(&["--json", "list"]).stdout);
    assert_eq!(v["schema"], "m8s.model-catalog/v1");
    assert_eq!(v["catalog_metadata_only"], true);
    for (key, expected) in [
        ("runtime_verification", "not-performed"),
        ("deployment", "not-assessed"),
        ("approval", "not-assessed"),
    ] {
        assert_eq!(v[key], expected);
    }
    let mut models = catalog();
    models.models.reverse();
    f.write(&models);
    assert_eq!(first.stdout, f.invoke(&["list", "--json"]).stdout);
    models.models.sort_by(|a, b| a.id.cmp(&b.id));
    for (i, model) in models.models.iter().enumerate() {
        let row = &v["models"][i];
        assert_eq!(row["id"], model.id);
        assert_eq!(row["params_billions"], model.params_billions);
        assert_eq!(row["weights_bytes"], model.weights_bytes);
        assert_eq!(
            row["runtime_overhead_bytes_per_replica"],
            model.runtime_overhead_bytes_per_replica
        );
        assert_eq!(
            row["kv_bytes_per_1024_tokens"],
            model.kv_bytes_per_1024_tokens
        );
        assert_eq!(row["max_context_tokens"], model.max_context_tokens);
        assert_eq!(row["declared_license_class"], model.license_class);
        assert_eq!(row["declared_runtime_status"], model.runtime_status);
        let shown = success(&f.invoke(&["show", &model.id, "--json"]));
        assert_eq!(shown["models"].as_array().unwrap().len(), 1);
        assert_eq!(shown["models"][0], *row);
        let reordered = invoke(&[
            "models".into(),
            "--json".into(),
            "show".into(),
            "--catalog".into(),
            f.catalog.clone().into(),
            model.id.clone().into(),
        ]);
        assert_eq!(success(&reordered), shown);
    }
    let text = f.invoke(&["list"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    let (header, projection) = text.split_once('\n').unwrap();
    assert!(header.contains("NOT runtime verification or approval"));
    assert_eq!(serde_json::from_str::<Value>(projection).unwrap(), v);
}
#[cfg(unix)]
#[test]
fn sensitive_free_text_is_omitted_and_certified_is_only_declared() {
    let f = Fixture::new();
    let mut c = catalog();
    for m in &mut c.models {
        m.source = "https://sensitive-source-marker.invalid/private".into();
        m.license = "sensitive-license-marker\n\u{1b}[31m".into();
        m.quant = "sensitive-quant-marker\t\u{1b}[0m".into();
        m.runtime_status = "certified".into();
    }
    f.write(&c);
    for flags in [
        vec!["list"],
        vec!["list", "--json"],
        vec!["show", "example-27b-q4km", "--json"],
    ] {
        let out = f.invoke(&flags);
        assert!(out.status.success());
        let text = String::from_utf8(out.stdout).unwrap();
        for marker in [
            "sensitive-",
            "\u{1b}",
            "https://",
            "launch_fingerprint",
            "Ready",
        ] {
            assert!(!text.contains(marker));
        }
        let json = if flags.contains(&"--json") {
            text.as_str()
        } else {
            text.split_once('\n').unwrap().1
        };
        let v: Value = serde_json::from_str(json).unwrap();
        assert_eq!(v["models"][0]["declared_runtime_status"], "certified");
        assert_eq!(v["runtime_verification"], "not-performed");
        assert_eq!(v["approval"], "not-assessed");
        for field in ["source", "license", "quant"] {
            assert!(v["models"][0].get(field).is_none());
        }
    }
}
#[cfg(unix)]
#[test]
fn entire_catalog_validated_before_even_show_or_unknown_lookup() {
    let f = Fixture::new();
    let mutations: &[fn(&mut m8s_models::ModelProfile)] = &[
        |m| m.weights_bytes = 0,
        |m| {
            m.weights_bytes = u64::MAX;
            m.runtime_overhead_bytes_per_replica = 1;
        },
        |m| m.runtime_status = "sensitive-argument-marker".into(),
        |m| m.license_class = "invalid".into(),
        |m| m.source = "sensitive-source-marker\ncontrol".into(),
    ];
    for change in mutations {
        let mut c = catalog();
        let mut unused = c.models[0].clone();
        unused.id = "unused".into();
        change(&mut unused);
        c.models.push(unused);
        f.write(&c);
        for flags in [
            vec!["list", "--json"],
            vec!["show", "example-27b-q4km", "--json"],
            vec!["show", "absent", "--json"],
        ] {
            refused(&f.invoke(&flags), "invalid-models-catalog");
        }
    }
    for c in [
        m8s_models::ModelCatalog { models: vec![] },
        {
            let mut c = catalog();
            c.models.push(c.models[0].clone());
            c
        },
        m8s_models::ModelCatalog {
            models: (0..=m8s_config::MAX_MODELS)
                .map(|i| {
                    let mut m = catalog().models.remove(0);
                    m.id = format!("model-{i}");
                    m
                })
                .collect(),
        },
    ] {
        f.write(&c);
        refused(&f.invoke(&["list", "--json"]), "invalid-models-catalog");
    }
    fs::write(&f.catalog, "raw-toml-marker = [invalid").unwrap();
    refused(&f.invoke(&["list", "--json"]), "invalid-models-catalog");
    fs::write(&f.catalog, [255, 0, 254]).unwrap();
    refused(&f.invoke(&["list", "--json"]), "invalid-models-catalog");
}
#[cfg(unix)]
#[test]
fn strict_grammar_unknown_id_and_argv_utf8_are_redacted() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    for flags in [
        vec![],
        vec!["show"],
        vec!["show", "Invalid"],
        vec!["list", "extra"],
        vec!["list", "--json", "--json"],
        vec!["--help", "list"],
        vec!["list", "--socket", "sensitive-argument-marker"],
        vec!["list", "--catalog", "/sensitive-path-marker"],
        vec!["show", "sensitive-argument-marker", "extra"],
        vec!["list", "--download"],
        vec!["--catalog"],
    ] {
        let mut flags = flags;
        flags.push("--json");
        refused(&f.invoke(&flags), "invalid-models-arguments");
    }
    refused(&f.invoke(&["show", "absent", "--json"]), "unknown-model-id");
    refused(
        &f.invoke(&["show", "sensitive-argument-marker", "--json"]),
        "unknown-model-id",
    );
    for id in ["x".repeat(97), "not-ascii-雪".into(), "".into()] {
        refused(
            &f.invoke(&["show", &id, "--json"]),
            "invalid-models-arguments",
        );
    }
    refused(&f.invoke(&["show", "absent"]), "unknown-model-id");
    refused(
        &invoke(&["models".into(), "list".into(), "--json".into()]),
        "invalid-models-arguments",
    );
    refused(
        &invoke(&[
            "models".into(),
            "--catalog".into(),
            "relative-sensitive-path-marker".into(),
            "list".into(),
            "--json".into(),
        ]),
        "invalid-models-arguments",
    );
    let mut args = f.args(&["list", "--json"]);
    args.push(OsString::from_vec(vec![255]));
    let out = invoke(&args);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, b"m8s: invalid arguments\n");
    let out = invoke(&["--help".into(), OsString::from_vec(vec![255])]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, b"m8s: invalid arguments\n");
}
#[cfg(unix)]
#[test]
fn unsafe_missing_and_oversized_files_refuse_without_mutation_or_blocking() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let parent = f.dir.path().canonicalize().unwrap();
    let missing = parent.join("sensitive-path-marker-missing");
    let link = parent.join("sensitive-path-marker-link");
    symlink(&f.catalog, &link).unwrap();
    let dangling = parent.join("sensitive-path-marker-dangling");
    symlink(&missing, &dangling).unwrap();
    let fifo = parent.join("sensitive-path-marker-fifo");
    nix::unistd::mkfifo(
        &fifo,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    let oversized = parent.join("sensitive-path-marker-oversized");
    fs::write(&oversized, vec![b'x'; 65537]).unwrap();
    for path in [&missing, &link, &dangling, &fifo, &oversized, &parent] {
        refused(
            &invoke(&[
                "models".into(),
                "--catalog".into(),
                path.clone().into(),
                "list".into(),
                "--json".into(),
            ]),
            "invalid-models-catalog",
        );
    }
    assert!(!missing.exists());
    assert_eq!(
        fs::read(&f.catalog).unwrap(),
        m8s_models::EXAMPLE_CATALOG.as_bytes()
    );
    assert_eq!(fs::read(&oversized).unwrap(), vec![b'x'; 65537]);
}
#[cfg(not(unix))]
#[test]
fn catalog_loader_refuses_unsupported_platform() {
    let f = Fixture::new();
    refused(&f.invoke(&["list", "--json"]), "invalid-models-catalog");
}
