#![cfg(unix)]
//! Public binary, only own bounded synthetic Unix peer; no daemon/provider.
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{ffi::OsStringExt, net::UnixListener},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};
struct OwnedChild(Option<Child>, std::fs::File, std::fs::File);
impl OwnedChild {
    fn spawn(args: &[OsString]) -> Self {
        // Regular anonymous files avoid pipe-capacity deadlocks while keeping
        // direct child waits and retained ownership bounded on every path.
        let stdout = tempfile::tempfile().unwrap();
        let stderr = tempfile::tempfile().unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_m8s"))
            .args(args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(stdout.try_clone().unwrap())
            .stderr(stderr.try_clone().unwrap())
            .spawn()
            .unwrap();
        Self(Some(child), stdout, stderr)
    }
    fn output(mut self) -> Output {
        let deadline = Instant::now() + Duration::from_secs(6);
        let status = loop {
            if let Some(status) = self.0.as_mut().unwrap().try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "owned CLI exit deadline");
            std::thread::sleep(Duration::from_millis(5));
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        self.1.seek(SeekFrom::Start(0)).unwrap();
        self.2.seek(SeekFrom::Start(0)).unwrap();
        (&self.1)
            .take((m8s_api::MAX_FRAME + 1) as u64)
            .read_to_end(&mut stdout)
            .unwrap();
        (&self.2).take(4096).read_to_end(&mut stderr).unwrap();
        assert!(stdout.len() <= m8s_api::MAX_FRAME);
        assert!(stderr.len() < 4096);
        Output {
            status,
            stdout,
            stderr,
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(c) = self.0.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
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
            .prefix("m8lp-")
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
            "logs".into(),
            "--after".into(),
            "0".into(),
            "--limit".into(),
            "2".into(),
            "--json".into(),
        ]
    }
    fn serve(&self, args: &[OsString], reply: &[u8]) -> Output {
        let child = OwnedChild::spawn(args);
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut stream = loop {
            match self.listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("{e}"),
            }
            assert!(
                Instant::now() < deadline,
                "explicit synthetic accept deadline"
            );
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
        let request: Value = serde_json::from_slice(&request).unwrap();
        assert_eq!(request["command"], "logs_page");
        let _ = stream.write_all(reply);
        drop(stream);
        child.output()
    }
}
fn page() -> Value {
    json!({"ok":true,"mode":"mock-only","inference":"not-carried","error":null,
    "records":[{"seq":1,"at":-2,"message":"synthetic\u{1b}[31m\n\u{9b}☃"},{"seq":4,"at":5,"message":"synthetic second"}],"next_cursor":4,"has_more":true})
}
fn frame(value: &Value) -> Vec<u8> {
    let mut b = serde_json::to_vec(value).unwrap();
    b.push(b'\n');
    b
}
fn refused(out: Output, code: &str) {
    assert!(!out.status.success());
    assert_eq!(out.stderr, format!("m8s: {code}\n").as_bytes());
    if !out.stdout.is_empty() {
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v, json!({"ok":false,"error":code}));
    }
    for b in [out.stdout, out.stderr] {
        assert!(!String::from_utf8_lossy(&b).contains("sensitive"));
    }
}
#[test]
fn public_text_json_are_same_escaped_projection_and_deterministic() {
    let p = Peer::new();
    let expected = page();
    let args = p.args();
    let out = p.serve(&args, &frame(&expected));
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert!(out.stdout.is_ascii());
    assert!(!out.stdout.contains(&27));
    assert!(!out.stdout.contains(&127));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        expected
    );
    assert_eq!(p.serve(&args, &frame(&expected)).stdout, out.stdout);
    let mut text = args.clone();
    text.pop();
    assert_eq!(p.serve(&text, &frame(&expected)).stdout, out.stdout);
    let mut reordered = args[..3].to_vec();
    reordered.extend([
        "--limit".into(),
        "2".into(),
        "--json".into(),
        "--after".into(),
        "0".into(),
    ]);
    assert_eq!(p.serve(&reordered, &frame(&expected)).stdout, out.stdout);
    let mut end = args;
    end[4] = i64::MAX.to_string().into();
    let response = json!({"ok":true,"mode":"mock-only","inference":"not-carried","error":null,"records":[],"next_cursor":i64::MAX,"has_more":false});
    assert!(p.serve(&end, &frame(&response)).status.success());
}
#[test]
fn grammar_redaction_and_help_never_connect() {
    let p = Peer::new();
    let base = p.args();
    for tail in [
        vec!["--after", "0"],
        vec!["--limit", "1"],
        vec!["--after", "-1", "--limit", "1"],
        vec!["--after", "+1", "--limit", "1"],
        vec!["--after", " 1", "--limit", "1"],
        vec!["--after", "1e1", "--limit", "1"],
        vec!["--after", "9223372036854775808", "--limit", "1"],
        vec!["--after", "18446744073709551616", "--limit", "1"],
        vec!["--after", "0", "--limit", "0"],
        vec!["--after", "0", "--limit", "201"],
        vec!["--after", "0", "--limit", "1", "--limit", "2"],
        vec!["--after", "0", "--after", "0", "--limit", "1"],
        vec!["--after", "0", "--limit", "1", "--json", "--json"],
        vec!["sensitive-unknown"],
    ] {
        let mut args = base[..3].to_vec();
        args.extend(tail.into_iter().map(OsString::from));
        refused(
            OwnedChild::spawn(&args).output(),
            "invalid-log-page-arguments",
        );
        assert_eq!(
            p.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    let mut invalid = base;
    invalid[4] = OsString::from_vec(vec![0xff]);
    refused(OwnedChild::spawn(&invalid).output(), "invalid arguments");
    let out = OwnedChild::spawn(&["logs".into(), "--help".into()]).output();
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert!(String::from_utf8(out.stdout).unwrap().contains("--after"));
    assert_eq!(
        p.listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
#[test]
fn malformed_untrusted_or_oversized_replies_are_fixed_errors() {
    let p = Peer::new();
    let args = p.args();
    let mut cases = vec![];
    for (key, value) in [
        ("mode", json!("sensitive-mode")),
        ("next_cursor", json!(3)),
        ("has_more", json!(false)),
        ("unknown", json!(true)),
    ] {
        // has_more=false is valid; test an empty-but-more response instead.
        if key == "has_more" {
            continue;
        }
        let mut v = page();
        v[key] = value;
        cases.push(frame(&v));
    }
    let mut v = page();
    v["records"][1]["seq"] = json!(1);
    cases.push(frame(&v));
    let mut v = page();
    v["records"][0]["message"] = json!("sensitive".repeat(513));
    cases.push(frame(&v));
    let mut v = page();
    v["records"] = json!([]);
    cases.push(frame(&v));
    let mut v = page();
    v["records"][0]["extra"] = json!(true);
    cases.push(frame(&v));
    let mut v = page();
    v["records"][0]["at"] = json!("sensitive-time");
    cases.push(frame(&v));
    let mut v = page();
    v["records"]
        .as_array_mut()
        .unwrap()
        .push(json!({"seq":5,"at":0,"message":"extra"}));
    cases.push(frame(&v));
    cases.extend([
        b"sensitive-not-json\n".to_vec(),
        b"{}".to_vec(),
        vec![b'x'; m8s_api::MAX_FRAME + 1],
        b"{\"ok\":true,\"ok\":false}\n".to_vec(),
    ]);
    for reply in cases {
        refused(p.serve(&args, &reply), "log-page-unavailable");
    }
    let refused_page = json!({"ok":false,"mode":"mock-only","inference":"not-carried","error":"sensitive-private-error","records":[],"next_cursor":0,"has_more":false});
    refused(p.serve(&args, &frame(&refused_page)), "log-page-refused");
    // Accepted wire strings can expand under safe ASCII projection. Refuse
    // BEFORE output, never emit a partial page or advance its cursor.
    let mut large = page();
    for r in large["records"].as_array_mut().unwrap() {
        r["message"] = json!("\u{9b}".repeat(2048));
    }
    // 2*4096 byte messages expand to only 24KiB, still within the output cap.
    let parsed: m8s_api::LogPageResponse = serde_json::from_slice(&frame(&large)).unwrap();
    assert_eq!(
        parsed
            .records
            .iter()
            .map(|r| r.message.len())
            .collect::<Vec<_>>(),
        vec![4096, 4096]
    );
    let out = p.serve(&args, &frame(&large));
    assert!(out.status.success(), "{out:?}");
    let mut ten = args;
    ten[6] = "10".into();
    large["records"] = (1..=10)
        .map(|seq| json!({"seq":seq,"at":0,"message":"\u{9b}".repeat(2048)}))
        .collect();
    large["next_cursor"] = json!(10);
    refused(p.serve(&ten, &frame(&large)), "log-page-output-unavailable");
}
