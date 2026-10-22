#![cfg(unix)]
//! Own bounded synthetic Unix peer and public CLI; no file config/provider.
mod support;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::Write,
    os::unix::{ffi::OsStringExt, net::UnixListener},
    path::PathBuf,
    process::Output,
    time::{Duration, Instant},
};
use support::Client;
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
            "config-status".into(),
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
            json!({"command":"config_status"})
        );
        let _ = stream.write_all(reply);
        drop(stream);
        child.output()
    }
}
fn status() -> Value {
    json!({"ok":true,"mode":"mock-only","scope":"mock-control-admission-only","error":null,"configured":true,"tiers":[{"id":"a-tier","min":0,"max":0},{"id":"z_TIER","min":2,"max":16}]})
}
fn frame(v: &Value) -> Vec<u8> {
    let mut b = serde_json::to_vec(v).unwrap();
    b.push(b'\n');
    b
}
fn refused(out: Output, code: &str) {
    assert!(!out.status.success());
    assert_eq!(out.stderr, format!("m8s: {code}\n").as_bytes());
    if !out.stdout.is_empty() {
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap(),
            json!({"ok":false,"error":code})
        );
    }
    assert!(!String::from_utf8_lossy(&out.stdout).contains("sensitive"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("sensitive"));
}
#[test]
fn safe_projection_text_json_determinism_legacy_and_broken_output() {
    let p = Peer::new();
    let args = p.args();
    let expected = status();
    let out = p.serve(&args, &frame(&expected), false);
    assert!(out.status.success() && out.stderr.is_empty());
    assert!(out.stdout.is_ascii());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        expected
    );
    assert_eq!(p.serve(&args, &frame(&expected), false).stdout, out.stdout);
    assert_eq!(
        p.serve(&args[..3], &frame(&expected), false).stdout,
        out.stdout
    );
    let mut legacy = status();
    legacy["configured"] = json!(false);
    legacy["tiers"] = json!([]);
    assert!(p.serve(&args, &frame(&legacy), false).status.success());
    refused(
        p.serve(&args, &frame(&expected), true),
        "config-status-output-unavailable",
    );
    let mut max = status();
    max["tiers"] = (0..32)
        .map(|n| json!({"id":format!("{n:048}"),"min":16,"max":16}))
        .collect();
    assert!(p.serve(&args, &frame(&max), false).status.success());
}
#[test]
fn arguments_and_help_are_redacted_and_do_not_connect() {
    let p = Peer::new();
    let base = p.args();
    for extra in [
        vec!["sensitive-path"],
        vec!["--config", "sensitive-path"],
        vec!["--json"],
        vec!["--help"],
    ] {
        let mut args = base.clone();
        args.extend(extra.into_iter().map(OsString::from));
        refused(
            Client::spawn(&args, false).output(),
            "invalid-config-status-arguments",
        );
        assert_eq!(
            p.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    for args in [
        vec!["config-status".into()],
        vec!["config-status".into(), "--json".into()],
        vec!["config-status".into(), "--help".into(), "--json".into()],
    ] {
        refused(
            Client::spawn(&args, false).output(),
            "invalid-config-status-arguments",
        );
        assert_eq!(
            p.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    let mut invalid = base;
    invalid[1] = OsString::from_vec(vec![255]);
    refused(Client::spawn(&invalid, false).output(), "invalid arguments");
    let help = Client::spawn(&["config-status".into(), "--help".into()], false).output();
    assert!(help.status.success() && help.stderr.is_empty());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("not desired state")
    );
    assert_eq!(
        p.listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
#[test]
fn untrusted_semantics_schema_labels_controls_and_frames_refuse() {
    let p = Peer::new();
    let args = p.args();
    let mut cases = vec![];
    for (key, value) in [
        ("mode", json!("sensitive")),
        ("scope", json!("sensitive")),
        ("path", json!("sensitive")),
        ("configured", json!(false)),
        ("ok", json!(false)),
        ("error", json!("sensitive")),
    ] {
        let mut v = status();
        v[key] = value;
        cases.push(frame(&v));
    }
    for id in [
        "",
        "a.b",
        "sensitive\u{1b}[31m",
        "sensitive\u{9b}",
        &"x".repeat(49),
    ] {
        let mut v = status();
        v["tiers"][0]["id"] = json!(id);
        cases.push(frame(&v));
    }
    for (key, value) in [
        ("min", json!(-1)),
        ("max", json!(17)),
        ("min", json!(1)),
        ("min", json!("sensitive")),
        ("path", json!("sensitive")),
    ] {
        let mut v = status();
        v["tiers"][0][key] = value;
        cases.push(frame(&v));
    }
    let mut missing = status();
    missing.as_object_mut().unwrap().remove("error");
    cases.push(frame(&missing));
    let mut v = status();
    v["tiers"][1]["id"] = json!("a-tier");
    cases.push(frame(&v));
    let mut v = status();
    v["tiers"].as_array_mut().unwrap().reverse();
    cases.push(frame(&v));
    let mut v = status();
    v["tiers"] = json!([]);
    cases.push(frame(&v));
    let mut v = status();
    v["tiers"] = (0..33)
        .map(|n| json!({"id":format!("{n:048}"),"min":0,"max":0}))
        .collect();
    cases.push(frame(&v));
    cases.extend([
        b"{\"ok\":true,\"ok\":false}\n".to_vec(),
        b"{}".to_vec(),
        vec![b'x'; 65537],
        b"sensitive-not-json\n".to_vec(),
    ]);
    for reply in cases {
        refused(p.serve(&args, &reply, false), "config-status-unavailable");
    }
    let failed = json!({"ok":false,"mode":"mock-only","scope":"mock-control-admission-only","error":"sensitive-remote-error","configured":false,"tiers":[]});
    refused(
        p.serve(&args, &frame(&failed), false),
        "config-status-refused",
    );
}
