//! Fake owned Docker processes only. No Docker daemon/image/model/mesh is run.
use super::*;
use m8s_bootstrap::{BundleSettings, CertificateVerifier};
use std::{
    os::unix::net::UnixListener,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
    _socket: UnixListener,
}
impl Fixture {
    fn new(fail: &str) -> Self {
        #[cfg(target_os = "macos")]
        let base = Path::new("/private/tmp");
        #[cfg(not(target_os = "macos"))]
        let base = Path::new("/tmp");
        let path = base.join(format!(
            "m8s-container-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        let path = path.canonicalize().unwrap();
        let socket = UnixListener::bind(path.join("docker.sock")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path.join("docker.sock"), fs::Permissions::from_mode(0o600)).unwrap();
        let id = "a".repeat(64);
        let digest = "b".repeat(64);
        let run = "c".repeat(32);
        let node = "02".repeat(32);
        let script = format!(
            r#"#!/bin/sh
root='{root}'
[ "$1" = --host ] && [ "$2" = 'unix://{root}/docker.sock' ] && [ "$3" = container ] || exit 9
shift 3
printf '%s\n' "$*" >> "$root/calls"
command=$1
case "$command" in
create) printf C > "$root/status"; [ '{fail}' = create ] && exit 1; printf '%s\n' '{id}' ;;
inspect)
  [ "$2" = '{id}' ] || exit 3
  read status < "$root/status" || :
  running=false; [ "$status" = R ] && running=true
  run='{run}'; [ '{fail}' = labels ] && run='{wrong}'
  printf '%s\n' "[{{\"Id\":\"{id}\",\"Name\":\"/m8s-{run}\",\"Config\":{{\"Image\":\"local/worker@sha256:{digest}\",\"Labels\":{{\"org.m8s.run\":\"$run\",\"org.m8s.node\":\"{node}\"}}}},\"State\":{{\"Running\":$running}}}}]" ;;
start) [ "$2" = '{id}' ] || exit 3; printf R > "$root/status"; [ '{fail}' = start ] && exit 1; printf '%s\n' '{id}' ;;
exec) [ "$2" = '{id}' ] || exit 3; if [ '{fail}' = projection ]; then printf '%s\n' '{{"error":"private-marker-\u001b","readiness":"Ready"}}'; exit 0; fi; printf '%s\n' '{{"schema":"m8s.prototype-completion/v1","scope":"local-http-qualification-only","model":"synthetic-model","content":"synthetic-not-inference","finish_reason":"stop","identity":"not-verified","readiness":"not-assessed","owner_routing":"not-verified","provider_qualification":"not-performed","launch_performed":false}}' ;;
stop) [ "$4" = '{id}' ] || exit 3; printf C > "$root/status"; printf '%s\n' '{id}' ;;
rm) [ "$2" = '{id}' ] || exit 3; printf X > "$root/status"; [ '{fail}' = rm ] && exit 1; printf '%s\n' '{id}' ;;
ls) [ '{fail}' = ls ] && exit 1; read status < "$root/status" || :; [ "$status" = X ] || printf '%s\n' '{id}' ;;
*) exit 8 ;;
esac
exit 0
"#,
            root = path.display(),
            wrong = "d".repeat(32)
        );
        for (name, bytes, mode) in [
            ("docker", script.as_bytes(), 0o700),
            ("mesh", b"#!/bin/sh\nexit 1\n".as_slice(), 0o700),
            ("model", b"not-a-model".as_slice(), 0o600),
            ("key", b"synthetic-key-placeholder".as_slice(), 0o600),
            ("prompt", b"synthetic-prompt".as_slice(), 0o600),
        ] {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(mode)
                .open(path.join(name))
                .unwrap();
            file.write_all(bytes).unwrap();
        }
        Self {
            path,
            _socket: socket,
        }
    }
    fn seal(&self, session: &mut ContainerSession) {
        struct PublicFixture;
        impl CertificateVerifier for PublicFixture {
            fn verify(
                &self,
                _: &NodeCertificate,
                _: &CertificateClaims,
            ) -> m8s_bootstrap::Result<()> {
                Ok(())
            }
        }
        let material = NodeMaterial::stage_with(
            &self.path.join("run/material"),
            ReceivedBundle {
                claims: CertificateClaims {
                    owner: EndpointId([1; 32]),
                    node: EndpointId([2; 32]),
                    issued_at: 10,
                    expires_at: 100,
                },
                certificate: NodeCertificate(b"synthetic-not-certificate".to_vec()),
                join_token: b"synthetic-token-never-argv".to_vec().into(),
                acl: ControlAcl::owner_only(EndpointId([1; 32])),
                settings: BundleSettings {
                    model: "synthetic-model".into(),
                    config: Vec::new().into(),
                    heartbeat_ttl_seconds: 10,
                },
            },
            self.launch(),
            &PublicFixture,
            20,
        )
        .unwrap();
        session.material = Some(material);
    }
    fn launch(&self) -> MeshLaunch {
        MeshLaunch {
            program: self.path.join("mesh"),
            gguf: self.path.join("model"),
            node_key: self.path.join("key"),
            owner: EndpointId([1; 32]),
            node: EndpointId([2; 32]),
            mesh_owner_id: "1".repeat(64),
            model: "synthetic-model".into(),
            heartbeat_ttl_seconds: 10,
            port: 9337,
            console_port: 3131,
        }
    }
    fn spec(&self) -> ContainerSpec {
        ContainerSpec {
            docker: self.path.join("docker"),
            socket: self.path.join("docker.sock"),
            image: format!("local/worker@sha256:{}", "b".repeat(64)),
            run_id: "c".repeat(32),
            prompt_file: self.path.join("prompt"),
            max_tokens: 8,
            request_timeout_ms: 100,
            command_timeout: Duration::from_secs(3),
            stop_grace_seconds: 1,
        }
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.path.join("calls")).unwrap_or_default()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
#[test]
fn unpinned_image_refuses_before_run_directory_or_subprocess() {
    let f = Fixture::new("");
    let mut spec = f.spec();
    spec.image = "local/worker:latest".into();
    assert!(
        ContainerSession::create_stopped_for_qualification(&f.path.join("run"), f.launch(), spec)
            .is_err()
    );
    assert!(f.calls().is_empty());
    assert!(!f.path.join("run").exists());
}
#[test]
fn stopped_container_id_precedes_seal_and_unsealed_start_refuses_but_cleanup_works() {
    let f = Fixture::new("");
    let mut session = ContainerSession::create_stopped_for_qualification(
        &f.path.join("run"),
        f.launch(),
        f.spec(),
    )
    .unwrap();
    assert_eq!(session.state(), ContainerState::Created);
    assert!(f.path.join("run/container-created.json").is_file());
    assert_eq!(
        session.created_container_id(),
        Some("a".repeat(64).as_str())
    );
    assert!(!f.path.join("run/material").exists());
    assert!(session.start_checked().is_err());
    assert!(!f.calls().lines().any(|s| s.starts_with("start ")));
    session.stop_and_remove().unwrap();
    assert_eq!(session.state(), ContainerState::RemovedObserved);
}
#[test]
fn exact_owned_lifecycle_and_inside_container_request_without_port_widening() {
    let f = Fixture::new("");
    let mut session = ContainerSession::create_stopped_for_qualification(
        &f.path.join("run"),
        f.launch(),
        f.spec(),
    )
    .unwrap();
    assert_eq!(session.state(), ContainerState::Created);
    assert!(f.path.join("run/container-created.json").is_file());
    f.seal(&mut session);
    session.start_checked().unwrap();
    assert_eq!(session.state(), ContainerState::RunningObserved);
    let reply = session.request().unwrap();
    assert_eq!(reply.content, "synthetic-not-inference");
    session.stop_and_remove().unwrap();
    assert_eq!(session.state(), ContainerState::RemovedObserved);
    let calls = f.calls();
    let lines: Vec<_> = calls.lines().collect();
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.starts_with("create "))
            .count(),
        1
    );
    assert!(lines[0].contains("--pull=never"));
    assert!(lines[0].contains("--read-only"));
    assert!(lines[0].contains("dst=/model.gguf,readonly"));
    assert!(lines[0].contains("dst=/node-key,readonly"));
    assert_eq!(
        lines[0]
            .split_whitespace()
            .filter(|arg| *arg == "--node-ownership-required")
            .count(),
        1
    );
    assert!(lines[0].contains(
        "--node-ownership /bootstrap/material/node-ownership.json --node-ownership-required --config"
    ));
    for forbidden in [
        "--publish",
        "--listen-all",
        "--privileged",
        "--network=host",
        "--local-model-only",
        "synthetic-token-never-argv",
        "synthetic-prompt",
        "--owner-key",
        "--sealed-node",
    ] {
        assert!(!calls.contains(forbidden), "{forbidden}");
    }
    assert!(calls.contains("/usr/local/bin/m8s prototype request --local-qualification"));
    assert!(
        lines
            .last()
            .unwrap()
            .starts_with("ls --all --no-trunc --filter id=")
    );
    assert!(f.path.join("run/request-prompt").is_file()); // Teardown is not a host wipe.
}
#[test]
fn one_shot_returns_completion_only_after_confirmed_removal() {
    // Same lifecycle engine, fake Docker only; no BootGate or crypto proof is
    // fabricated by this private start_checked fixture.
    for failure in ["", "start", "projection", "rm", "ls"] {
        let f = Fixture::new(failure);
        let mut session = ContainerSession::create_stopped_for_qualification(
            &f.path.join("run"),
            f.launch(),
            f.spec(),
        )
        .unwrap();
        f.seal(&mut session);
        let started = session.start_checked();
        let result = session.finish_once(started);
        assert_eq!(result.is_ok(), failure.is_empty());
        if let Ok(completion) = result {
            assert_eq!(completion.content, "synthetic-not-inference");
        }
        assert_eq!(
            session.state(),
            if matches!(failure, "rm" | "ls") {
                ContainerState::TeardownUncertain
            } else {
                ContainerState::RemovedObserved
            }
        );
        let calls = f.calls();
        for operation in ["start ", "stop ", "rm "] {
            assert_eq!(
                calls.lines().filter(|s| s.starts_with(operation)).count(),
                1
            );
        }
        assert_eq!(
            calls.lines().filter(|s| s.starts_with("exec ")).count(),
            usize::from(failure != "start")
        );
    }
}
#[test]
fn one_shot_refusal_still_removes_own_stopped_container_without_start_or_request() {
    let f = Fixture::new("");
    let mut session = ContainerSession::create_stopped_for_qualification(
        &f.path.join("run"),
        f.launch(),
        f.spec(),
    )
    .unwrap();
    assert!(session.finish_once(Err(ContainerError)).is_err());
    assert_eq!(session.state(), ContainerState::RemovedObserved);
    let calls = f.calls();
    assert!(
        !calls
            .lines()
            .any(|s| s.starts_with("start ") || s.starts_with("exec "))
    );
    assert_eq!(calls.lines().filter(|s| s.starts_with("rm ")).count(), 1);
}
#[test]
fn ambiguous_create_is_retained_without_id_adoption_or_retry() {
    let f = Fixture::new("create");
    let mut session = ContainerSession::create_stopped_for_qualification(
        &f.path.join("run"),
        f.launch(),
        f.spec(),
    )
    .unwrap();
    assert_eq!(session.state(), ContainerState::CreateUncertain);
    assert!(session.start_checked().is_err());
    assert!(session.stop_and_remove().is_err());
    assert_eq!(f.calls().lines().count(), 1);
    assert!(f.path.join("run/container-intent.json").is_file());
}
#[test]
fn failed_start_stays_uncertain_but_explicit_exact_id_teardown_can_succeed() {
    let f = Fixture::new("start");
    let mut session = ContainerSession::create_stopped_for_qualification(
        &f.path.join("run"),
        f.launch(),
        f.spec(),
    )
    .unwrap();
    f.seal(&mut session);
    assert!(session.start_checked().is_err());
    assert_eq!(session.state(), ContainerState::StartUncertain);
    assert!(session.request().is_err());
    assert!(session.start_checked().is_err());
    session.stop_and_remove().unwrap();
    assert_eq!(session.state(), ContainerState::RemovedObserved);
    assert_eq!(
        f.calls()
            .lines()
            .filter(|s| s.starts_with("start "))
            .count(),
        1
    );
}
#[test]
fn wrong_labels_and_failed_remove_never_fabricate_absence() {
    for failure in ["labels", "rm"] {
        let f = Fixture::new(failure);
        let mut session = ContainerSession::create_stopped_for_qualification(
            &f.path.join("run"),
            f.launch(),
            f.spec(),
        )
        .unwrap();
        assert!(session.stop_and_remove().is_err());
        assert_eq!(session.state(), ContainerState::TeardownUncertain);
        let calls = f.calls();
        if failure == "labels" {
            assert!(
                !calls
                    .lines()
                    .any(|s| s.starts_with("rm ") || s.starts_with("stop "))
            );
        } else {
            assert!(!calls.lines().any(|s| s.starts_with("ls ")));
        }
        assert!(f.path.join("run/container-intent.json").exists());
        if failure == "rm" {
            assert!(session.observe_removed().unwrap());
            assert_eq!(session.state(), ContainerState::RemovedObserved);
            assert_eq!(
                f.calls().lines().filter(|s| s.starts_with("rm ")).count(),
                1
            );
        }
    }
}

