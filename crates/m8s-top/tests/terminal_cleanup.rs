//! Synthetic PTY only: no access to the user's controlling terminal or secrets.
#![cfg(unix)]
use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    pty::{Winsize, openpty},
    sys::{
        signal::{Signal, kill},
        termios::tcgetattr,
    },
    unistd::Pid,
};
use std::{
    fs::File,
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{fs::PermissionsExt, process::CommandExt},
    },
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Reap(Child);
impl Drop for Reap {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
fn collect(master: &mut File, output: &mut Vec<u8>) {
    let mut buffer = [0u8; 8192];
    loop {
        match master.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                output.extend_from_slice(&buffer[..n]);
                assert!(
                    output.len() <= 1_000_000,
                    "synthetic terminal output exceeded test bound"
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}
fn contains(output: &[u8], marker: &[u8]) -> bool {
    output.windows(marker.len()).any(|window| window == marker)
}
fn cleanup_case(label: &str, signal: Option<Signal>, key: Option<&[u8]>) {
    let size = Winsize {
        ws_row: 24,
        ws_col: 150,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pty = openpty(Some(&size), None).expect("synthetic PTY capability required");
    let initial = tcgetattr(&pty.slave).unwrap();
    let flags =
        OFlag::from_bits_truncate(fcntl(pty.master.as_raw_fd(), FcntlArg::F_GETFL).unwrap());
    fcntl(
        pty.master.as_raw_fd(),
        FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK),
    )
    .unwrap();
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_m8s-top"));
    command
        .env_clear()
        .env("TERM", "xterm-256color")
        .arg("--socket")
        .arg(
            dir.path()
                .join(format!("synthetic-unavailable-{label}.sock")),
        )
        .stdin(Stdio::from(File::from(pty.slave.try_clone().unwrap())))
        .stdout(Stdio::from(File::from(pty.slave.try_clone().unwrap())))
        .stderr(Stdio::piped());
    // SAFETY: the post-fork closure performs only the async-signal-safe setsid
    // syscall, plus nonallocating errno conversion. Detaching prevents terminal
    // libraries from accidentally accessing an inherited user controlling TTY;
    // only our synthetic stdin/stdout slave remains available to this child.
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid()
                .map(|_| ())
                .map_err(std::io::Error::from)
        });
    }
    let mut agent = Reap(command.spawn().unwrap());
    let mut master = File::from(pty.master);
    let mut output = vec![];
    let deadline = Instant::now() + Duration::from_secs(3);
    while !contains(&output, b"\x1b[?1049h") {
        collect(&mut master, &mut output);
        assert!(
            agent.0.try_wait().unwrap().is_none(),
            "dashboard exited before synthetic screen entry"
        );
        assert!(
            Instant::now() < deadline,
            "dashboard did not enter synthetic alternate screen"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_ne!(
        tcgetattr(&pty.slave).unwrap(),
        initial,
        "expected raw mode before interruption"
    );
    if let Some(signal) = signal {
        kill(Pid::from_raw(i32::try_from(agent.0.id()).unwrap()), signal).unwrap();
    }
    if let Some(key) = key {
        master.write_all(key).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        collect(&mut master, &mut output);
        if let Some(status) = agent.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "dashboard failed bounded terminal cleanup"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    collect(&mut master, &mut output);
    assert!(
        status.success(),
        "dashboard should leave through normal cleanup scope"
    );
    assert!(
        contains(&output, b"\x1b[?1049l"),
        "alternate screen not left"
    );
    assert!(contains(&output, b"\x1b[?25h"), "cursor not restored");
    assert_eq!(
        tcgetattr(&pty.slave).unwrap(),
        initial,
        "terminal attributes not restored"
    );
    let mut stderr = String::new();
    agent
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.is_empty(), "unexpected synthetic dashboard failure");
}
#[test]
fn quit_key_restores_synthetic_terminal() {
    cleanup_case("quit", None, Some(b"q"));
}
#[test]
fn ctrl_c_key_restores_synthetic_terminal() {
    cleanup_case("ctrl-c", None, Some(b"\x03"));
}
#[test]
fn sigterm_restores_synthetic_terminal() {
    cleanup_case("term", Some(Signal::SIGTERM), None);
}
#[test]
fn sigint_restores_synthetic_terminal() {
    cleanup_case("int", Some(Signal::SIGINT), None);
}
