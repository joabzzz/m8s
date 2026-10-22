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
            .prefix("m8ba-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = dir.path().canonicalize().unwrap().join("sensitive");
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
            "scale-batch".into(),
            "b=0".into(),
            "a=2".into(),
            "--json".into(),
        ]
    }
    fn serve(&self, reply: &Value, broken: bool) -> Output {
        self.serve_args(&self.args(), reply, broken)
    }
    fn serve_args(&self, args: &[OsString], reply: &Value, broken: bool) -> Output {
        let c = Client::spawn(args, broken);
        let end = Instant::now() + Duration::from_secs(4);
        let mut s = loop {
            match self.listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => panic!("synthetic accept"),
            };
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        };
        s.set_nonblocking(false).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&m8s_api::read_frame(&mut s).unwrap()).unwrap(),
            json!({"command":"scale_batch","targets":[{"tier":"a","replicas":2},{"tier":"b","replicas":0}]})
        );
        let mut bytes = serde_json::to_vec(reply).unwrap();
        bytes.push(b'\n');
        let _ = s.write_all(&bytes);
        drop(s);
        let out = c.output();
        self.none();
        out
    }
    fn none(&self) {
        assert_eq!(
            self.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
fn response() -> Value {
    json!({"ok":true,"mode":"mock-only","nodes":[],"tiers":[["a",2],["b",0],["untouched",1]],"events":["sensitive\u{1b}"],"error":null,"secure_bootstrap":"sensitive"})
}
fn refused(o: Output, code: &str) {
    assert!(!o.status.success());
    assert!(o.stdout.is_empty());
    assert_eq!(o.stderr, format!("m8s: {code}\n").as_bytes());
}
#[test]
fn safe_sorted_projection_omits_untrusted_payload_and_output_failure_is_uncertain() {
    let p = Peer::new();
    let out = p.serve(&response(), false);
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["requested"],
        json!([{"tier":"a","replicas":2},{"tier":"b","replicas":0}])
    );
    assert_eq!(v["observed_desired"].as_array().unwrap().len(), 3);
    assert!(!String::from_utf8_lossy(&out.stdout).contains("sensitive"));
    assert_eq!(p.serve(&response(), false).stdout, out.stdout);
    let mut args = p.args();
    args.pop();
    assert_eq!(p.serve_args(&args, &response(), false).stdout, out.stdout);
    refused(p.serve(&response(), true), "scale-batch-output-unavailable");
}
#[test]
fn response_mismatch_duplicates_controls_and_refusal_do_not_claim_success() {
    let p = Peer::new();
    for (key, v) in [
        ("mode", json!("sensitive")),
        ("tiers", json!([["a", 1], ["b", 0]])),
        ("tiers", json!([["a", 2], ["a", 2], ["b", 0]])),
        ("tiers", json!([["a", 2], ["b", 0], ["sensitive\u{1b}", 1]])),
        ("error", json!("sensitive")),
    ] {
        let mut r = response();
        r[key] = v;
        refused(p.serve(&r, false), "scale-batch-outcome-unknown");
    }
    refused(p.serve(&Value::Null, false), "scale-batch-outcome-unknown");
    let mut r = response();
    r["ok"] = json!(false);
    r["error"] = json!("sensitive");
    refused(p.serve(&r, false), "scale-batch-refused-or-outcome-unknown");
}
#[test]
fn grammar_help_utf8_and_invalid_targets_never_connect() {
    let p = Peer::new();
    for suffix in [
        vec![],
        vec!["a=1", "a=0"],
        vec!["a=1", "bad.id=2"],
        vec!["a=1", "b=17"],
        vec!["a=-1"],
        vec!["a=+1"],
        vec!["a= 1"],
        vec!["a=1e1"],
        vec!["a=1", "--json", "--json"],
        vec!["--all"],
        vec!["a=1=2"],
        vec!["a=18446744073709551616"],
    ] {
        let mut args = p.args();
        args.truncate(3);
        args.extend(suffix.into_iter().map(OsString::from));
        refused(
            Client::spawn(&args, false).output(),
            "invalid-scale-batch-arguments",
        );
        p.none();
    }
    let mut args = p.args();
    args.truncate(3);
    args.extend((0..33).map(|n| format!("tier{n}=0").into()));
    refused(
        Client::spawn(&args, false).output(),
        "invalid-scale-batch-arguments",
    );
    p.none();
    let mut args = p.args();
    args[1] = OsString::from_vec(vec![255]);
    refused(Client::spawn(&args, false).output(), "invalid arguments");
    let out = Client::spawn(&["scale-batch".into(), "--help".into()], false).output();
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("may follow commit")
    );
    p.none();
}
