#![cfg(unix)]
//! Own bounded synthetic Unix peer and public CLI; no file config/provider.
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::OwnedFd,
        unix::{
            ffi::OsStringExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};
struct Client(Child, File, File);
impl Client {
    fn spawn(args: &[OsString], broken: bool) -> Self {
        let out = tempfile::tempfile().unwrap();
        let err = tempfile::tempfile().unwrap();
        let stdout = if broken {
            let (reader, writer) = UnixStream::pair().unwrap();
            drop(reader);
            let fd: OwnedFd = writer.into();
            Stdio::from(fd)
        } else {
            Stdio::from(out.try_clone().unwrap())
        };
        Self(
            Command::new(env!("CARGO_BIN_EXE_m8s"))
                .args(args)
                .env_clear()
                .stdin(Stdio::null())
                .stdout(stdout)
                .stderr(err.try_clone().unwrap())
                .spawn()
                .unwrap(),
            out,
            err,
        )
    }
    fn output(mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(6);
        let status = loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "owned CLI deadline");
            std::thread::sleep(Duration::from_millis(5));
        };
        self.1.seek(SeekFrom::Start(0)).unwrap();
        self.2.seek(SeekFrom::Start(0)).unwrap();
        let (mut stdout, mut stderr) = (vec![], vec![]);
        (&self.1).take(65537).read_to_end(&mut stdout).unwrap();
        (&self.2).take(4096).read_to_end(&mut stderr).unwrap();
        assert!(stdout.len() <= 65536 && stderr.len() < 4096);
        Output {
            status,
            stdout,
            stderr,
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Peer {
    _dir: tempfile::TempDir,
    path: PathBuf,
    listener: UnixListener,
}
impl Peer {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("m8cs-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = dir.path().canonicalize().unwrap().join("sensitive-socket");
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self {
            _dir: dir,
            path,
            listener,
        }
    }
    fn args(&self) -> Vec<OsString> {
        vec![
            "--socket".into(),
            self.path.clone().into(),
            "reload-config".into(),
            "--check".into(),
            "--json".into(),
        ]
    }
    fn serve(&self, args: &[OsString], reply: &[u8], broken: bool) -> Output {
        let child = Client::spawn(args, broken);
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut stream = loop {
            match self.listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("{e}"),
            };
            assert!(Instant::now() < deadline, "synthetic accept deadline");
            std::thread::sleep(Duration::from_millis(5));
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let request = m8s_api::read_frame(&mut stream).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&request).unwrap(),
            json!({"command":"reload_config_check"})
        );
        let _ = stream.write_all(reply);
        drop(stream);
        child.output()
    }
}
fn response() -> Value {
    json!({"schema":"m8s.reload-check/v1","mode":"mock-only","scope":"mock-control-admission-only","ok":true,"error":null,
        "check_only":true,"applied":false,"later_reload_guaranteed":false,"active":[{"id":"a","min":0,"max":1}],"candidate":[{"id":"a","min":0,"max":2}],"bounds_changed":true})
}
fn frame(v: &Value) -> Vec<u8> {
    let mut b = serde_json::to_vec(v).unwrap();
    b.push(b'\n');
    b
}
fn refused(o: Output, code: &str) {
    assert!(!o.status.success());
    assert_eq!(o.stderr, format!("m8s: {code}\n").as_bytes());
    if !o.stdout.is_empty() {
        assert_eq!(
            serde_json::from_slice::<Value>(&o.stdout).unwrap(),
            json!({"ok":false,"error":code})
        );
    }
}
#[test]
fn safe_deterministic_projection_and_broken_output() {
    let p = Peer::new();
    let args = p.args();
    let expected = response();
    let out = p.serve(&args, &frame(&expected), false);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        expected
    );
    assert_eq!(p.serve(&args, &frame(&expected), false).stdout, out.stdout);
    assert_eq!(
        p.serve(&args[..4], &frame(&expected), false).stdout,
        out.stdout
    );
    let mut reverse = args.clone();
    reverse.swap(3, 4);
    assert_eq!(
        p.serve(&reverse, &frame(&expected), false).stdout,
        out.stdout
    );
    refused(
        p.serve(&args, &frame(&expected), true),
        "reload-check-output-unavailable",
    );
}
#[test]
fn invalid_args_help_and_utf8_never_connect() {
    let p = Peer::new();
    for extra in [
        vec!["--check"],
        vec!["--json"],
        vec!["--config", "sensitive"],
        vec!["--help"],
        vec!["sensitive"],
    ] {
        let mut args = p.args();
        args.extend(extra.into_iter().map(OsString::from));
        refused(
            Client::spawn(&args, false).output(),
            "invalid-reload-check-arguments",
        );
        assert_eq!(
            p.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    for args in [
        vec!["reload-config", "--check", "--json"],
        vec!["reload-config", "--help", "--json"],
    ] {
        let args: Vec<_> = args.into_iter().map(OsString::from).collect();
        refused(
            Client::spawn(&args, false).output(),
            "invalid-reload-check-arguments",
        );
    }
    let mut args = p.args();
    args[1] = OsString::from_vec(vec![255]);
    refused(Client::spawn(&args, false).output(), "invalid arguments");
    let help = Client::spawn(&["reload-config".into(), "--help".into()], false).output();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("Later reload can fail")
    );
    assert_eq!(
        p.listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
#[test]
fn strict_remote_semantics_and_fixed_refusals() {
    let p = Peer::new();
    let args = p.args();
    for (key, value) in [
        ("mode", json!("sensitive\u{1b}")),
        ("applied", json!(true)),
        ("bounds_changed", json!(false)),
        ("candidate", json!([{"id":"a\u{1b}","min":0,"max":1}])),
        ("extra", json!("sensitive")),
    ] {
        let mut v = response();
        v[key] = value;
        refused(
            p.serve(&args, &frame(&v), false),
            "reload-check-unavailable",
        );
    }
    for b in [
        b"{}".to_vec(),
        b"sensitive\n".to_vec(),
        vec![b'x'; 65537],
        b"{\"ok\":true,\"ok\":false}\n".to_vec(),
    ] {
        refused(p.serve(&args, &b, false), "reload-check-unavailable");
    }
    let mut v = response();
    v["ok"] = json!(false);
    v["error"] = json!("reload-check-refused");
    v["active"] = json!([]);
    v["candidate"] = json!([]);
    v["bounds_changed"] = Value::Null;
    refused(p.serve(&args, &frame(&v), false), "reload-check-refused");
}