#[test]
fn malformed_request_projection_is_static_failure_not_inference_or_ready() {
    let f = Fixture::new("projection");
    let mut session = ContainerSession::create_stopped_for_qualification(
        &f.path.join("run"),
        f.launch(),
        f.spec(),
    )
    .unwrap();
    f.seal(&mut session);
    session.start_checked().unwrap();
    let result = session.request();
    let error = match result {
        Ok(_) => panic!("malformed projection accepted"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "container operation refused or outcome uncertain; retain private run material"
    );
    assert_eq!(session.state(), ContainerState::RunningObserved);
    session.stop_and_remove().unwrap();
}

#[test]
fn teardown_still_works_after_late_bootstrap_material_loss() {
    let f = Fixture::new("");
    let mut session = ContainerSession::create_stopped_for_qualification(
        &f.path.join("run"),
        f.launch(),
        f.spec(),
    )
    .unwrap();
    f.seal(&mut session);
    session.start_checked().unwrap();
    fs::remove_file(f.path.join("run/material/join-token")).unwrap();
    assert!(session.request().is_err());
    session.stop_and_remove().unwrap();
    assert_eq!(session.state(), ContainerState::RemovedObserved);
}

struct JournalProbe {
    fail: Option<(&'static str, RecordStep)>,
    events: Vec<(String, RecordStep)>,
}
impl RecordIo for JournalProbe {
    fn step(&mut self, name: &str, step: RecordStep, file: &File, bytes: &[u8]) -> CResult<()> {
        self.events.push((name.to_owned(), step));
        if self.fail == Some((name, step)) {
            return Err(ContainerError);
        }
        SystemRecordIo.step(name, step, file, bytes)
    }
}
#[test]
fn intent_write_and_every_required_sync_must_complete_before_any_docker_effect() {
    for failure in [
        RecordStep::Write,
        RecordStep::FileSync,
        RecordStep::DirectorySync,
        RecordStep::ParentSync,
    ] {
        let f = Fixture::new("");
        let mut io = JournalProbe {
            fail: Some(("container-intent.json", failure)),
            events: Vec::new(),
        };
        assert!(
            ContainerSession::create_with_io(&f.path.join("run"), f.launch(), f.spec(), &mut io)
                .is_err()
        );
        assert!(f.calls().is_empty());
        assert!(f.path.join("run/container-intent.json").exists());
        assert_eq!(
            io.events.last(),
            Some(&("container-intent.json".into(), failure))
        );
    }
}
#[test]
fn created_id_sync_failure_never_exposes_created_and_retains_cleanup_custody() {
    for failure in [
        RecordStep::Write,
        RecordStep::FileSync,
        RecordStep::DirectorySync,
    ] {
        let f = Fixture::new("");
        let mut io = JournalProbe {
            fail: Some(("container-created.json", failure)),
            events: Vec::new(),
        };
        let mut session =
            ContainerSession::create_with_io(&f.path.join("run"), f.launch(), f.spec(), &mut io)
                .unwrap();
        assert_eq!(session.state(), ContainerState::CreateUncertain);
        assert!(session.created_container_id().is_none());
        assert!(f.path.join("run/container-created.json").exists());
        assert!(!f.calls().lines().any(|s| s.starts_with("start ")));
        session.stop_and_remove().unwrap();
        assert_eq!(session.state(), ContainerState::RemovedObserved);
    }
}
#[test]
fn start_record_sync_failure_prevents_start_and_never_retries_or_deletes_artifacts() {
    for failure in [
        RecordStep::Write,
        RecordStep::FileSync,
        RecordStep::DirectorySync,
    ] {
        let f = Fixture::new("");
        let mut session = ContainerSession::create_stopped_for_qualification(
            &f.path.join("run"),
            f.launch(),
            f.spec(),
        )
        .unwrap();
        f.seal(&mut session);
        let mut io = JournalProbe {
            fail: Some(("container-start-attempt", failure)),
            events: Vec::new(),
        };
        assert!(session.start_with_io(&mut io).is_err());
        assert_eq!(session.state(), ContainerState::StartUncertain);
        assert!(f.path.join("run/container-start-attempt").exists());
        assert!(!f.calls().lines().any(|s| s.starts_with("start ")));
        assert!(session.start_checked().is_err());
        session.stop_and_remove().unwrap();
        assert_eq!(session.state(), ContainerState::RemovedObserved);
    }
}
#[test]
fn journal_positive_control_orders_write_file_sync_directory_sync_before_publish_and_start() {
    let f = Fixture::new("");
    let mut io = JournalProbe {
        fail: None,
        events: Vec::new(),
    };
    let mut session =
        ContainerSession::create_with_io(&f.path.join("run"), f.launch(), f.spec(), &mut io)
            .unwrap();
    assert_eq!(session.state(), ContainerState::Created);
    assert_eq!(
        io.events,
        vec![
            ("container-intent.json".into(), RecordStep::Write),
            ("container-intent.json".into(), RecordStep::FileSync),
            ("container-intent.json".into(), RecordStep::DirectorySync),
            ("container-intent.json".into(), RecordStep::ParentSync),
            ("container-created.json".into(), RecordStep::Write),
            ("container-created.json".into(), RecordStep::FileSync),
            ("container-created.json".into(), RecordStep::DirectorySync),
        ]
    );
    f.seal(&mut session);
    session.start_with_io(&mut io).unwrap();
    assert_eq!(
        &io.events[7..],
        &[
            ("container-start-attempt".into(), RecordStep::Write),
            ("container-start-attempt".into(), RecordStep::FileSync),
            ("container-start-attempt".into(), RecordStep::DirectorySync),
        ]
    );
    assert_eq!(session.state(), ContainerState::RunningObserved);
    session.stop_and_remove().unwrap();
}

fn stop_spec(f: &Fixture) -> StopSpec {
    let spec = f.spec();
    StopSpec {
        docker: spec.docker,
        socket: spec.socket,
        run_id: spec.run_id,
        container_id: "a".repeat(64),
        command_timeout: spec.command_timeout,
        stop_grace_seconds: spec.stop_grace_seconds,
    }
}
#[test]
fn reopened_stop_uses_private_records_without_model_or_join_material() {
    let f = Fixture::new("");
    let session = ContainerSession::create_stopped_for_qualification(
        &f.path.join("run"),
        f.launch(),
        f.spec(),
    )
    .unwrap();
    drop(session);
    fs::write(f.path.join("status"), b"R").unwrap();
    fs::remove_file(f.path.join("model")).unwrap();
    fs::remove_file(f.path.join("key")).unwrap();
    let mut stop = OwnedContainerStop::open(&f.path.join("run"), stop_spec(&f)).unwrap();
    stop.stop_and_remove().unwrap();
    assert!(stop.stop_and_remove().is_err());
    let calls = f.calls();
    assert_eq!(calls.lines().filter(|s| s.starts_with("stop ")).count(), 1);
    assert_eq!(calls.lines().filter(|s| s.starts_with("rm ")).count(), 1);
    assert!(!calls.contains("prune"));
    assert!(f.path.join("run/container-created.json").exists());
}
#[test]
fn reopened_stop_refuses_bad_records_or_inspection_and_never_claims_rm_failure_as_absence() {
    for failure in ["labels", "rm", "ls"] {
        let f = Fixture::new(failure);
        let session = ContainerSession::create_stopped_for_qualification(
            &f.path.join("run"),
            f.launch(),
            f.spec(),
        )
        .unwrap();
        drop(session);
        // A failed initial inspect never produces a created record; recovery refuses.
        let result = OwnedContainerStop::open(&f.path.join("run"), stop_spec(&f));
        if failure == "labels" {
            assert!(result.is_err());
            continue;
        }
        let mut stop = result.unwrap();
        assert!(stop.stop_and_remove().is_err());
        assert!(stop.stop_and_remove().is_err());
        assert_eq!(
            f.calls().lines().filter(|s| s.starts_with("rm ")).count(),
            1
        );
    }
    let f = Fixture::new("");
    let _session = ContainerSession::create_stopped_for_qualification(
        &f.path.join("run"),
        f.launch(),
        f.spec(),
    )
    .unwrap();
    let mut spec = stop_spec(&f);
    spec.container_id = "f".repeat(64);
    assert!(OwnedContainerStop::open(&f.path.join("run"), spec).is_err());
}

#[test]
fn reopened_stop_rechecks_daemon_identity_before_destructive_calls() {
    let f = Fixture::new("");
    let session = ContainerSession::create_stopped_for_qualification(
        &f.path.join("run"),
        f.launch(),
        f.spec(),
    )
    .unwrap();
    drop(session);
    // Modify the owned fake's response before opening the cleanup handle.
    let script = fs::read_to_string(f.path.join("docker"))
        .unwrap()
        .replace(&format!("run='{}'", "c".repeat(32)), "run='wrong'");
    fs::write(f.path.join("docker"), script).unwrap();
    // Same inode/path and mode: the exact inspect fence must still reject the labels.
    let mut stop = OwnedContainerStop::open(&f.path.join("run"), stop_spec(&f)).unwrap();
    assert!(stop.stop_and_remove().is_err());
    assert!(
        !f.calls()
            .lines()
            .any(|s| s.starts_with("stop ") || s.starts_with("rm "))
    );
}
