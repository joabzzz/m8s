#![cfg(unix)]
use std::{
    io::Write,
    os::unix::{fs::DirBuilderExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct StateDirectory(PathBuf);
impl StateDirectory {
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for StateDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn delayed_local_client_survives_nonblocking_listener_accept() {
    // Keep AF_UNIX paths below BSD's short sockaddr_un limit; macOS TMPDIR
    // can exceed that limit even before appending a fixture directory.
    let path = std::fs::canonicalize("/tmp").unwrap().join(format!(
        "m8s-delayed-client-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    let state = StateDirectory(path);
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_m8sd"))
            .arg(state.path())
            .arg("synthetic-delay-owner")
            .env_clear()
            .env("HOME", state.path())
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let path = state.path().join("control.sock");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !path.exists() {
        assert!(daemon.0.try_wait().unwrap().is_none());
        assert!(
            Instant::now() < deadline,
            "synthetic daemon startup timeout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Longer than the listener's 50ms poll interval, but within its 2s I/O
    // limit. This models socket-path/peer validation before sending Status.
    for _ in 0..2 {
        let mut stream = UnixStream::connect(&path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        std::thread::sleep(Duration::from_millis(150));
        stream.write_all(b"{\"command\":\"status\"}\n").unwrap();
        let frame = m8s_api::read_frame(&mut stream).unwrap();
        let response: m8s_api::Response = serde_json::from_slice(&frame).unwrap();
        assert!(response.ok);
        assert_eq!(response.mode, "mock-only");
        assert!(response.nodes.is_empty());
        assert!(daemon.0.try_wait().unwrap().is_none());
    }
}
