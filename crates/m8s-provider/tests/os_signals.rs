//! Real local process/signal tests only. No admission or provider assertions.
#![cfg(all(unix, feature = "agent-runtime"))]
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

struct LocalAgent {
    child: Child,
    directory: PathBuf,
}
impl LocalAgent {
    fn launch(label: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("m8s-agent-signal-{}-{label}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let pidfile = directory.join("synthetic-child-pids");
        let child = Command::new(env!("CARGO_BIN_EXE_m8s-agent"))
            .env_clear()
            .args(["--local", "60", "--", "/bin/sh", "-c",
                "/bin/sleep 30 & descendant=$!; printf '%s %s\\n' \"$$\" \"$descendant\" > \"$1\"; wait",
                "synthetic-signal-test"])
            .arg(pidfile)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped())
            .spawn().unwrap();
        Self { child, directory }
    }
    fn await_children(&mut self) -> Vec<i32> {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "agent exited before readiness fixture"
            );
            if let Ok(data) = std::fs::read_to_string(self.directory.join("synthetic-child-pids")) {
                let ids: Vec<i32> = data
                    .split_whitespace()
                    .map(|id| id.parse().unwrap())
                    .collect();
                if ids.len() == 2 {
                    assert!(ids.iter().all(|pid| *pid > 1));
                    return ids;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("synthetic child/grandchild not started before deadline");
    }
    fn signal(&self, signal: i32) -> std::io::Result<()> {
        let pid = i32::try_from(self.child.id()).unwrap();
        assert!(pid > 1);
        // SAFETY: this unreaped direct child reserves the positive PID. Only
        // SIGTERM/SIGINT target the test's own agent, not another process group.
        if unsafe { libc::kill(pid, signal) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    fn await_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("agent failed to terminate within bounded local shutdown deadline");
    }
}
impl Drop for LocalAgent {
    fn drop(&mut self) {
        // Best-effort test cleanup on assertion failure. Never signal a numeric
        // PID after reaping; successful tests have already reaped the agent.
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.signal(libc::SIGTERM);
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if !matches!(self.child.try_wait(), Ok(None)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            if matches!(self.child.try_wait(), Ok(None)) {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn assert_signal_cleanup(label: &str, first: i32, repeated: bool) {
    let mut agent = LocalAgent::launch(label);
    let children = agent.await_children();
    let start = Instant::now();
    agent.signal(first).unwrap();
    if repeated {
        // Repeat both signals while the owned agent PID remains unreaped. These
        // must be idempotent requests, not a default-handler forced termination.
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGTERM, libc::SIGINT] {
            if let Err(error) = agent.signal(signal) {
                assert!(matches!(
                    error.raw_os_error(),
                    Some(libc::ESRCH | libc::EPERM)
                ));
            }
        }
    }
    let status = agent.await_exit();
    assert_eq!(
        status.code(),
        Some(2),
        "must not die by default signal disposition"
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    let mut stderr = String::new();
    agent
        .child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.contains("provider absence unconfirmed"));
    assert!(!stderr.contains("local child termination failed"));
    for pid in children {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            // A separate local kill -0 checks only existence. Orphan descendant
            // reaping belongs to OS init, not to the agent's waitpid boundary.
            let exists = Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success();
            if !exists {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "synthetic group member remains after signal cleanup"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
#[test]
fn sigterm_stops_agent_child_and_grandchild() {
    assert_signal_cleanup("term", libc::SIGTERM, false);
}
#[test]
fn sigint_stops_agent_child_and_grandchild() {
    assert_signal_cleanup("int", libc::SIGINT, false);
}
#[test]
fn repeated_signals_remain_idempotent_and_bounded() {
    assert_signal_cleanup("repeated", libc::SIGTERM, true);
}
