//! Real CLI processes, only dedicated private synthetic status sockets.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
fn dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn socket(dir: &tempfile::TempDir) -> (PathBuf, UnixListener) {
    let path = dir.path().canonicalize().unwrap().join("synthetic.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    (path, listener)
}
fn fixture() -> Value {
    json!({"ok":true,"mode":"synthetic-only","nodes":[
        {"uid":"synthetic-node","tier":"synthetic-pool","owner":"synthetic-owner",
         "instance_id":"synthetic-instance","phase":"Ready","created_at":42}],
        "tiers":[["synthetic-pool",1]],"events":["opaque-server-event"],
        "error":null,"secure_bootstrap":"unavailable"})
}
fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_m8s"));
    // Makes an accidental PATH/sibling discovery dispatch fail. No inherited
    // credentials or controlling TTY are used by these snapshot tests.
    command
        .env_clear()
        .env("PATH", "/synthetic/no-executables")
        .stdin(Stdio::null());
    command
}
fn top(path: &Path, alias: bool, flags: &[&str]) -> Output {
    let mut command = command();
    if alias {
        command.arg("top");
    }
    command.arg("--socket").arg(path);
    if !alias {
        command.arg("top");
    }
    command.args(flags).output().unwrap()
}
fn exchange(expected: Value, response: Vec<u8>, invoke: impl FnOnce(&Path) -> Output) -> Output {
    let dir = dir();
    let (path, listener) = socket(&dir);
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "CLI never reached synthetic socket"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => panic!("synthetic accept failed"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let frame = m8s_api::read_frame(&mut stream).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&frame).unwrap(), expected);
        stream.write_all(&response).unwrap();
        let mut rest = [0; 1];
        assert_eq!(
            stream.read(&mut rest).unwrap(),
            0,
            "only one request allowed"
        );
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    });
    let output = invoke(&path);
    server.join().unwrap();
    output
}
fn response(value: Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    bytes
}
#[test]
fn canonical_and_alias_json_and_once_are_status_only_and_unknown() {
    for alias in [false, true] {
        for flags in [
            vec!["--once"],
            vec!["--json"],
            vec![],
            vec!["--once", "--json", "--timeout-ms", "1000"],
        ] {
            let output = exchange(json!({"command":"status"}), response(fixture()), |path| {
                top(path, alias, &flags)
            });
            assert!(output.status.success(), "{:?}", output.stderr);
            assert!(output.stderr.is_empty());
            assert!(!output.stdout.contains(&0x1b));
            let text = String::from_utf8(output.stdout.clone()).unwrap();
            assert!(!text.contains("opaque-server-event"));
            if flags.contains(&"--json") {
                let value: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(value["reported_mode"], "synthetic-only");
                assert_eq!(value["nodes"][0]["reported_phase"], "Ready");
                for field in [
                    "bootstrap_verified",
                    "heartbeat_age_seconds",
                    "hourly_micro_usd",
                ] {
                    assert!(value[field].is_null());
                }
                for field in ["model_ready", "last_observation"] {
                    assert!(value["nodes"][0][field].is_null());
                }
            } else {
                assert!(text.contains("reported Ready (unverified)"));
            }
        }
    }
}
#[test]
fn refusals_do_not_emit_values_or_contact_socket() {
    let dir = dir();
    let (path, listener) = socket(&dir);
    for alias in [false, true] {
        for flags in [
            vec!["--timeout-ms", "0"],
            vec!["--timeout-ms", "10001"],
            vec!["--timeout-ms", "opaque-invalid-value"],
            vec!["--once", "--once"],
            vec!["opaque-invalid-value"],
            vec!["--socket", "opaque-invalid-value"],
        ] {
            let output = top(&path, alias, &flags);
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains("opaque-invalid-value"));
        }
        let link = dir.path().canonicalize().unwrap().join(if alias {
            "alias-link"
        } else {
            "prefix-link"
        });
        symlink(&path, &link).unwrap();
        let output = top(&link, alias, &["--json"]);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("status socket trust checks failed"));
        assert!(!error.contains(link.to_str().unwrap()));
    }
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
}
#[test]
fn response_errors_are_redacted_not_empty_fleet() {
    let mut rejected = fixture();
    rejected["ok"] = false.into();
    rejected["error"] = "opaque-server-error".into();
    for bytes in [response(rejected), b"opaque-invalid-payload\n".to_vec()] {
        let output = exchange(json!({"command":"status"}), bytes, |path| {
            top(path, false, &["--json"])
        });
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(!error.contains("opaque-server-error"));
        assert!(!error.contains("opaque-invalid-payload"));
    }
}
#[test]
fn legacy_commands_and_json_envelope_are_unchanged() {
    // Legacy positive responses use the accepted Mock contract, not top's
    // deliberately synthetic Ready projection fixture.
    let mut legacy = fixture();
    legacy["mode"] = json!("mock-only");
    legacy["nodes"][0]["phase"] = json!("Booting");
    for (args, expected) in [
        (
            vec!["up", "synthetic"],
            json!({"command":"up","tier":"synthetic"}),
        ),
        (vec!["down"], json!({"command":"down"})),
        (
            vec!["scale", "synthetic", "2"],
            json!({"command":"scale","tier":"synthetic","replicas":2}),
        ),
        (vec!["status"], json!({"command":"status"})),
        (vec!["logs"], json!({"command":"logs"})),
    ] {
        let output = exchange(expected, response(legacy.clone()), |path| {
            command()
                .arg("--socket")
                .arg(path)
                .args(&args)
                .arg("--json")
                .output()
                .unwrap()
        });
        assert!(output.status.success());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value, legacy); // Raw API envelope, not top projection.
    }
    assert!(
        command()
            .args(["secrets", "--help"])
            .output()
            .unwrap()
            .status
            .success()
    );
}
#[test]
fn invalid_utf8_remains_opaque_for_all_cli_paths() {
    for prefix in [
        vec!["top", "--socket"],
        vec!["--socket"],
        vec!["secrets"],
        vec!["--socket", "synthetic", "status"],
    ] {
        let output = command()
            .args(prefix)
            .arg(std::ffi::OsString::from_vec(b"opaque-\xff".to_vec()))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, b"m8s: invalid arguments\n");
    }
}
