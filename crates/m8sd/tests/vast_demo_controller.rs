//! Synthetic CLI child only. No provider, credential, model or paid resource.
#![cfg(all(unix, feature = "prototype-vast"))]
use m8s_api::{Request, Response};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    daemon: Option<Child>,
}
impl Fixture {
    fn new() -> Self {
        // Darwin's per-user temp directory leaves too little sun_path space.
        // The fixture remains a unique, privately created child of sticky /tmp.
        let parent = PathBuf::from("/tmp").canonicalize().unwrap();
        let root = parent.join(format!(
            "m8s-vast-controller-{}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let cli = root.join("synthetic-cli");
        fs::write(
            &cli,
            r#"#!/bin/sh
set -eu
fixture_root=${0%/*}
case "$2" in
launch-check) exit 0 ;;
launch)
  printf 'called\n' >> "$fixture_root/calls"
  printf 'prototype stage: cleanup-armed\n' >&2
  printf 'untrusted secret-like output must not be retained\n' >&2
  count=0
  while [ ! -f "$fixture_root/state/down-requested" ] && [ "$count" -lt 100 ]; do
    sleep .05
    count=$((count + 1))
  done
  printf 'done\n' > "$fixture_root/finished"
  exit 0 ;;
*) exit 2 ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
        let plan = serde_json::json!({"schema":"m8s.vast-demo/v1",
            "directory":root.join("attempt"),"control_directory":root.join("state"),
            "owner":"synthetic-owner","tier":"qwen38-27b"});
        fs::write(
            root.join("selected-plan.json"),
            serde_json::to_vec(&plan).unwrap(),
        )
        .unwrap();
        Self { root, daemon: None }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_m8sd"));
        command
            .args(["--vast-demo", "--state-dir"])
            .arg(self.root.join("state"))
            .arg("--plan")
            .arg(self.root.join("selected-plan.json"))
            .arg("--m8s")
            .arg(self.root.join("synthetic-cli"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }
    fn refused_start(&self) {
        let mut child = self.command().spawn().unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(!status.success());
                return;
            }
            if Instant::now() >= until {
                let _ = child.kill();
                let _ = child.wait();
                panic!("unsafe controller startup did not refuse");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn start(&mut self) {
        self.daemon = Some(self.command().spawn().unwrap());
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(r) = m8s_api::call(&self.root.join("state/control.sock"), &Request::Status {})
            {
                assert!(r.ok);
                return;
            }
            assert!(self.daemon.as_mut().unwrap().try_wait().unwrap().is_none());
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn call(&self, r: Request) -> Response {
        m8s_api::call(&self.root.join("state/control.sock"), &r).unwrap()
    }
    fn stop_daemon(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    fn wait_for_launch(&self) {
        let until = Instant::now() + Duration::from_secs(2);
        while fs::read(self.root.join("calls")).ok().as_deref() != Some(b"called\n") {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::write(self.root.join("state/down-requested"), b"down\n");
        self.stop_daemon();
        // Let the exact synthetic child observe down before removing its
        // marker. Retain the fixture on uncertainty; its five-second bound is
        // independent of daemon lifetime, and no broad process kill is used.
        if self.root.join("calls").exists() {
            let until = Instant::now() + Duration::from_secs(1);
            while !self.root.join("finished").exists() {
                if Instant::now() >= until {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn real_control_commands_launch_only_once_refuse_extra_replicas_and_request_down() {
    let mut f = Fixture::new();
    f.start();
    assert!(
        !f.call(Request::Scale {
            tier: "qwen38-27b".into(),
            replicas: 2
        })
        .ok
    );
    assert!(!f.root.join("calls").exists());
    assert!(
        f.call(Request::Up {
            tier: "qwen38-27b".into()
        })
        .ok
    );
    f.wait_for_launch();
    assert!(
        f.call(Request::Scale {
            tier: "qwen38-27b".into(),
            replicas: 1
        })
        .ok
    );
    assert_eq!(
        fs::read_to_string(f.root.join("calls")).unwrap(),
        "called\n"
    );
    let status = f.call(Request::Status {});
    assert_eq!(status.mode, "vast-prototype");
    assert!(
        status
            .secure_bootstrap
            .contains("not fresh provider absence")
    );
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let logs = f.call(Request::Logs {});
        assert!(logs.events.iter().all(|e| !e.contains("secret-like")));
        if logs.events.iter().any(|e| e.contains("cleanup-armed")) {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(f.call(Request::Down {}).ok);
    assert_eq!(
        fs::read(f.root.join("state/down-requested")).unwrap(),
        b"down\n"
    );
    assert!(
        !f.call(Request::Up {
            tier: "qwen38-27b".into()
        })
        .ok
    );
}
#[test]
fn restart_cannot_replay_a_claim_even_when_provider_outcome_is_unknown() {
    let mut f = Fixture::new();
    f.start();
    assert!(
        f.call(Request::Up {
            tier: "qwen38-27b".into()
        })
        .ok
    );
    f.wait_for_launch();
    f.stop_daemon();
    f.start();
    assert!(
        !f.call(Request::Up {
            tier: "qwen38-27b".into()
        })
        .ok
    );
    assert_eq!(
        fs::read_to_string(f.root.join("calls")).unwrap(),
        "called\n"
    );
    let r = f.call(Request::Status {});
    assert!(
        r.events
            .iter()
            .any(|e| e.contains("provider outcome not yet observed"))
    );
    assert!(f.call(Request::Down {}).ok);
}

#[test]
fn public_state_directory_refuses_before_launch_or_claim() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("state")).unwrap();
    fs::set_permissions(f.root.join("state"), fs::Permissions::from_mode(0o755)).unwrap();
    f.refused_start();
    assert!(!f.root.join("state/launch-claimed").exists());
    assert!(!f.root.join("calls").exists());
}

#[test]
fn preexisting_attempt_is_retained_and_never_adopted_as_a_live_demo() {
    let f = Fixture::new();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(f.root.join("attempt"))
        .unwrap();
    fs::write(f.root.join("attempt/retained"), b"synthetic old state").unwrap();
    f.refused_start();
    assert_eq!(
        fs::read(f.root.join("attempt/retained")).unwrap(),
        b"synthetic old state"
    );
    assert!(!f.root.join("state/launch-claimed").exists());
    assert!(!f.root.join("calls").exists());
}
