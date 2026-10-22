#![cfg(unix)]
//! HTTP contract fixtures are NOT real inference or secure mesh qualification.
mod support;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    time::{Duration, Instant},
};
use support::Client;
struct Peer {
    dir: tempfile::TempDir,
    listener: TcpListener,
}
impl Peer {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("prompt.txt"), "Say hello.").unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self { dir, listener }
    }
    fn args(&self) -> Vec<OsString> {
        vec![
            "prototype".into(),
            "request".into(),
            "--local-qualification".into(),
            "--endpoint".into(),
            format!(
                "http://127.0.0.1:{}",
                self.listener.local_addr().unwrap().port()
            )
            .into(),
            "--model".into(),
            "fixture-model".into(),
            "--prompt-file".into(),
            self.dir.path().join("prompt.txt").into_os_string(),
            "--max-tokens".into(),
            "8".into(),
            "--timeout-ms".into(),
            "1000".into(),
            "--json".into(),
        ]
    }
    fn accept(&self) -> TcpStream {
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            match self.listener.accept() {
                Ok((s, _)) => {
                    s.set_nonblocking(true).unwrap();
                    return s;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => panic!("own HTTP accept"),
            };
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn no_more(&self) {
        assert_eq!(
            self.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    fn exchange(&self, response: &[u8], broken: bool) -> std::process::Output {
        let child = Client::spawn(&self.args(), broken);
        let mut stream = self.accept();
        request(&mut stream);
        send(&mut stream, response);
        drop(stream);
        let out = child.output();
        self.no_more();
        out
    }
}
fn request(s: &mut TcpStream) {
    let end = Instant::now() + Duration::from_secs(2);
    let mut bytes = vec![];
    loop {
        let mut chunk = [0; 2048];
        match s.read(&mut chunk) {
            Ok(0) => panic!("own request EOF"),
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2))
            }
            Err(_) => panic!("own request read"),
        };
        assert!(Instant::now() < end && bytes.len() < 65536);
        if let Some(pos) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            let header = std::str::from_utf8(&bytes[..pos]).unwrap();
            assert!(header.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
            assert!(!header.to_lowercase().contains("authorization"));
            let len = header
                .lines()
                .find_map(|s| s.strip_prefix("Content-Length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            if bytes.len() == pos + 4 + len {
                let value: Value = serde_json::from_slice(&bytes[pos + 4..]).unwrap();
                assert_eq!(
                    value,
                    json!({"model":"fixture-model","messages":[{"role":"user","content":"Say hello."}],"stream":false,"max_tokens":8})
                );
                return;
            }
        }
    }
}
fn send(s: &mut TcpStream, bytes: &[u8]) {
    let end = Instant::now() + Duration::from_secs(2);
    let mut offset = 0;
    while offset < bytes.len() {
        match s.write(&bytes[offset..]) {
            Ok(0) => break,
            Ok(n) => offset += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2))
            }
            Err(_) => break,
        };
        assert!(Instant::now() < end);
    }
}
fn completion() -> Value {
    json!({"id":"fixture","model":"fixture-model","choices":[{"index":0,"message":{"role":"assistant","content":"hello é\u{202e}\n"},"finish_reason":"stop"}]})
}
fn frame(body: &[u8]) -> Vec<u8> {
    let mut b = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    b.extend_from_slice(body);
    b
}
fn refusal(out: std::process::Output, code: &str) {
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(out.stderr, format!("m8s: {code}\n").as_bytes());
}
#[test]
fn actual_request_shape_final_buffered_close_and_ascii_projection() {
    let p = Peer::new();
    let response = frame(&serde_json::to_vec(&completion()).unwrap());
    let a = p.exchange(&response, false);
    assert!(a.status.success());
    assert!(a.stderr.is_empty());
    assert!(a.stdout.is_ascii());
    let v: Value = serde_json::from_slice(&a.stdout).unwrap();
    assert_eq!(
        v["content"],
        completion()["choices"][0]["message"]["content"]
    );
    assert_eq!(v["identity"], "not-verified");
    assert_eq!(v["readiness"], "not-assessed");
    assert_eq!(v["owner_routing"], "not-verified");
    assert_eq!(v["launch_performed"], false);
    assert_eq!(p.exchange(&response, false).stdout, a.stdout);
    let mut args = p.args();
    args.pop();
    let child = Client::spawn(&args, false);
    let mut s = p.accept();
    request(&mut s);
    send(&mut s, &response);
    drop(s);
    let text = child.output();
    assert!(text.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&text.stdout).unwrap(), v);
    p.no_more();
}
#[test]
fn malformed_protocol_wrong_model_errors_and_unfinished_output_are_not_completions() {
    let p = Peer::new();
    for body in [
        json!({"error":{"message":"private\u{1b}"}}),
        json!({"model":"other","choices":[]}),
        json!({"model":"fixture-model","choices":[{"index":0,"message":{"role":"assistant","content":""},"finish_reason":"stop"}]}),
    ] {
        refusal(
            p.exchange(&frame(&serde_json::to_vec(&body).unwrap()), false),
            "prototype-response-unavailable",
        );
    }
    for raw in [
        b"not-json".as_slice(),
        br#"{"model":"other","model":"fixture-model","choices":[]}"#,
    ] {
        refusal(
            p.exchange(&frame(raw), false),
            "prototype-response-unavailable",
        );
    }
    for response in [b"HTTP/1.1 302 Redirect\r\nContent-Length: 0\r\n\r\n".as_slice(),b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n",b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx",b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 65537\r\n\r\n",b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{}"] {refusal(p.exchange(response,false),"prototype-request-outcome-unknown");}
}
#[test]
fn strict_help_arguments_utf8_and_prompt_files_have_no_network_effects() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let p = Peer::new();
    let help = Client::spawn(
        &["prototype".into(), "request".into(), "--help".into()],
        false,
    )
    .output();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("not verified identity")
    );
    assert!(help.stderr.is_empty());
    for (index, value) in [
        (2, "--unknown-private"),
        (4, "http://localhost:9337"),
        (4, "http://127.0.0.1:1/path"),
        (4, "http://127.0.0.2:9337"),
        (6, "bad\nmodel"),
        (8, "relative"),
        (10, "129"),
        (10, "+1"),
        (12, "0"),
    ] {
        let mut a = p.args();
        a[index] = value.into();
        refusal(
            Client::spawn(&a, false).output(),
            "invalid-prototype-request-arguments",
        );
        p.no_more();
    }
    let mut a = p.args();
    a.push("--json".into());
    refusal(
        Client::spawn(&a, false).output(),
        "invalid-prototype-request-arguments",
    );
    let mut a = p.args();
    a.push(OsString::from_vec(vec![0xff]));
    refusal(Client::spawn(&a, false).output(), "invalid arguments");
    for path in [
        p.dir.path().join("missing"),
        p.dir.path().join("link"),
        p.dir.path().to_path_buf(),
    ] {
        if path.ends_with("link") {
            symlink(p.dir.path().join("prompt.txt"), &path).unwrap();
        }
        let mut a = p.args();
        a[8] = path.into_os_string();
        refusal(
            Client::spawn(&a, false).output(),
            "prototype-prompt-unavailable",
        );
        p.no_more();
    }
    std::fs::write(p.dir.path().join("prompt.txt"), vec![b'x'; 8193]).unwrap();
    refusal(
        Client::spawn(&p.args(), false).output(),
        "prototype-prompt-unavailable",
    );
    p.no_more();
}
#[test]
fn single_deadline_stall_and_drip_fail_without_retry_and_broken_output_is_unknown() {
    for drip in [false, true] {
        let p = Peer::new();
        let mut args = p.args();
        args[12] = "100".into();
        let child = Client::spawn(&args, false);
        let mut s = p.accept();
        request(&mut s);
        if drip {
            for _ in 0..10 {
                send(&mut s, b"H");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        refusal(child.output(), "prototype-request-outcome-unknown");
        drop(s);
        p.no_more();
    }
    let p = Peer::new();
    refusal(
        p.exchange(&frame(&serde_json::to_vec(&completion()).unwrap()), true),
        "prototype-output-unavailable",
    );
}

#[test]
fn bounded_header_body_and_expansion_fail_without_partial_completion_output() {
    let p = Peer::new();
    let huge = format!("HTTP/1.1 200 OK\r\nX-Padding: {}", "x".repeat(8192));
    refusal(
        p.exchange(huge.as_bytes(), false),
        "prototype-request-outcome-unknown",
    );
    let mut c = completion();
    c["choices"][0]["message"]["content"] = json!("x".repeat(32769));
    refusal(
        p.exchange(&frame(&serde_json::to_vec(&c).unwrap()), false),
        "prototype-response-unavailable",
    );
    c["choices"][0]["message"]["content"] = json!("é".repeat(16000));
    refusal(
        p.exchange(&frame(&serde_json::to_vec(&c).unwrap()), false),
        "prototype-output-unavailable",
    );
}
