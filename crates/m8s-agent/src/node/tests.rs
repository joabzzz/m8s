//! Synthetic file/command contract tests, NOT authentication or real inference.
use super::*;
use m8s_bootstrap::{BundleSettings, ControlAcl};
use std::{
    cell::Cell,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        #[cfg(target_os = "macos")]
        let base = Path::new("/private/tmp");
        #[cfg(not(target_os = "macos"))]
        let base = Path::new("/tmp");
        let root = base.join(format!(
            "m8s-node-material-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        for (name, bytes, mode) in [
            ("mesh", b"#!/bin/sh\nexit 0\n".as_slice(), 0o700),
            ("model.gguf", b"synthetic-not-a-model".as_slice(), 0o600),
            ("key", b"synthetic-not-owner-key".as_slice(), 0o600),
        ] {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(mode)
                .open(root.join(name))
                .unwrap();
            f.write_all(bytes).unwrap();
        }
        Self(root.canonicalize().unwrap())
    }
    fn launch(&self) -> MeshLaunch {
        MeshLaunch {
            program: self.0.join("mesh"),
            gguf: Some(self.0.join("model.gguf")),
            node_key: self.0.join("key"),
            owner: EndpointId([1; 32]),
            node: EndpointId([2; 32]),
            mesh_owner_id: "1".repeat(64),
            model: "synthetic-model".into(),
            heartbeat_ttl_seconds: 10,
            port: 9337,
            console_port: 3131,
            model_pin: None,
        }
    }
    fn root(&self) -> PathBuf {
        self.0.join("fresh-node")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Checker {
    calls: Cell<usize>,
    refuse: Option<usize>,
}
impl Checker {
    fn accept() -> Self {
        Self {
            calls: Cell::new(0),
            refuse: None,
        }
    }
}
impl CertificateVerifier for Checker {
    fn verify(&self, _: &NodeCertificate, _: &CertificateClaims) -> m8s_bootstrap::Result<()> {
        let n = self.calls.get() + 1;
        self.calls.set(n);
        if self.refuse == Some(n) {
            Err(m8s_bootstrap::Error::Certificate)
        } else {
            Ok(())
        }
    }
}
fn bundle(config: &[u8]) -> ReceivedBundle {
    ReceivedBundle {
        claims: CertificateClaims {
            owner: EndpointId([1; 32]),
            node: EndpointId([2; 32]),
            issued_at: 10,
            expires_at: 100,
        },
        certificate: NodeCertificate(b"synthetic-not-a-certificate".to_vec()),
        join_token: b"synthetic-join-material".to_vec().into(),
        acl: ControlAcl::owner_only(EndpointId([1; 32])),
        settings: BundleSettings {
            model: "synthetic-model".into(),
            config: config.to_vec().into(),
            heartbeat_ttl_seconds: 10,
        },
    }
}
#[test]
fn exact_private_files_and_enforcing_native_command_never_expose_secret_bytes() {
    let f = Fixture::new();
    let check = Checker::accept();
    let material =
        NodeMaterial::stage_with(&f.root(), bundle(b""), f.launch(), &check, 20).unwrap();
    assert_eq!(check.calls.get(), 2);
    assert_eq!(fs::metadata(f.root()).unwrap().mode() & 0o7777, 0o700);
    for (name, bytes) in [
        ("join-token", b"synthetic-join-material".as_slice()),
        (
            "node-ownership.json",
            b"synthetic-not-a-certificate".as_slice(),
        ),
        ("runtime.toml", b"".as_slice()),
    ] {
        let p = f.root().join(name);
        assert_eq!(fs::read(&p).unwrap(), bytes);
        let m = fs::metadata(p).unwrap();
        assert_eq!(m.mode() & 0o7777, 0o600);
        assert_eq!(m.nlink(), 1);
    }
    let command = material.command().unwrap();
    assert_eq!(command.get_program(), f.0.join("mesh").as_os_str());
    let args: Vec<_> = command
        .get_args()
        .map(|s| s.to_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        args,
        vec![
            "serve".into(),
            "--gguf".into(),
            f.0.join("model.gguf").display().to_string(),
            "--join-file".into(),
            f.root().join("join-token").display().to_string(),
            "--node-ownership".into(),
            f.root().join("node-ownership.json").display().to_string(),
            "--node-ownership-required".into(),
            "--config".into(),
            f.root().join("runtime.toml").display().to_string(),
            "--trust-policy".into(),
            "allowlist".into(),
            "--trust-owner".into(),
            "1".repeat(64),
            "--headless".into(),
            "--log-format".into(),
            "json".into(),
            "--no-default-plugins".into(),
            "--port".into(),
            "9337".into(),
            "--console".into(),
            "3131".into()
        ]
    );
    for forbidden in [
        "--owner-key",
        "--owner-required",
        "--local-model-only",
        "--auto",
        "--client",
        "--publish",
        "--listen-all",
        "synthetic-join-material",
        "synthetic-not-owner-key",
    ] {
        assert!(!args.iter().any(|a| a == forbidden));
    }
    let env: Vec<_> = command.get_envs().collect();
    assert_eq!(env.len(), 6);
    assert!(
        env.iter()
            .any(|(k, v)| *k == "HOME" && *v == Some(f.root().as_os_str()))
    );
    assert!(env.iter().any(|(k,v)|*k=="MESH_LLM_NODE_KEY_PATH" && *v==Some(f.0.join("key").as_os_str())));
    assert!(
        env.iter()
            .any(|(k, v)| *k == "MESH_LLM_ANALYTICS" && *v == Some(std::ffi::OsStr::new("0")))
    );
    assert!(
        env.iter()
            .any(|(k, v)| *k == "DO_NOT_TRACK" && *v == Some(std::ffi::OsStr::new("1")))
    );
    material.recheck_with(&check, 99).unwrap();
    assert!(material.recheck_with(&check, 100).is_err());
    // No process is launched by staging/command construction or this test.
}
#[test]
fn binding_policy_and_certificate_refusal_happen_before_creating_staging_directory() {
    for field in 0..7 {
        let f = Fixture::new();
        let mut launch = f.launch();
        let mut b = bundle(b"");
        match field {
            0 => launch.node = EndpointId([9; 32]),
            1 => launch.owner = EndpointId([9; 32]),
            2 => launch.model = "other".into(),
            3 => launch.heartbeat_ttl_seconds = 11,
            4 => b.claims.expires_at = 20,
            5 => b.join_token = vec![0].into(),
            _ => launch.mesh_owner_id = "private\n".into(),
        };
        let check = Checker::accept();
        assert!(NodeMaterial::stage_with(&f.root(), b, launch, &check, 20).is_err());
        assert!(!f.root().exists());
        assert_eq!(check.calls.get(), 0);
    }
    let f = Fixture::new();
    let check = Checker {
        calls: Cell::new(0),
        refuse: Some(1),
    };
    assert!(matches!(
        NodeMaterial::stage_with(&f.root(), bundle(b""), f.launch(), &check, 20),
        Err(Error::Certificate)
    ));
    assert!(!f.root().exists());
}
#[test]
fn existing_alias_hardlink_and_replaced_anchor_refuse_without_overwrite_or_repair() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::DirBuilder::new().mode(0o700).create(f.root()).unwrap();
    fs::write(f.root().join("sentinel"), b"unchanged").unwrap();
    assert!(
        NodeMaterial::stage_with(&f.root(), bundle(b""), f.launch(), &Checker::accept(), 20)
            .is_err()
    );
    assert_eq!(fs::read(f.root().join("sentinel")).unwrap(), b"unchanged");
    for hardlink in [false, true] {
        let f = Fixture::new();
        let mut l = f.launch();
        if hardlink {
            fs::hard_link(&l.node_key, f.0.join("alias")).unwrap();
        } else {
            symlink(l.gguf.as_ref().unwrap(), f.0.join("alias")).unwrap();
            l.gguf = Some(f.0.join("alias"));
        }
        assert!(
            NodeMaterial::stage_with(&f.root(), bundle(b""), l, &Checker::accept(), 20).is_err()
        );
        assert!(!f.root().exists());
    }
    let f = Fixture::new();
    let c = Checker::accept();
    let material = NodeMaterial::stage_with(&f.root(), bundle(b""), f.launch(), &c, 20).unwrap();
    fs::rename(f.root().join("join-token"), f.0.join("retained-join")).unwrap();
    fs::write(f.root().join("join-token"), b"replacement").unwrap();
    assert!(material.recheck_with(&c, 21).is_err());
    assert_eq!(
        fs::read(f.0.join("retained-join")).unwrap(),
        b"synthetic-join-material"
    );
}
#[test]
fn late_certificate_failure_retains_private_artifacts_and_opaque_config_cannot_launch() {
    let f = Fixture::new();
    let check = Checker {
        calls: Cell::new(0),
        refuse: Some(2),
    };
    assert!(matches!(
        NodeMaterial::stage_with(&f.root(), bundle(b""), f.launch(), &check, 20),
        Err(Error::Certificate)
    ));
    assert!(f.root().is_dir());
    assert_eq!(
        fs::read(f.root().join("join-token")).unwrap(),
        b"synthetic-join-material"
    );
    let f = Fixture::new();
    let material = NodeMaterial::stage_with(
        &f.root(),
        bundle(b"opaque-owner-config"),
        f.launch(),
        &Checker::accept(),
        20,
    )
    .unwrap();
    assert_eq!(
        fs::read(f.root().join("runtime.toml")).unwrap(),
        b"opaque-owner-config"
    );
    assert!(matches!(material.command(), Err(Error::Policy)));
}

#[test]
fn explicit_fresh_dms_must_fit_received_ttl_and_certificate_without_defaults() {
    let f = Fixture::new();
    let m = NodeMaterial::stage_with(&f.root(), bundle(b""), f.launch(), &Checker::accept(), 20)
        .unwrap();
    let valid = crate::DeadManSwitch::new("trusted-owner".into(), vec![], 20, 10, 80).unwrap();
    assert!(m.check_dms(&valid, 20).is_ok());
    let mut stopped =
        crate::DeadManSwitch::new("trusted-owner".into(), vec![], 20, 10, 80).unwrap();
    stopped.trigger();
    assert!(m.check_dms(&stopped, 20).is_err());
    for (start, ttl, lifetime) in [(19, 10, 80), (20, 11, 80), (20, 10, 81)] {
        let d = crate::DeadManSwitch::new("trusted-owner".into(), vec![], start, ttl, lifetime)
            .unwrap();
        assert!(m.check_dms(&d, 20).is_err());
    }
}

// Only owned fake programs run here; never mesh, a real key, or an endpoint.
fn fake_native(script: &str) -> (Fixture, NodeMaterial) {
    let f = Fixture::new();
    fs::write(f.0.join("mesh"), script).unwrap();
    let material =
        NodeMaterial::stage_with(&f.root(), bundle(b""), f.launch(), &Checker::accept(), 20)
            .unwrap();
    (f, material)
}
#[test]
fn native_public_id_exact_command_and_environment_then_immediate_exit() {
    let script = format!(
        "#!/bin/sh\n[ \"$#\" = 5 ] && [ \"$1\" = auth ] && [ \"$2\" = node-id ] && [ \"$3\" = --key-path ] && [ \"$5\" = --json ] || exit 2\n[ -f \"$4\" ] && [ -d \"$HOME\" ] && [ \"$MESH_LLM_ANALYTICS\" = 0 ] && [ \"$DO_NOT_TRACK\" = 1 ] && [ -z \"$MESH_LLM_NODE_KEY_PATH\" ] || exit 3\nprintf '%s\\n' '{{\"node_id\":\"{}\"}}'\n",
        "02".repeat(32)
    );
    let (_f, material) = fake_native(&script);
    material
        .native_id(std::time::Instant::now() + std::time::Duration::from_secs(2))
        .unwrap();
}
#[test]
fn native_public_id_refuses_mismatch_unknown_duplicate_partial_and_failed_exit() {
    for body in [
        format!("printf '%s' '{{\"node_id\":\"{}\"}}'", "03".repeat(32)),
        format!(
            "printf '%s' '{{\"node_id\":\"{}\",\"extra\":0}}'",
            "02".repeat(32)
        ),
        format!(
            "printf '%s' '{{\"node_id\":\"{}\",\"node_id\":\"{}\"}}'",
            "02".repeat(32),
            "02".repeat(32)
        ),
        "printf '%s' '{'".into(),
        format!(
            "printf '%s' '{{\"node_id\":\"{}\"}}'; exit 1",
            "02".repeat(32)
        ),
        "printf '%s' 'hostile-private-marker'".into(),
    ] {
        let (_f, material) = fake_native(&format!("#!/bin/sh\n{body}\n"));
        let error = material
            .native_id(std::time::Instant::now() + std::time::Duration::from_secs(2))
            .unwrap_err();
        assert!(matches!(error, Error::NativeId));
        assert_eq!(
            error.to_string(),
            "native node identity unavailable or mismatched"
        );
    }
}
#[test]
fn native_capture_and_process_exit_share_one_finite_deadline() {
    for body in [
        "while :; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; done".to_string(),
        "exec /bin/sleep 2".to_string(),
        format!(
            "printf '%s' '{{\"node_id\":\"{}\"}}'; exec 1>&-; exec /bin/sleep 2",
            "02".repeat(32)
        ),
    ] {
        let (_f, material) = fake_native(&format!("#!/bin/sh\n{body}\n"));
        let start = std::time::Instant::now();
        assert!(matches!(
            material.native_id(start + std::time::Duration::from_millis(50)),
            Err(Error::NativeId)
        ));
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}

#[test]
fn native_model_selection_needs_no_local_weights_and_preserves_admission_flags() {
    let f = Fixture::new();
    let mut launch = f.launch();
    launch.gguf = None;
    launch.model = "org/repo@0123456789012345678901234567890123456789/model.gguf".into();
    launch.model_pin = Some(model::ModelPin {
        bytes: 1,
        sha256: [0; 32],
    });
    let mut received = bundle(b"");
    received.settings.model = launch.model.clone();
    let material =
        NodeMaterial::stage_with(&f.root(), received, launch, &Checker::accept(), 20).unwrap();
    let command = material.command().unwrap();
    let args: Vec<_> = command.get_args().map(|s| s.to_str().unwrap()).collect();
    assert_eq!(
        &args[..3],
        &[
            "serve",
            "--model",
            "org/repo@0123456789012345678901234567890123456789:model.gguf"
        ]
    );
    assert!(args.contains(&"--node-ownership-required"));
    assert!(args.contains(&"--join-file"));
    assert!(args.contains(&"--trust-owner"));
    assert!(args.windows(2).any(|a| a == ["--ctx-size", "4096"]));
    assert!(args.windows(2).any(|a| a == ["--parallel", "1"]));
    assert!(args.contains(&"--no-draft"));
    let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
    assert_eq!(
        env[std::ffi::OsStr::new("PATH")],
        Some(std::ffi::OsStr::new(
            "/usr/local/nvidia/bin:/usr/local/cuda/bin:/usr/sbin:/usr/bin:/sbin:/bin"
        ))
    );
    assert_eq!(
        env[std::ffi::OsStr::new("LD_LIBRARY_PATH")],
        Some(std::ffi::OsStr::new(
            "/usr/local/nvidia/lib64:/usr/local/nvidia/lib:/usr/local/cuda/lib64"
        ))
    );
    assert_eq!(
        env[std::ffi::OsStr::new("MESH_HF_RETRY_MAX_ATTEMPTS")],
        Some(std::ffi::OsStr::new("0"))
    );
    assert_eq!(
        env[std::ffi::OsStr::new("HF_XET_CLIENT_RETRY_MAX_ATTEMPTS")],
        Some(std::ffi::OsStr::new("0"))
    );
    assert!(!args.contains(&"--gguf"));
}

#[test]
fn native_selector_preserves_pinned_qwen_file_and_other_model_syntax() {
    let input = "unsloth/Qwen3.8-27B-GGUF@4ca720788d1e01f1bff70c033e0d0028fd02e502/Qwen3.8-27B-UD-Q4_K_M.gguf";
    assert_eq!(
        native_model_ref(input),
        "unsloth/Qwen3.8-27B-GGUF@4ca720788d1e01f1bff70c033e0d0028fd02e502:Qwen3.8-27B-UD-Q4_K_M.gguf"
    );
    for unchanged in [
        "catalog-model",
        "org/repo@revision:Q4_K_M",
        "org/repo@main/file.gguf",
        "org/repo@0123456789012345678901234567890123456789/../file.gguf",
    ] {
        assert_eq!(native_model_ref(unchanged), unchanged);
    }
}

#[test]
fn null_gguf_requires_pin_and_exact_pinned_ref() {
    let good = "org/repo@0123456789012345678901234567890123456789/model.gguf";
    let pin = model::ModelPin {
        bytes: 1,
        sha256: [0; 32],
    };
    for (model_ref, model_pin) in [
        (good, None),
        (
            good,
            Some(model::ModelPin {
                bytes: 0,
                sha256: [0; 32],
            }),
        ),
        ("org/repo@main/model.gguf", Some(pin)),
        ("synthetic-model", Some(pin)),
    ] {
        let f = Fixture::new();
        let mut launch = f.launch();
        launch.gguf = None;
        launch.model = model_ref.into();
        launch.model_pin = model_pin;
        let mut received = bundle(b"");
        received.settings.model = launch.model.clone();
        let root = f.root();
        assert!(matches!(
            NodeMaterial::stage_with(&root, received, launch, &Checker::accept(), 20),
            Err(Error::Policy)
        ));
        // Refused before any staging directory exists.
        assert!(!root.exists());
    }
}

#[test]
fn null_gguf_child_gets_dictated_cache_and_watch_targets_snapshot() {
    let f = Fixture::new();
    let mut launch = f.launch();
    launch.gguf = None;
    launch.model = "org/repo@0123456789012345678901234567890123456789/model.gguf".into();
    launch.model_pin = Some(model::ModelPin {
        bytes: 1,
        sha256: [0; 32],
    });
    let mut received = bundle(b"");
    received.settings.model = launch.model.clone();
    let root = f.root();
    let material =
        NodeMaterial::stage_with(&root, received, launch, &Checker::accept(), 20).unwrap();
    let command = material.command().unwrap();
    let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
    assert_eq!(
        env[std::ffi::OsStr::new("HF_HUB_CACHE")],
        Some(root.join("hf-cache").as_os_str())
    );
    let watch = material.model_watch().unwrap();
    assert!(!watch.verified() && !watch.failed());
}
