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
            "explain-tier".into(),
            "t".into(),
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
            json!({"command":"explain_tier","tier":"t"})
        );
        let _ = stream.write_all(reply);
        drop(stream);
        child.output()
    }
}
fn response() -> Value {
    json!({"schema":"m8s.explain-tier/v1","mode":"mock-only","scope":"stored-launch-gates-only","ok":true,"error":null,"readiness":"not-assessed","provider_currentness":"not-assessed","authorization":"not-assessed","configured":true,"configured_tier":{"id":"t","min":0,"max":2},"data":{"tier":"t","evaluated_at":3600,"desired":null,"stored_node_count":0,"stored_deleting_count":0,"stored_phases":[],"launches_after_cutoff":0,"future_attempt_present":false,"launch_rate_available":true,"rate_limit":3,"window_seconds":3600,"cutoff_exclusive":0}})
}
fn frame(v: &Value) -> Vec<u8> {
    let mut b = serde_json::to_vec(v).unwrap();
    b.push(b'\n');
    b
}
fn refused(o: Output, code: &str) {
    assert!(!o.status.success());
    assert!(o.stdout.is_empty());
    assert_eq!(o.stderr, format!("m8s: {code}\n").as_bytes());
}
#[test]
fn safe_projection_deterministic_text_json_and_output_failure() {
    let p = Peer::new();
    let args = p.args();
    let v = response();
    let out = p.serve(&args, &frame(&v), false);
    assert!(out.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&out.stdout).unwrap(), v);
    assert_eq!(p.serve(&args, &frame(&v), false).stdout, out.stdout);
    assert_eq!(p.serve(&args[..4], &frame(&v), false).stdout, out.stdout);
    refused(
        p.serve(&args, &frame(&v), true),
        "explain-tier-output-unavailable",
    );
}
#[test]
fn malformed_unsafe_and_incoherent_responses_are_static_failures() {
    let p = Peer::new();
    let args = p.args();
    for (key, bad) in [
        ("mode", json!("sensitive")),
        ("scope", json!("live")),
        ("readiness", json!("Ready")),
        ("configured", json!(false)),
        ("data", Value::Null),
    ] {
        let mut r = response();
        r[key] = bad;
        refused(
            p.serve(&args, &frame(&r), false),
            "explain-tier-unavailable",
        );
    }
    for (key, bad) in [
        ("tier", json!("other")),
        ("desired", json!(17)),
        ("stored_node_count", json!(1)),
        (
            "stored_phases",
            json!([{"phase":"sensitive\u{1b}","count":1}]),
        ),
    ] {
        let mut r = response();
        r["data"][key] = bad;
        refused(
            p.serve(&args, &frame(&r), false),
            "explain-tier-unavailable",
        );
    }
    for raw in [
        b"{}".to_vec(),
        vec![b'x'; 65537],
        b"sensitive-not-json\n".to_vec(),
    ] {
        refused(p.serve(&args, &raw, false), "explain-tier-unavailable");
    }
    let mut r = response();
    r["ok"] = json!(false);
    r["error"] = json!("explain-tier-refused");
    r["configured"] = json!(false);
    r["configured_tier"] = Value::Null;
    r["data"] = Value::Null;
    refused(p.serve(&args, &frame(&r), false), "explain-tier-refused");
}
#[test]
fn strict_arguments_utf8_and_effectfree_help() {
    let p = Peer::new();
    for extra in [
        vec!["--json"],
        vec!["--now", "0"],
        vec!["sensitive"],
        vec!["--help"],
    ] {
        let mut args = p.args();
        args.extend(extra.into_iter().map(OsString::from));
        refused(
            Client::spawn(&args, false).output(),
            "invalid-explain-tier-arguments",
        );
        assert_eq!(
            p.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    for bad in ["", "bad.id", "sensitive\u{1b}", &"x".repeat(65)] {
        let mut args = p.args();
        args[3] = bad.into();
        refused(
            Client::spawn(&args, false).output(),
            "invalid-explain-tier-arguments",
        );
        assert_eq!(
            p.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    let mut args = p.args();
    args[3] = OsString::from_vec(vec![255]);
    refused(Client::spawn(&args, false).output(), "invalid arguments");
    let out = Client::spawn(&["explain-tier".into(), "--help".into()], false).output();
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("NOT overall eligibility")
    );
    assert_eq!(
        p.listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
