//! Actual binary, offline explicit capacities and shipped/synthetic catalogs only.
#![cfg(unix)]
use m8s_models::{Layout, ModelCatalog, PlanRequest};
use serde_json::Value;
use std::{
    ffi::OsString,
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
};
struct Fixture {
    _dir: tempfile::TempDir,
    catalog: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("sensitive-catalog-path");
        fs::write(&catalog, m8s_models::EXAMPLE_CATALOG).unwrap();
        Self { _dir: dir, catalog }
    }
    fn args(&self, layout: &str) -> Vec<OsString> {
        let mut args: Vec<OsString> = ["models", "fit", "--catalog"].map(OsString::from).into();
        args.push(self.catalog.clone().into());
        args.extend(
            [
                "--model",
                "example-27b-q4km",
                "--gpu-count",
                "2",
                "--vram-bytes-per-gpu",
                "80000000000",
                "--layout",
                layout,
                "--context-tokens",
                "1024",
                "--minimum-sequences",
                "1",
                "--reserved-bytes-per-gpu",
                "0",
                "--json",
            ]
            .map(OsString::from),
        );
        args
    }
    fn run(&self, args: &[OsString]) -> Output {
        let before = fs::read(&self.catalog).unwrap();
        let out = invoke(args);
        assert_eq!(fs::read(&self.catalog).unwrap(), before);
        out
    }
    fn write(&self, catalog: &ModelCatalog) {
        fs::write(&self.catalog, toml::to_string(catalog).unwrap()).unwrap();
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
fn change(args: &mut [OsString], flag: &str, value: &str) {
    let index = args.iter().position(|arg| arg == flag).unwrap();
    args[index + 1] = value.into();
}
fn success(out: &Output) -> Value {
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty());
    assert!(out.stdout.len() <= 16384);
    serde_json::from_slice(&out.stdout).unwrap()
}
fn refused(out: &Output, error: &str) {
    assert!(!out.status.success());
    assert_eq!(out.stderr, format!("m8s: {error}\n").as_bytes());
    if !out.stdout.is_empty() {
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["schema"], "m8s.model-fit/v1");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], error);
    }
    for bytes in [&out.stdout, &out.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains("sensitive-"));
    }
}
fn catalog() -> ModelCatalog {
    toml::from_str(m8s_models::EXAMPLE_CATALOG).unwrap()
}
#[test]
fn each_layout_cross_checks_actual_library_and_explicit_projection() {
    let f = Fixture::new();
    for (layout, typed) in [("a", Layout::A), ("b", Layout::B), ("c", Layout::C)] {
        let args = f.args(layout);
        let out = f.run(&args);
        let v = success(&out);
        assert_eq!(out.stdout, f.run(&args).stdout);
        let capacity = m8s_core::Offer {
            id: "SYNTHETIC-TEST-CAPACITY-NOT-A-LIVE-OFFER".into(),
            country: None,
            region: None,
            inet_down_mbps: 0,
            gpu_count: 2,
            vram_bytes_per_gpu: 80_000_000_000,
            hourly_micro_usd: 0,
            interruptible: false,
            reliability_bps: 0,
        };
        let fit = m8s_models::plan(
            &catalog().models[0],
            &capacity,
            PlanRequest {
                layout: typed,
                context_tokens: 1024,
                minimum_sequences_per_replica: 1,
                reserved_bytes_per_gpu: 0,
            },
        )
        .unwrap();
        for (field, expected) in [
            ("replicas", fit.replicas),
            ("gpus_per_replica", u64::from(fit.gpus_per_replica)),
            (
                "weights_and_overhead_bytes_per_replica",
                fit.weights_and_overhead_bytes_per_replica,
            ),
            ("kv_bytes_per_sequence", fit.kv_bytes_per_sequence),
            (
                "kv_headroom_bytes_per_replica",
                fit.kv_headroom_bytes_per_replica,
            ),
            (
                "concurrent_sequences_per_replica",
                fit.concurrent_sequences_per_replica,
            ),
            ("total_concurrent_sequences", fit.total_concurrent_sequences),
        ] {
            assert_eq!(v["fit"][field], expected);
        }
        assert_eq!(v["fit"]["layout"], layout);
        assert_eq!(
            v["sizing"],
            serde_json::json!({"gpu_count":2,"vram_bytes_per_gpu":80_000_000_000u64,"layout":layout,"context_tokens":1024,"minimum_sequences":1,"reserved_bytes_per_gpu":0})
        );
        assert_eq!(v["model"], "example-27b-q4km");
        for (key, expected) in [
            ("status", "capacity-estimate-only"),
            ("model_evidence", "model-unmeasured"),
            ("runtime", "runtime-not-verified"),
            ("provider_policy", "provider-policy-not-assessed"),
            ("authorization", "not-authorized"),
            ("execution", "not-executable"),
        ] {
            assert_eq!(v[key], expected);
        }
        for field in [
            "offer",
            "source",
            "price",
            "country",
            "throughput",
            "launch_fingerprint",
        ] {
            assert!(v.get(field).is_none());
        }
        let mut text = args.clone();
        text.pop();
        let text = f.run(&text);
        assert!(text.status.success());
        let text = String::from_utf8(text.stdout).unwrap();
        let (header, projection) = text.split_once('\n').unwrap();
        assert!(header.contains("NOT authorized or executable"));
        assert_eq!(serde_json::from_str::<Value>(projection).unwrap(), v);
        let mut reordered = vec!["models".into(), "fit".into(), "--json".into()];
        for pair in args[2..args.len() - 1].chunks_exact(2).rev() {
            reordered.extend_from_slice(pair);
        }
        assert_eq!(success(&f.run(&reordered)), v);
    }
}
#[test]
fn explicit_flags_decimal_bounds_utf8_and_help() {
    use std::os::unix::ffi::OsStringExt;
    for args in [
        vec!["models".into(), "fit".into(), "--help".into()],
        vec!["--help".into()],
    ] {
        let out = invoke(&args);
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        assert!(
            String::from_utf8(out.stdout)
                .unwrap()
                .contains("models fit")
        );
    }
    let f = Fixture::new();
    let good = f.args("a");
    for index in (2..good.len() - 1).step_by(2) {
        let mut args = good.clone();
        args.drain(index..index + 2);
        refused(&f.run(&args), "invalid-model-fit-arguments");
        let mut args = good.clone();
        args.extend_from_slice(&good[index..index + 2]);
        refused(&f.run(&args), "invalid-model-fit-arguments");
    }
    for extra in [
        vec!["--json"],
        vec!["--help"],
        vec!["--socket", "sensitive-argument"],
        vec!["--execute"],
        vec!["sensitive-argument"],
    ] {
        let mut args = good.clone();
        args.extend(extra.into_iter().map(OsString::from));
        refused(&f.run(&args), "invalid-model-fit-arguments");
    }
    for flag in [
        "--gpu-count",
        "--vram-bytes-per-gpu",
        "--context-tokens",
        "--minimum-sequences",
        "--reserved-bytes-per-gpu",
    ] {
        for raw in [
            "",
            "+1",
            "-1",
            " 1",
            "1 ",
            "1e3",
            "1.0",
            "١",
            "18446744073709551616",
            "000000000000000000000",
        ] {
            let mut args = good.clone();
            change(&mut args, flag, raw);
            refused(&f.run(&args), "invalid-model-fit-arguments");
        }
        if flag != "--reserved-bytes-per-gpu" {
            let mut args = good.clone();
            change(&mut args, flag, "0");
            refused(&f.run(&args), "invalid-model-fit-arguments");
        }
    }
    for (flag, value) in [
        ("--gpu-count", "4294967296"),
        ("--layout", "A"),
        ("--layout", "d"),
        ("--model", "Invalid"),
        ("--catalog", "relative-sensitive-path"),
    ] {
        let mut args = good.clone();
        change(&mut args, flag, value);
        refused(&f.run(&args), "invalid-model-fit-arguments");
    }
    let mut args = good.clone();
    change(&mut args, "--model", &"x".repeat(97));
    refused(&f.run(&args), "invalid-model-fit-arguments");
    let mut args = good.clone();
    args.push(OsString::from_vec(vec![255]));
    let out = f.run(&args);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, b"m8s: invalid arguments\n");
}
#[test]
fn arithmetic_refusals_and_maximum_capacity_types_are_honest() {
    let f = Fixture::new();
    for (flag, value, error) in [
        ("--vram-bytes-per-gpu", "1", "model-does-not-fit"),
        (
            "--reserved-bytes-per-gpu",
            "80000000000",
            "model-does-not-fit",
        ),
        (
            "--reserved-bytes-per-gpu",
            "18446744073709551615",
            "model-does-not-fit",
        ),
        ("--context-tokens", "131073", "invalid-model-fit-arguments"),
        (
            "--minimum-sequences",
            "18446744073709551615",
            "model-does-not-fit",
        ),
    ] {
        let mut args = f.args("b");
        change(&mut args, flag, value);
        refused(&f.run(&args), error);
    }
    let mut args = f.args("b");
    change(&mut args, "--gpu-count", "1");
    change(&mut args, "--vram-bytes-per-gpu", "18446744073709551615");
    assert_eq!(
        success(&f.run(&args))["sizing"]["vram_bytes_per_gpu"],
        u64::MAX
    );
    let mut args = f.args("c");
    change(&mut args, "--gpu-count", "4294967295");
    change(&mut args, "--vram-bytes-per-gpu", "18446744073709551615");
    refused(&f.run(&args), "model-fit-overflow");
    let mut c = catalog();
    c.models[0].weights_bytes = 1;
    c.models[0].runtime_overhead_bytes_per_replica = 0;
    c.models[0].kv_bytes_per_1024_tokens = 1;
    f.write(&c);
    let mut args = f.args("b");
    change(&mut args, "--gpu-count", "4294967295");
    change(&mut args, "--vram-bytes-per-gpu", "2");
    assert_eq!(success(&f.run(&args))["fit"]["replicas"], u32::MAX);
    c.models[0].kv_bytes_per_1024_tokens = u64::MAX;
    c.models[0].max_context_tokens = u64::MAX;
    f.write(&c);
    let mut args = f.args("b");
    change(&mut args, "--context-tokens", "18446744073709551615");
    refused(&f.run(&args), "model-fit-overflow");
}
#[test]
fn full_catalog_precedes_lookup_and_free_text_never_leaks() {
    let f = Fixture::new();
    let args = f.args("a");
    let baseline = success(&f.run(&args));
    let mut c = catalog();
    for m in &mut c.models {
        m.source = "https://sensitive-source.invalid".into();
        m.license = "sensitive-license\u{1b}".into();
        m.quant = "sensitive-quant\n".into();
        m.runtime_status = "certified".into();
    }
    f.write(&c);
    assert_eq!(success(&f.run(&args)), baseline);
    let mut absent = args.clone();
    change(&mut absent, "--model", "sensitive-id-absent");
    refused(&f.run(&absent), "unknown-model-fit-id");
    let mut unused = c.models[0].clone();
    unused.id = "unused".into();
    unused.weights_bytes = 0;
    c.models.push(unused);
    f.write(&c);
    for args in [&args, &absent] {
        refused(&f.run(args), "invalid-model-fit-catalog");
    }
    fs::write(&f.catalog, "sensitive-catalog-parser=[invalid").unwrap();
    refused(&f.run(&args), "invalid-model-fit-catalog");
    let mut missing = args.clone();
    let path = f.catalog.with_file_name("sensitive-missing");
    change(&mut missing, "--catalog", path.to_str().unwrap());
    refused(&f.run(&missing), "invalid-model-fit-catalog");
    assert!(!path.exists());
}
