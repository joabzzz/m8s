use m8s_config::{ColdBehavior, Config, Error, MAX_BYTES, ValidationLimits};
use m8s_models::{ModelCatalog, ModelProfile};
fn catalog() -> ModelCatalog {
    ModelCatalog {
        models: vec![profile("example-27b-q4km"), profile("example-70b-q4km")],
    }
}
fn profile(id: &str) -> ModelProfile {
    ModelProfile {
        id: id.into(),
        source: "example://explicit-owner-selection".into(),
        params_billions: 27,
        quant: "Q4_K_M".into(),
        weights_bytes: 17_000_000_000,
        runtime_overhead_bytes_per_replica: 2_000_000_000,
        kv_bytes_per_1024_tokens: 64_000_000,
        max_context_tokens: 131072,
        license: "owner-must-verify".into(),
        license_class: "unknown".into(),
        runtime_status: "untested".into(),
    }
}
const MINIMAL: &str = "schema = 1\n[tier.small]\nmodel = 'example-27b-q4km'\n";
fn parse(text: &str) -> Result<Config, Error> {
    Config::parse(text.as_bytes(), &catalog(), &ValidationLimits::default())
}
fn bad(text: &str) {
    assert!(
        parse(text).is_err(),
        "malformed synthetic config unexpectedly accepted"
    );
}
#[test]
fn example_and_defaults_roundtrip_without_effects() {
    for text in [MINIMAL, m8s_config::EXAMPLE] {
        let config = parse(text).unwrap();
        assert_eq!(config.schema(), 1);
        assert_eq!(config.budget().monthly_micro_usd, 20_000_000);
        assert_eq!(config.dms().ttl_seconds, 3600);
        assert_eq!(config.dms().max_lifetime_seconds, 43200);
        assert_eq!(config.minimum_down_mbps(), 1000);
        assert!(config.signed_image_required() && config.unknown_location_rejected());
        let output = config.to_toml().unwrap();
        assert!(config == parse(&output).unwrap());
        assert_eq!(output, parse(&output).unwrap().to_toml().unwrap());
    }
    let config = parse(MINIMAL).unwrap();
    let tier = config.tier("small").unwrap();
    assert_eq!(tier.replicas().min, 0);
    assert_eq!(tier.replicas().max, 1);
    assert!(!tier.interruptible());
    assert!(tier.manual());
    assert_eq!(
        tier.cold(),
        ColdBehavior::Wait {
            timeout_seconds: 900
        }
    );
    assert!(config.timezone().is_none());
    let config = parse(m8s_config::EXAMPLE).unwrap();
    assert_eq!(config.timezone(), Some("America/New_York"));
    assert_eq!(
        config.tier("large").unwrap().cold(),
        ColdBehavior::Fallback { tier: "small" }
    );
    assert_eq!(
        config
            .tier("small")
            .unwrap()
            .queue()
            .unwrap()
            .scale_up_for_seconds(),
        300
    );
}
#[test]
fn schema_duplicate_unknown_nested_and_type_errors_fail_closed() {
    for text in [
        "",
        "schema=2\n[tier.small]\nmodel='example-27b-q4km'",
        "[tier.small]\nmodel='example-27b-q4km'",
        "schema=true",
        "schema=1.0",
        "schema=1\nschema=1",
        "schema=1\nunknown='synthetic'",
        "schema=1\n[tier]",
        "schema=1\n[tier.small]\nmodel='example-27b-q4km'\nmodel='example-70b-q4km'",
        "schema=1\n[tier.small]\nmodel='example-27b-q4km'\nunknown=1",
    ] {
        bad(text);
    }
    for tail in [
        "replicas={min=0,max=1,extra=1}",
        "gpu={count=1,extra=1}",
        "cold={mode='reject',extra=1}",
        "queue={queue_wait_ms_gt=1,scale_up_for='1s',scale_down_idle='1s',extra=1}",
        "replicas={min=true,max=1}",
        "provider='vast-secure'",
        "interruptible='false'",
        "manual=1",
    ] {
        bad(&format!("{MINIMAL}{tail}\n"));
    }
    for prefix in [
        "[owner]\ntimezone='UTC'\nextra=1\n",
        "[budget]\nmonthly_usd=20.0\n",
        "[dead_mans_switch]\nextra=1\n",
        "[policy]\nsigned_images_only=false\n",
    ] {
        bad(&format!(
            "schema=1\n{prefix}[tier.small]\nmodel='example-27b-q4km'"
        ));
    }
}
#[test]
fn canonical_money_is_integer_only_and_envelope_is_injected() {
    for value in ["20.0", "'20'", "-1", "true", "nan", "inf", "20000001"] {
        bad(&format!(
            "schema=1\n[budget]\nmonthly_micro_usd={value}\n[tier.small]\nmodel='example-27b-q4km'"
        ));
    }
    let text =
        "schema=1\n[budget]\nmonthly_micro_usd=30000000\n[tier.small]\nmodel='example-27b-q4km'";
    bad(text);
    // Explicit trusted envelope allows different owner policy, never spend approval.
    let limits = ValidationLimits {
        maximum_monthly_micro_usd: 30_000_000,
        ..Default::default()
    };
    assert!(Config::parse(text.as_bytes(), &catalog(), &limits).is_ok());
    let limits = ValidationLimits {
        maximum_monthly_micro_usd: 2_000_000,
        ..Default::default()
    };
    assert!(Config::parse(MINIMAL.as_bytes(), &catalog(), &limits).is_err());
    for budget in [
        "max_total_hourly_micro_usd=3000001",
        "max_instance_hourly_micro_usd=3000001",
        "max_instance_hourly_micro_usd=2000000\nmax_total_hourly_micro_usd=1000000",
    ] {
        bad(&format!(
            "schema=1\n[budget]\n{budget}\n[tier.small]\nmodel='example-27b-q4km'"
        ));
    }
}
#[test]
fn identifiers_models_and_known_provider_preferences() {
    for model in ["missing", "${MODEL}", "../escape", "a/b", "UPPER", "a b"] {
        bad(&format!("schema=1\n[tier.small]\nmodel='{model}'"));
    }
    for tier in ["../escape", "UPPER", "a b", "${TIER}", "a/b"] {
        bad(&format!(
            "schema=1\n[tier.'{tier}']\nmodel='example-27b-q4km'"
        ));
    }
    for providers in [
        "[]",
        "['unknown']",
        "['vast-secure','vast-secure']",
        "['vast-secure','vast-community','vast-secure']",
    ] {
        bad(&format!("{MINIMAL}provider={providers}"));
    }
    assert!(
        parse(&format!(
            "{MINIMAL}provider=['vast-community','vast-secure']"
        ))
        .is_ok()
    );
}
#[test]
fn replica_gpu_bandwidth_and_policy_bounds() {
    for tail in [
        "replicas={min=2,max=1}",
        "replicas={min=0,max=17}",
        "replicas={min=-1,max=1}",
        "gpu={count=0}",
        "gpu={count=17}",
        "gpu={min_vram_bytes=0}",
        "gpu={min_vram_bytes=1000000000001}",
    ] {
        bad(&format!("{MINIMAL}{tail}"));
    }
    for value in [0, 999, 1_000_001] {
        bad(&format!(
            "schema=1\n[policy]\nminimum_down_mbps={value}\n[tier.small]\nmodel='example-27b-q4km'"
        ));
    }
    for (min, max) in [(0, 0), (0, 16), (16, 16)] {
        assert!(parse(&format!("{MINIMAL}replicas={{min={min},max={max}}}")).is_ok());
    }
}
#[test]
fn cold_modes_exhaustive_fields_references_and_cycles() {
    assert_eq!(
        parse(&format!("{MINIMAL}cold={{mode='reject'}}"))
            .unwrap()
            .tier("small")
            .unwrap()
            .cold(),
        ColdBehavior::Reject
    );
    for cold in [
        "{mode='unknown'}",
        "{mode='wait'}",
        "{mode='wait',timeout='0s'}",
        "{mode='reject',timeout='1s'}",
        "{mode='reject',tier='small'}",
        "{mode='fallback'}",
        "{mode='fallback',tier='missing'}",
        "{mode='fallback',tier='small'}",
        "{mode='fallback',tier='small',timeout='1s'}",
    ] {
        bad(&format!("{MINIMAL}cold={cold}"));
    }
    bad(
        "schema=1\n[tier.one]\nmodel='example-27b-q4km'\ncold={mode='fallback',tier='two'}\n[tier.two]\nmodel='example-70b-q4km'\ncold={mode='fallback',tier='one'}",
    );
}
#[test]
fn duration_units_unicode_overflow_and_dms_order() {
    for (s, want) in [("1s", 1), ("15m", 900), ("1h", 3600), ("31d", 2678400)] {
        assert_eq!(m8s_config::duration(s).unwrap(), want);
    }
    for s in [
        "off",
        "0s",
        "01s",
        "1",
        "1ms",
        "1.5h",
        "1h30m",
        "-1s",
        "+1s",
        "32d",
        "18446744073709551615h",
        "€",
        "éx",
        "1秒",
    ] {
        assert!(m8s_config::duration(s).is_err());
        bad(&format!(
            "schema=1\n[dead_mans_switch]\nttl='{s}'\n[tier.small]\nmodel='example-27b-q4km'"
        ));
    }
    bad(
        "schema=1\n[dead_mans_switch]\nttl='2h'\nmax_lifetime='1h'\n[tier.small]\nmodel='example-27b-q4km'",
    );
}
#[test]
fn exact_decimal_helpers_use_conservative_rounding_without_float() {
    for (text, down, up) in [
        ("0", 0, 0),
        ("20", 20_000_000, 20_000_000),
        ("0.0000001", 0, 1),
        ("1.9999999", 1_999_999, 2_000_000),
        ("18446744073709.551615", u64::MAX, u64::MAX),
    ] {
        assert_eq!(m8s_config::usd_limit_to_micro_usd(text).unwrap(), down);
        assert_eq!(m8s_config::usd_cost_to_micro_usd(text).unwrap(), up);
    }
    for s in [
        "",
        "-1",
        "+1",
        "nan",
        "inf",
        "1e3",
        "1_000",
        "01",
        ".1",
        "1.",
        "1.0.0",
        " 1",
        "1 ",
        "18446744073709.551616",
        "0.0000000000000000001",
    ] {
        assert!(m8s_config::usd_cost_to_micro_usd(s).is_err());
        assert!(m8s_config::usd_limit_to_micro_usd(s).is_err());
    }
    assert_eq!(
        m8s_config::usd_limit_to_micro_usd("18446744073709.5516159").unwrap(),
        u64::MAX
    );
    assert!(m8s_config::usd_cost_to_micro_usd("18446744073709.5516159").is_err());
}
#[test]
fn schedule_requires_explicit_known_timezone_and_rejects_bad_cron() {
    let schedule = "[[tier.small.schedule]]\nopen='0 8 * * 1-5'\nclose='0 18 * * 1-5'\n";
    bad(&format!("{MINIMAL}{schedule}"));
    for tz in ["local", "${TZ}", "Europe/NotAZone", "+02:00", ""] {
        bad(&format!(
            "schema=1\n[owner]\ntimezone='{tz}'\n[tier.small]\nmodel='example-27b-q4km'\n{schedule}"
        ));
    }
    for cron in [
        "@daily",
        "0 8 * * MON-FRI",
        "60 8 * * 1-5",
        "0 24 * * 1-5",
        "0 8 0 * *",
        "0 8 * 13 *",
        "0 8 * * 7",
        "*/0 8 * * *",
        "0 8 * * 5-1",
        "0 8 * * *;bad",
        "0  8 * * *",
        "0 8 * * * *",
        "5/3 8 * * *",
    ] {
        bad(&format!(
            "schema=1\n[owner]\ntimezone='UTC'\n[tier.small]\nmodel='example-27b-q4km'\n[[tier.small.schedule]]\nopen='{cron}'\nclose='0 18 * * 1-5'"
        ));
    }
    let valid = "schema=1\n[owner]\ntimezone='UTC'\n[tier.small]\nmodel='example-27b-q4km'\n[[tier.small.schedule]]\nopen='*/15 8-12/2 1,15 * 0-6'\nclose='0 18 * * 1-5'";
    assert!(parse(valid).is_ok());
}
#[test]
fn queue_typed_thresholds_and_positive_durations_only() {
    for queue in [
        "{scale_up_for='5m',scale_down_idle='30m'}",
        "{queue_wait_ms_gt=0,scale_up_for='5m',scale_down_idle='30m'}",
        "{inflight_gt=1000001,scale_up_for='5m',scale_down_idle='30m'}",
        "{inflight_gt=1,scale_up_for='0s',scale_down_idle='30m'}",
        "{scale_up_when='queue_wait_ms > 2 for 5m',scale_down_when='inflight == 0'}",
    ] {
        bad(&format!("{MINIMAL}queue={queue}"));
    }
}
#[test]
fn catalog_duplicates_bounds_status_and_numeric_validation() {
    let mut values = vec![ModelCatalog { models: vec![] }];
    let mut dup = catalog();
    dup.models.push(dup.models[0].clone());
    values.push(dup);
    for field in 0..7 {
        let mut c = catalog();
        match field {
            0 => c.models[0].weights_bytes = 0,
            1 => c.models[0].kv_bytes_per_1024_tokens = 0,
            2 => c.models[0].max_context_tokens = 0,
            3 => c.models[0].runtime_status = "invented".into(),
            4 => c.models[0].license_class = "invented".into(),
            5 => c.models[0].params_billions = 0,
            _ => c.models[0].id = "../escape".into(),
        }
        values.push(c);
    }
    for c in values {
        assert!(matches!(
            Config::parse(MINIMAL.as_bytes(), &c, &ValidationLimits::default()),
            Err(Error::Catalog)
        ));
    }
    // 'certified' is permitted descriptive metadata, never converted into Ready evidence.
    let mut c = catalog();
    c.models[0].runtime_status = "certified".into();
    assert!(Config::parse(MINIMAL.as_bytes(), &c, &ValidationLimits::default()).is_ok());
}
#[test]
fn bounded_bytes_tiers_schedules_and_error_redaction() {
    assert!(matches!(
        Config::parse(
            &vec![b' '; MAX_BYTES + 1],
            &catalog(),
            &ValidationLimits::default()
        ),
        Err(Error::TooLarge)
    ));
    assert!(Config::parse(&[0xff], &catalog(), &ValidationLimits::default()).is_err());
    let mut text = "schema=1\n".to_string();
    for i in 0..33 {
        text.push_str(&format!("[tier.t{i}]\nmodel='example-27b-q4km'\n"));
    }
    bad(&text);
    let mut text =
        "schema=1\n[owner]\ntimezone='UTC'\n[tier.small]\nmodel='example-27b-q4km'\n".to_string();
    for _ in 0..17 {
        text.push_str("[[tier.small.schedule]]\nopen='0 8 * * *'\nclose='0 18 * * *'\n");
    }
    bad(&text);
    let error = match parse("schema=1\napi_key='SYNTHETIC-DO-NOT-ECHO'") {
        Ok(_) => panic!("unexpected acceptance"),
        Err(e) => e,
    };
    assert!(!format!("{error:?} {error}").contains("SYNTHETIC"));
    assert!(std::error::Error::source(&error).is_none());
}
#[test]
fn bounded_explicit_regular_file_only_no_discovery() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("supplied.toml");
    std::fs::write(&path, MINIMAL).unwrap();
    let config = Config::from_path(&path, &catalog(), &ValidationLimits::default()).unwrap();
    assert!(config == parse(MINIMAL).unwrap());
    assert!(Config::from_path(directory.path(), &catalog(), &ValidationLimits::default()).is_err());
    assert!(
        Config::from_path(
            &directory.path().join("missing"),
            &catalog(),
            &ValidationLimits::default()
        )
        .is_err()
    );
    #[cfg(unix)]
    {
        let link = directory.path().join("symlink.toml");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Config::from_path(&link, &catalog(), &ValidationLimits::default()).is_err());
    }
    std::fs::write(&path, vec![b' '; MAX_BYTES + 1]).unwrap();
    assert!(matches!(
        Config::from_path(&path, &catalog(), &ValidationLimits::default()),
        Err(Error::TooLarge)
    ));
}

