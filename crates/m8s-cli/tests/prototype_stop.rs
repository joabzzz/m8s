#![cfg(all(unix, feature = "prototype-container"))]
//! Owned fake Docker only: these are CLI cleanup tests, not container qualification.
use std::{
    fs,
    io::Write,
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        net::UnixListener,
    },
    process::Command,
};
struct Fixture {
    dir: tempfile::TempDir,
    _socket: UnixListener,
}
impl Fixture {
    fn new(fail: &str) -> Self {
        #[cfg(target_os = "macos")]
        let base = "/private/tmp";
        #[cfg(not(target_os = "macos"))]
        let base = "/tmp";
        let dir = tempfile::Builder::new()
            .prefix("m8s-cli-stop-")
            .tempdir_in(base)
            .unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("run"))
            .unwrap();
        let socket = UnixListener::bind(root.join("docker.sock")).unwrap();
        fs::set_permissions(root.join("docker.sock"), fs::Permissions::from_mode(0o600)).unwrap();
        let id = "a".repeat(64);
        let run = "c".repeat(32);
        let node = "02".repeat(32);
        let image = format!("local/worker@sha256:{}", "b".repeat(64));
        let mut intent = serde_json::json!({"schema":"m8s.container-intent/v2","image":image,"run_id":run,"node":node});
        let created = serde_json::json!({"schema":"m8s.container-created/v1","image":image,"run_id":run,"id":id});
        let script = format!(
            r#"#!/bin/sh
root='{root}'
[ "$1" = --host ] && [ "$2" = 'unix://{root}/docker.sock' ] && [ "$3" = container ] || exit 9
shift 3
printf '%s\n' "$*" >> "$root/calls"
case "$1" in
inspect)
 [ "$2" = '{id}' ] || exit 2
 running=true; [ -f "$root/stopped" ] && running=false
 run='{run}'; [ '{fail}' = labels ] && run=wrong
 printf '%s\n' "[{{\"Id\":\"{id}\",\"Name\":\"/m8s-{run}\",\"Config\":{{\"Image\":\"{image}\",\"Labels\":{{\"org.m8s.run\":\"$run\",\"org.m8s.node\":\"{node}\"}}}},\"State\":{{\"Running\":$running}}}}]" ;;
stop) [ "$4" = '{id}' ] || exit 2; printf yes > "$root/stopped" ;;
rm) [ "$2" = '{id}' ] || exit 2; printf yes > "$root/removed"; [ '{fail}' = rm ] && exit 1 ;;
ls) [ '{fail}' = ls ] && exit 1; [ -f "$root/removed" ] || printf '%s\n' '{id}' ;;
*) exit 9 ;;
esac
exit 0
"#,
            root = root.display()
        );
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o700)
            .open(root.join("docker"))
            .unwrap();
        file.write_all(script.as_bytes()).unwrap();
        use std::os::unix::fs::MetadataExt;
        let docker_metadata = fs::metadata(root.join("docker")).unwrap();
        let socket_metadata = fs::symlink_metadata(root.join("docker.sock")).unwrap();
        intent["transport"] = serde_json::json!({
            "docker":root.join("docker"),"socket":root.join("docker.sock"),
            "docker_dev":docker_metadata.dev(),"docker_ino":docker_metadata.ino(),"docker_uid":docker_metadata.uid(),"docker_mode":docker_metadata.mode(),
            "socket_dev":socket_metadata.dev(),"socket_ino":socket_metadata.ino(),"socket_uid":socket_metadata.uid(),"socket_mode":socket_metadata.mode()
        });
        for (name, value) in [
            ("container-intent.json", intent),
            ("container-created.json", created),
        ] {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(root.join("run").join(name))
                .unwrap();
            file.write_all(serde_json::to_string(&value).unwrap().as_bytes())
                .unwrap();
        }
        Self {
            dir,
            _socket: socket,
        }
    }
    fn command(&self) -> Command {
        let root = self.dir.path().canonicalize().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_m8s"));
        command
            .args(["prototype", "stop", "--run-directory"])
            .arg(root.join("run"))
            .args([
                "--run-id",
                &"c".repeat(32),
                "--container-id",
                &"a".repeat(64),
                "--docker",
            ])
            .arg(root.join("docker"))
            .arg("--docker-socket")
            .arg(root.join("docker.sock"))
            .args([
                "--command-timeout-ms",
                "3000",
                "--stop-grace-seconds",
                "1",
                "--json",
            ]);
        command
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.dir.path().join("calls")).unwrap_or_default()
    }
}
#[test]
fn cli_stops_owned_id_once_then_reports_only_local_absence() {
    let f = Fixture::new("");
    let out = f.command().output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["local_container"], "removed-observed");
    assert_eq!(value["billing"], "not-assessed");
    assert_eq!(value["provider_absence"], "not-assessed");
    let calls = f.calls();
    assert_eq!(calls.lines().filter(|s| s.starts_with("stop ")).count(), 1);
    assert_eq!(calls.lines().filter(|s| s.starts_with("rm ")).count(), 1);
    assert!(f.dir.path().join("run/container-created.json").exists());
}
#[test]
fn cli_refuses_identity_mismatch_and_lost_removal_or_confirmation() {
    for fail in ["labels", "rm", "ls"] {
        let f = Fixture::new(fail);
        let out = f.command().output().unwrap();
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("prototype-stop-refused-or-uncertain")
        );
        if fail == "labels" {
            assert!(
                !f.calls()
                    .lines()
                    .any(|s| s.starts_with("stop ") || s.starts_with("rm "))
            );
        }
        assert!(f.dir.path().join("run/container-created.json").exists());
    }
}
#[test]
fn cli_refuses_public_or_symlinked_records_before_docker() {
    for symlink in [false, true] {
        let f = Fixture::new("");
        let path = f.dir.path().join("run/container-created.json");
        if symlink {
            fs::rename(&path, f.dir.path().join("created-copy")).unwrap();
            std::os::unix::fs::symlink(f.dir.path().join("created-copy"), &path).unwrap();
        } else {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        let out = f.command().output().unwrap();
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert!(f.calls().is_empty());
    }
}

#[test]
fn cli_refuses_transport_substitution_before_contacting_docker() {
    let f = Fixture::new("");
    let path = f.dir.path().join("run/container-intent.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["transport"]["socket_ino"] = serde_json::json!(0);
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    let out = f.command().output().unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(f.calls().is_empty());
}

#[test]
fn explicit_read_only_resolution_after_lost_rm_reply_never_repeats_destruction() {
    let f = Fixture::new("rm");
    let failed = f.command().output().unwrap();
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
    let before = f.calls();
    let observed = f.command().arg("--observe-only").output().unwrap();
    assert!(
        observed.status.success(),
        "{}",
        String::from_utf8_lossy(&observed.stderr)
    );
    let after = f.calls();
    let extra = after.strip_prefix(&before).unwrap();
    assert_eq!(extra.lines().count(), 1);
    assert!(extra.starts_with("ls "));
    assert_eq!(after.lines().filter(|s| s.starts_with("rm ")).count(), 1);
    let value: serde_json::Value = serde_json::from_slice(&observed.stdout).unwrap();
    assert_eq!(value["local_container"], "removed-observed");
}
#[test]
fn read_only_resolution_of_present_id_is_not_absence() {
    let f = Fixture::new("");
    let out = f.command().arg("--observe-only").output().unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    let calls = f.calls();
    assert_eq!(calls.lines().count(), 1);
    assert!(calls.starts_with("ls "));
}
