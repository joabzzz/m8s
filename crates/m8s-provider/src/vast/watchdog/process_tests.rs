//! No provider clients or credentials: qualify production detachment only.
use super::*;
use std::{
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
};

const HELPER: &str = "vast::watchdog::process_tests::synthetic_process_helper";
const ROLE: &str = "M8S_SYNTHETIC_DETACH_ROLE";

fn helper(role: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", HELPER, "--ignored", "--nocapture"])
        .env_clear()
        .env(ROLE, role)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    command
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn await_marker(stdout: &mut std::process::ChildStdout, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut line = Vec::new();
    let mut total = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "synthetic child deadline");
        let mut fd = libc::pollfd {
            fd: stdout.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut fd, 1, remaining.as_millis().min(8000) as i32) };
        if ready < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        assert!(ready > 0, "synthetic child unavailable");
        let mut byte = [0];
        assert_eq!(stdout.read(&mut byte).unwrap(), 1, "missing marker");
        total += 1;
        assert!(total <= 4096, "bounded synthetic output");
        if byte[0] == b'\n' {
            if line == marker.as_bytes() {
                return;
            }
            line.clear();
        } else {
            line.push(byte[0]);
        }
    }
}

#[test]
fn detached_worker_survives_exact_launcher_group_loss() {
    // A fresh group belongs solely to this synthetic launcher and its children.
    let mut command = helper("launcher");
    command.process_group(0).stdout(Stdio::piped());
    let mut parent = OwnedChild(command.spawn().unwrap());
    let mut stdout = parent.0.stdout.take().unwrap();
    await_marker(&mut stdout, "synthetic-worker-armed");
    let pid = i32::try_from(parent.0.id()).unwrap();
    assert_eq!(unsafe { libc::getpgid(pid) }, pid);
    assert_eq!(unsafe { libc::kill(-pid, libc::SIGKILL) }, 0);
    assert!(!parent.0.wait().unwrap().success());
    await_marker(&mut stdout, "synthetic-worker-survived");
}

#[test]
#[ignore = "invoked only as a synthetic subprocess by the parent-loss test"]
fn synthetic_process_helper() {
    match std::env::var(ROLE).as_deref() {
        Ok("launcher") => {
            let mut child = OwnedChild(helper("worker").stdout(Stdio::inherit()).spawn().unwrap());
            assert!(child.0.wait().unwrap().success());
        }
        Ok("worker") => {
            let parent = unsafe { libc::getppid() };
            detach().unwrap();
            assert_eq!(unsafe { libc::getsid(0) }, unsafe { libc::getpid() });
            println!("synthetic-worker-armed");
            std::io::stdout().flush().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while unsafe { libc::getppid() } == parent {
                assert!(Instant::now() < deadline, "launcher did not exit");
                std::thread::sleep(Duration::from_millis(10));
            }
            println!("synthetic-worker-survived");
            std::io::stdout().flush().unwrap();
        }
        _ => panic!("synthetic helper requires its test harness"),
    }
}