#[test]
fn bounded_catalog_count_deep_type_errors_and_long_strings() {
    let mut c = catalog();
    for i in 0..256 {
        c.models.push(profile(&format!("model{i}")));
    }
    assert!(matches!(
        Config::parse(MINIMAL.as_bytes(), &c, &ValidationLimits::default()),
        Err(Error::Catalog)
    ));
    let deep = format!("schema=1\nunknown={}0{}", "[".repeat(200), "]".repeat(200));
    bad(&deep);
    bad(&format!(
        "schema=1\n[tier.'{}']\nmodel='example-27b-q4km'",
        "a".repeat(49)
    ));
    let mut c = catalog();
    c.models[0].weights_bytes = u64::MAX;
    c.models[0].runtime_overhead_bytes_per_replica = 1;
    assert!(matches!(
        Config::parse(MINIMAL.as_bytes(), &c, &ValidationLimits::default()),
        Err(Error::Catalog)
    ));
}
#[cfg(unix)]
#[test]
fn supplied_fifo_is_refused_without_blocking_open() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("synthetic.fifo");
    let path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // Synthetic local fixture only; no process/provider/credential interaction.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(matches!(
        Config::from_path(&fifo, &catalog(), &ValidationLimits::default()),
        Err(Error::UnsafePath)
    ));
}
