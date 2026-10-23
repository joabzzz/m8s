#![cfg(all(unix, feature = "agent-runtime"))]
use m8s_agent::supervisor::*;
use m8s_agent::{DeadManSwitch, Effect, Outcome};
use std::sync::{Arc, Mutex};

struct FakeDriver {
    calls: Arc<Mutex<Vec<Effect>>>,
    absent: bool,
}
impl TeardownDriver for FakeDriver {
    fn perform(&mut self, effect: Effect) -> Outcome {
        self.calls.lock().unwrap().push(effect);
        if effect == Effect::ConfirmAbsent && self.absent {
            Outcome::ConfirmedAbsent
        } else if effect == Effect::Destroy {
            Outcome::Success // Accepted destroy is NOT proof of absence.
        } else {
            Outcome::Failure
        }
    }
}
fn spec() -> ChildSpec {
    ChildSpec {
        program: "/bin/sleep".into(),
        args: vec!["60".into()],
    }
}
fn policy() -> DeadManSwitch {
    DeadManSwitch::new("owner".into(), vec!["delegate".into()], 0, 10, 20).unwrap()
}
fn await_reaped<D: TeardownDriver>(supervisor: &mut Supervisor<D>) {
    for _ in 0..100 {
        if supervisor.child_exited().unwrap() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("synthetic child did not reach nonblocking reap completion");
}
#[test]
fn unavailable_boot_never_spawns() {
    let calls = Arc::default();
    let result = Supervisor::start(
        spec(),
        &mut UnavailableBoot,
        policy(),
        FakeDriver {
            calls,
            absent: false,
        },
        RetryTeardown::new(4).unwrap(),
    );
    assert!(result.is_err());
}
#[test]
fn ttl_stops_child_and_uncertainty_retries_with_capped_backoff() {
    let calls = Arc::new(Mutex::new(vec![]));
    let mut s = Supervisor::start(
        spec(),
        &mut LocalOnlyBoot,
        policy(),
        FakeDriver {
            calls: calls.clone(),
            absent: false,
        },
        RetryTeardown::new(4).unwrap(),
    )
    .unwrap();
    assert!(!s.heartbeat("inference", 9));
    assert_eq!(s.tick(9).unwrap(), TeardownState::Running);
    assert_eq!(s.tick(10).unwrap(), TeardownState::Pending);
    await_reaped(&mut s);
    assert_eq!(
        s.local_termination_state(),
        LocalTerminationState::DirectChildReaped
    );
    assert!(!s.heartbeat("owner", 10));
    assert_eq!(calls.lock().unwrap().len(), 4);
    s.tick(10).unwrap();
    assert_eq!(calls.lock().unwrap().len(), 4);
    for now in [11, 13, 17, 21] {
        s.tick(now).unwrap();
    }
    assert_eq!(calls.lock().unwrap().len(), 20);
    s.tick(20).unwrap(); // Backwards time never accelerates retry.
    assert_eq!(calls.lock().unwrap().len(), 20);
}
#[test]
fn hard_lifetime_and_explicit_shutdown_cannot_be_renewed() {
    let calls = Arc::default();
    let mut s = Supervisor::start(
        spec(),
        &mut LocalOnlyBoot,
        policy(),
        FakeDriver {
            calls,
            absent: true,
        },
        RetryTeardown::new(4).unwrap(),
    )
    .unwrap();
    assert!(s.heartbeat("delegate", 9));
    assert!(s.heartbeat("owner", 18));
    assert_eq!(s.tick(20).unwrap(), TeardownState::ConfirmedAbsent);
    await_reaped(&mut s);
    assert_eq!(
        s.local_termination_state(),
        LocalTerminationState::DirectChildReaped
    );
    assert_eq!(s.tick(21).unwrap(), TeardownState::ConfirmedAbsent);
    assert!(!s.heartbeat("owner", 21));
}
#[test]
fn explicit_shutdown_eventually_reaps_without_blocking_tick() {
    let calls = Arc::default();
    let mut s = Supervisor::start(
        spec(),
        &mut LocalOnlyBoot,
        policy(),
        FakeDriver {
            calls,
            absent: false,
        },
        RetryTeardown::new(4).unwrap(),
    )
    .unwrap();
    s.shutdown();
    assert_eq!(s.tick(0).unwrap(), TeardownState::Pending);
    await_reaped(&mut s);
    assert_eq!(
        s.local_termination_state(),
        LocalTerminationState::DirectChildReaped
    );
}
#[test]
fn binary_fails_closed_and_local_child_exit_is_not_destroyed() {
    let bin = env!("CARGO_BIN_EXE_m8s-agent");
    let output = std::process::Command::new(bin).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("secure boot unavailable"));
    let output = std::process::Command::new(bin)
        .args(["--local", "1", "--", "/bin/sleep", "60"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "local teardown requested; provider absence unconfirmed; owner cleanup required"
    ));
}
#[test]
fn no_inherited_environment_reaches_child() {
    // Only a synthetic marker is injected into the agent. No real environment
    // value is read, printed or written by the test.
    let start = std::time::Instant::now();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_m8s-agent"))
        .env("M8S_SYNTHETIC_ENV_SENTINEL", "synthetic")
        .args([
            "--local",
            "10",
            "--",
            "/bin/sh",
            "-c",
            "test -z \"${M8S_SYNTHETIC_ENV_SENTINEL+x}\" || /bin/sleep 60",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn drop_requests_stop_and_transfers_pending_reap_without_waiting() {
    let calls = Arc::default();
    let s = Supervisor::start(
        spec(),
        &mut LocalOnlyBoot,
        policy(),
        FakeDriver {
            calls,
            absent: false,
        },
        RetryTeardown::new(4).unwrap(),
    )
    .unwrap();
    let pid = i32::try_from(s.child_id()).unwrap();
    let start = std::time::Instant::now();
    drop(s);
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    // Supervisor ownership has ended. The test explicitly takes responsibility
    // for any still-pending direct-child reap; WNOHANG only, never blocking wait.
    for _ in 0..100 {
        let mut status = 0;
        // SAFETY: positive PID was our direct child; status is writable storage.
        // No future supervisor operation can signal this PID after drop.
        let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        if reaped == pid {
            return;
        }
        if reaped < 0 {
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
            return; // Already reaped by the bounded Drop step.
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("synthetic direct child still pending after transferred test cleanup");
}

#[test]
fn early_child_exit_latches_teardown() {
    let calls = Arc::new(Mutex::new(vec![]));
    let mut s = Supervisor::start(
        ChildSpec {
            program: "/usr/bin/true".into(),
            args: vec![],
        },
        &mut LocalOnlyBoot,
        policy(),
        FakeDriver {
            calls: calls.clone(),
            absent: false,
        },
        RetryTeardown::new(4).unwrap(),
    )
    .unwrap();
    for _ in 0..100 {
        match s.child_exited() {
            Ok(false) => {}
            Ok(true) => {
                assert_eq!(s.tick(0).unwrap(), TeardownState::Pending);
                assert!(!s.heartbeat("owner", 0));
                return;
            }
            Err(error) => {
                // macOS sandbox may deny group signalling an exited leader.
                // This is cleanup uncertainty, never successful termination.
                assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
                assert_eq!(s.tick(0).unwrap(), TeardownState::Pending);
                assert_eq!(
                    s.local_termination_state(),
                    LocalTerminationState::Uncertain
                );
                assert!(calls.lock().unwrap().contains(&Effect::Destroy));
                let attempts = calls.lock().unwrap().len();
                assert_eq!(s.tick(0).unwrap(), TeardownState::Pending);
                assert_eq!(calls.lock().unwrap().len(), attempts);
                assert!(!s.heartbeat("owner", 0));
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("local child did not exit");
}

#[test]
fn spawn_failure_has_no_teardown_side_effects() {
    let calls = Arc::new(Mutex::new(vec![]));
    let result = Supervisor::start(
        ChildSpec {
            program: "/m8s-synthetic-missing-program".into(),
            args: vec![],
        },
        &mut LocalOnlyBoot,
        policy(),
        FakeDriver {
            calls: calls.clone(),
            absent: false,
        },
        RetryTeardown::new(4).unwrap(),
    );
    assert!(result.is_err());
    assert!(calls.lock().unwrap().is_empty());
    assert!(RetryTeardown::new(0).is_err());
}

#[test]
fn shutdown_terminates_descendant_in_dedicated_process_group() {
    assert_descendant_cleanup(false);
}

#[test]
fn natural_leader_exit_terminates_remaining_descendant_group() {
    assert_descendant_cleanup(true);
}

fn assert_descendant_cleanup(natural_exit: bool) {
    let file = std::env::temp_dir().join(format!(
        "m8s-agent-descendant-{}-{}",
        std::process::id(),
        natural_exit
    ));
    let calls = Arc::default();
    let mut s = Supervisor::start(
        ChildSpec {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                if natural_exit {
                    "/bin/sleep 60 & echo $! > \"$1\"; exit 0".into()
                } else {
                    "/bin/sleep 60 & echo $! > \"$1\"; wait".into()
                },
                "local-test".into(),
                file.clone().into_os_string(),
            ],
        },
        &mut LocalOnlyBoot,
        policy(),
        FakeDriver {
            calls,
            absent: false,
        },
        RetryTeardown::new(4).unwrap(),
    )
    .unwrap();
    let mut descendant = None;
    for _ in 0..100 {
        descendant = std::fs::read_to_string(&file)
            .ok()
            .and_then(|p| p.trim().parse::<u32>().ok());
        if descendant.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let pid = descendant.expect("descendant did not start");
    if natural_exit {
        for _ in 0..100 {
            if s.child_exited().unwrap() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    } else {
        s.shutdown();
    }
    s.tick(0).unwrap();
    await_reaped(&mut s);
    assert_eq!(
        s.local_termination_state(),
        LocalTerminationState::DirectChildReaped
    );
    std::fs::remove_file(file).unwrap();
    // OS init must reap the orphan; wait for disappearance without reading
    // system process listings (which restricted environments may prohibit).
    for _ in 0..100 {
        if !std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("descendant still present after process-group teardown");
}
