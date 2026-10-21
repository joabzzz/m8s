#![cfg(unix)]
use m8s_secrets::{FileStore, SecretName, SecretStore};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
fn fixture() -> (tempfile::TempDir, PathBuf) {
    #[cfg(target_os = "macos")]
    let base = "/private/tmp";
    #[cfg(not(target_os = "macos"))]
    let base = "/tmp";
    let dir = tempfile::Builder::new()
        .prefix("m8s-cli-synthetic-")
        .tempdir_in(base)
        .unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("secrets.toml");
    (dir, path)
}
fn cli(path: Option<&Path>, args: &[&str], input: &[u8]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_m8s"));
    command.arg("secrets");
    if let Some(path) = path {
        command.arg("--file").arg(path);
    }
    let mut child = command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    // Small synthetic writes fit pipe; early-rejected commands may close stdin.
    let _ = stdin.write_all(input);
    drop(stdin);
    child.wait_with_output().unwrap()
}
fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}
fn no_value(output: &Output) {
    assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-sensitive-value"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("synthetic-sensitive-value"));
}
#[test]
fn actual_file_cli_roundtrip_arbitrary_bytes_metadata_and_delete() {
    let (_dir, path) = fixture();
    let bytes: Vec<_> = (0..=255u8).collect();
    let output = cli(
        Some(&path),
        &["set", "test/session", "--stdin", "--json"],
        &bytes,
    );
    assert!(output.status.success());
    assert_eq!(
        json(&output),
        serde_json::json!({"ok":true,"operation":"set","backend":"file"})
    );
    assert!(
        output.stderr.is_empty(),
        "explicit stdin must not invoke terminal prompt"
    );
    assert!(fs::read(&path).unwrap().starts_with(b"version = 1\n"));
    let store = FileStore::open(&path).unwrap();
    assert_eq!(
        store
            .get(&SecretName::new("test/session").unwrap(), 0)
            .unwrap()
            .unwrap()
            .expose(),
        bytes
    );
    let output = cli(Some(&path), &["list", "--json"], b"");
    assert!(output.status.success());
    assert_eq!(
        json(&output)["entries"],
        serde_json::json!([{"name":"test/session","expires_at":null}])
    );
    let output = cli(Some(&path), &["check", "--json"], b"");
    assert!(output.status.success());
    let checked = json(&output);
    assert_eq!(checked["scope"], "local-storage-only");
    assert_eq!(checked["owner_keystore"], "not-checked");
    assert_eq!(checked["active_entries"], 1);
    for removed in [true, false] {
        let output = cli(Some(&path), &["rm", "test/session", "--json"], b"");
        assert!(output.status.success());
        assert_eq!(json(&output)["removed"], removed);
    }
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
#[test]
fn rejects_argv_values_no_get_show_no_implicit_backend_or_redirected_terminal() {
    let (_dir, path) = fixture();
    for args in [
        vec!["get", "test/session", "--json"],
        vec!["show", "--json"],
        vec!["set", "test/session", "synthetic-sensitive-value", "--json"],
        vec![
            "set",
            "test/session",
            "--value",
            "synthetic-sensitive-value",
            "--json",
        ],
        vec!["set", "test/session", "--json"],
        vec!["set", "test/session", "--stdin", "--stdin", "--json"],
        vec!["list", "--json", "--json"],
    ] {
        let output = cli(Some(&path), &args, b"synthetic-sensitive-value");
        assert!(!output.status.success());
        assert_eq!(
            json(&output),
            serde_json::json!({"ok":false,"error":"secret-command-failed"})
        );
        no_value(&output);
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Secret (no echo)"));
        assert!(!path.exists());
    }
    // No file/default/env discovery: missing explicit file never invokes Keychain.
    let output = cli(None, &["check", "--json"], b"");
    assert!(!output.status.success());
    assert_eq!(json(&output)["ok"], false);
}
#[test]
fn stdin_bound_expiry_and_failures_never_print_values_or_modify_prior_data() {
    let (_dir, path) = fixture();
    let output = cli(
        Some(&path),
        &["set", "test/session", "--stdin", "--json"],
        b"synthetic-sensitive-value\n",
    );
    assert!(output.status.success());
    no_value(&output);
    let old = fs::read(&path).unwrap();
    for input in [vec![], vec![b'x'; 65537]] {
        let output = cli(
            Some(&path),
            &["set", "test/session", "--stdin", "--json"],
            &input,
        );
        assert!(!output.status.success());
        assert_eq!(fs::read(&path).unwrap(), old);
    }
    let output = cli(
        Some(&path),
        &[
            "set",
            "test/session",
            "--stdin",
            "--expires-at",
            "0",
            "--json",
        ],
        b"synthetic-sensitive-value",
    );
    assert!(!output.status.success());
    no_value(&output);
    assert_eq!(fs::read(&path).unwrap(), old);
    let output = cli(
        Some(&path),
        &[
            "set",
            "test/max",
            "--stdin",
            "--expires-at",
            "9223372036854775807",
            "--json",
        ],
        &vec![b'x'; 65536],
    );
    assert!(output.status.success());
    let store = FileStore::open(&path).unwrap();
    assert_eq!(
        store
            .get(&SecretName::new("test/session").unwrap(), 0)
            .unwrap()
            .unwrap()
            .expose(),
        b"synthetic-sensitive-value\n"
    );
    assert_eq!(
        store
            .get(&SecretName::new("test/max").unwrap(), 0)
            .unwrap()
            .unwrap()
            .expose()
            .len(),
        65536
    );
}
#[test]
fn unsafe_file_git_symlink_and_malformed_toml_fail_closed() {
    let (dir, path) = fixture();
    let output = cli(
        Some(&path),
        &["set", "test/session", "--stdin"],
        b"synthetic-sensitive-value",
    );
    assert!(output.status.success());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let output = cli(Some(&path), &["check", "--json"], b"");
    assert!(!output.status.success());
    no_value(&output);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&path, b"version = 99\n# synthetic-sensitive-value\n").unwrap();
    let output = cli(Some(&path), &["list", "--json"], b"");
    assert!(!output.status.success());
    no_value(&output);
    fs::write(dir.path().join(".git"), b"synthetic git marker").unwrap();
    let output = cli(Some(&path), &["check", "--json"], b"");
    assert!(!output.status.success());
    no_value(&output);
    let (_other, link_path) = fixture();
    symlink(&path, &link_path).unwrap();
    let output = cli(Some(&link_path), &["check", "--json"], b"");
    assert!(!output.status.success());
    no_value(&output);
}

mod terminal {
    use super::*;
    use std::{
        fs::File,
        io::Read,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::process::CommandExt,
        },
        process::Child,
        time::{Duration, Instant},
    };
    struct Pty {
        child: Option<Child>,
        master: File,
        inspect: File,
        transcript: Vec<u8>,
    }
    fn attrs(file: &File) -> libc::termios {
        let mut attrs = std::mem::MaybeUninit::uninit();
        // SAFETY: live descriptor and valid out pointer.
        assert_eq!(
            unsafe { libc::tcgetattr(file.as_raw_fd(), attrs.as_mut_ptr()) },
            0
        );
        // SAFETY: tcgetattr initialized the buffer.
        unsafe { attrs.assume_init() }
    }
    impl Pty {
        fn start(path: &Path) -> Self {
            let (mut master, mut slave) = (-1, -1);
            // SAFETY: valid out pointers and null optional attributes.
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            // SAFETY: distinct owned descriptors from successful openpty.
            let master = unsafe { File::from_raw_fd(master) };
            // SAFETY: owned slave descriptor from openpty.
            let slave = unsafe { File::from_raw_fd(slave) };
            for file in [&master, &slave] {
                // SAFETY: live descriptor; do not leak extra PTY fds into child.
                assert_ne!(
                    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
                    -1
                );
            }
            // SAFETY: live descriptor; nonblocking test observation only.
            assert_ne!(
                unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) },
                -1
            );
            let inspect = slave.try_clone().unwrap();
            let mut command = Command::new(env!("CARGO_BIN_EXE_m8s"));
            command
                .args(["secrets", "--file"])
                .arg(path)
                .args(["set", "test/terminal", "--json"])
                .stdin(Stdio::from(slave.try_clone().unwrap()))
                .stdout(Stdio::piped())
                .stderr(Stdio::from(slave));
            // SAFETY: child callback uses only async-signal-safe libc calls. A new
            // session and synthetic controlling PTY keep /dev/tty away from host TTY.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            Self {
                child: Some(command.spawn().unwrap()),
                master,
                inspect,
                transcript: Vec::new(),
            }
        }
        fn drain(&mut self) {
            let mut buffer = [0; 1024];
            loop {
                match self.master.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => self.transcript.extend_from_slice(&buffer[..n]),
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.raw_os_error() == Some(libc::EIO) =>
                    {
                        break;
                    }
                    Err(e) => panic!("synthetic PTY read failed: {e}"),
                }
            }
        }
        fn enter(&mut self, prompt: &[u8], bytes: &[u8]) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                self.drain();
                let mode = attrs(&self.inspect);
                if self
                    .transcript
                    .windows(prompt.len())
                    .any(|part| part == prompt)
                {
                    assert_eq!(
                        mode.c_lflag & (libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG),
                        0,
                        "prompt observed before no-echo"
                    );
                    break;
                }
                assert!(
                    self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                    "child exited before fixed prompt"
                );
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for fixed prompt"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            self.master.write_all(bytes).unwrap();
        }
        fn finish(mut self) -> Output {
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.child.as_mut().unwrap().try_wait().unwrap().is_none() {
                self.drain();
                assert!(
                    Instant::now() < deadline,
                    "synthetic terminal child timed out"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            self.drain();
            // On macOS session-leader exit revokes this synthetic controlling
            // slave, so post-exit tcgetattr cannot inspect restoration here.
            // Library dedicated-PTY tests separately assert exact restoration.
            assert!(
                !String::from_utf8_lossy(&self.transcript).contains("synthetic-sensitive-value")
            );
            self.child.take().unwrap().wait_with_output().unwrap()
        }
    }
    impl Drop for Pty {
        fn drop(&mut self) {
            if let Some(child) = self.child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    #[test]
    #[ignore = "explicit OS qualification: requires /dev/tty access in a dedicated synthetic session"]
    fn actual_terminal_confirm_noecho_mismatch_and_cancel() {
        for scenario in 0..4 {
            let (_dir, path) = fixture();
            let mut pty = Pty::start(&path);
            pty.enter(
                b"Secret (no echo): ",
                if scenario == 2 {
                    &[3]
                } else {
                    b"synthetic-sensitive-value\n"
                },
            );
            if scenario != 2 {
                pty.enter(
                    b"Confirm secret (no echo): ",
                    match scenario {
                        0 => b"synthetic-sensitive-value\n",
                        1 => b"synthetic-mismatch\n",
                        _ => &[3],
                    },
                );
            }
            let output = pty.finish();
            no_value(&output);
            assert_eq!(output.status.success(), scenario == 0);
            assert_eq!(path.exists(), scenario == 0);
            if scenario == 0 {
                let store = FileStore::open(&path).unwrap();
                assert_eq!(
                    store
                        .get(&SecretName::new("test/terminal").unwrap(), 0)
                        .unwrap()
                        .unwrap()
                        .expose(),
                    b"synthetic-sensitive-value"
                );
            }
        }
    }
}

#[test]
fn help_and_non_utf8_arguments_never_open_backends_or_echo_raw_arguments() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let output = cli(None, &["--help"], b"");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Default Keychain is unavailable"));
    let mut malformed = vec![0xff];
    malformed.extend_from_slice(b"synthetic-sensitive-value");
    let output = Command::new(env!("CARGO_BIN_EXE_m8s"))
        .args(["secrets", "set", "test/session"])
        .arg(OsString::from_vec(malformed))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    no_value(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "m8s: invalid arguments\n"
    );
}
