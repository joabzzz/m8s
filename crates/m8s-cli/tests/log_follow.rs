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
            .prefix("m8lf-")
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
    fn args(&self, pages: u32) -> Vec<OsString> {
        vec![
            "--socket".into(),
            self.path.clone().into(),
            "logs".into(),
            "--follow".into(),
            "--after".into(),
            "7".into(),
            "--limit".into(),
            "2".into(),
            "--poll-ms".into(),
            "50".into(),
            "--max-pages".into(),
            pages.to_string().into(),
            "--json".into(),
        ]
    }
    fn step(&self, cursor: u64, limit: u32, reply: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut stream = loop {
            match self.listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => panic!("synthetic accept"),
            };
            assert!(Instant::now() < deadline, "synthetic accept deadline");
            std::thread::sleep(Duration::from_millis(2));
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let request = m8s_api::read_frame(&mut stream).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&request).unwrap(),
            json!({"command":"logs_page","after":cursor,"limit":limit})
        );
        let _ = stream.write_all(reply);
    }
    fn none(&self) {
        assert_eq!(
            self.listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
fn page(after: u64, events: Vec<Value>, more: bool) -> Value {
    let next = events.last().map_or(after, |e| e["seq"].as_u64().unwrap());
    json!({"ok":true,"mode":"mock-only","inference":"not-carried","error":null,"records":events,"next_cursor":next,"has_more":more})
}
fn event(seq: u64, message: &str) -> Value {
    json!({"seq":seq,"at":0,"message":message})
}
fn frame(v: &Value) -> Vec<u8> {
    let mut b = serde_json::to_vec(v).unwrap();
    b.push(b'\n');
    b
}
fn lines(out: &Output) -> Vec<Value> {
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty());
    assert!(out.stdout.is_ascii());
    out.stdout
        .split(|b| *b == b'\n')
        .filter(|s| !s.is_empty())
        .map(|s| serde_json::from_slice(s).unwrap())
        .collect()
}
fn refused(out: Output, code: &str) {
    assert!(!out.status.success());
    assert_eq!(out.stderr, format!("m8s: {code}\n").as_bytes());
}
#[test]
fn follows_exact_cursors_drains_more_and_emits_safe_checkpoints() {
    let p = Peer::new();
    let args = p.args(4);
    let child = Client::spawn(&args, false);
    let pages = [
        page(
            7,
            vec![event(8, "\u{1b}[31m\u{202e}é😀\u{7f}"), event(9, "b")],
            true,
        ),
        page(9, vec![event(10, "c")], false),
        page(10, vec![], false),
        page(10, vec![event(11, "d")], false),
    ];
    for (cursor, v) in [7, 9, 10, 10].into_iter().zip(&pages) {
        p.step(cursor, 2, &frame(v));
    }
    let out = child.output();
    assert_eq!(lines(&out), pages);
    p.none();
    assert!(!out.stdout.contains(&0x1b));
    assert!(!out.stdout.contains(&0x7f));
    // Text/default mode is the same NDJSON, even for repeated empty checkpoints.
    let mut args = p.args(2);
    args.pop();
    let child = Client::spawn(&args, false);
    let empty = page(7, vec![], false);
    p.step(7, 2, &frame(&empty));
    p.step(7, 2, &frame(&empty));
    assert_eq!(lines(&child.output()), vec![empty.clone(), empty]);
    p.none();
}
#[test]
fn finite_page_cap_stops_immediately_without_poll_or_extra_fetch() {
    let p = Peer::new();
    let mut args = p.args(1);
    args[9] = "60000".into();
    let child = Client::spawn(&args, false);
    let v = page(7, vec![event(8, "a"), event(9, "b")], true);
    p.step(7, 2, &frame(&v));
    assert_eq!(lines(&child.output()), vec![v]);
    p.none();
    // With a huge poll interval, has_more pages must drain without sleeping.
    let mut args = p.args(3);
    args[9] = "60000".into();
    let child = Client::spawn(&args, false);
    for (cursor, v) in [
        (7, page(7, vec![event(8, "a"), event(9, "b")], true)),
        (9, page(9, vec![event(10, "c"), event(11, "d")], true)),
        (11, page(11, vec![], false)),
    ] {
        p.step(cursor, 2, &frame(&v));
    }
    assert_eq!(lines(&child.output()).len(), 3);
    p.none();
}
#[test]
fn first_failure_or_broken_output_stops_without_another_request() {
    let p = Peer::new();
    let failed = json!({"ok":false,"mode":"mock-only","inference":"not-carried","error":"sensitive","records":[],"next_cursor":7,"has_more":false});
    for (reply, code) in [
        (vec![], "log-follow-unavailable"),
        (b"sensitive\n".to_vec(), "log-follow-unavailable"),
        (frame(&failed), "log-follow-refused"),
        (frame(&page(8, vec![], false)), "log-follow-unavailable"),
    ] {
        let child = Client::spawn(&p.args(3), false);
        p.step(7, 2, &reply);
        let out = child.output();
        assert!(out.stdout.is_empty());
        refused(out, code);
        p.none();
    }
    let child = Client::spawn(&p.args(3), true);
    p.step(7, 2, &frame(&page(7, vec![], false)));
    refused(child.output(), "log-follow-output-unavailable");
    p.none();
    let child = Client::spawn(&p.args(3), false);
    let good = page(7, vec![event(8, "a"), event(9, "b")], true);
    p.step(7, 2, &frame(&good));
    p.step(9, 2, b"broken\n");
    let out = child.output();
    assert_eq!(serde_json::from_slice::<Value>(&out.stdout).unwrap(), good);
    refused(out, "log-follow-unavailable");
    p.none();
}
#[test]
fn encoding_expansion_refuses_entire_page_without_advancing() {
    let p = Peer::new();
    let mut args = p.args(3);
    args[7] = "20".into();
    let child = Client::spawn(&args, false);
    let v = page(
        7,
        (8..22).map(|n| event(n, &"é".repeat(2000))).collect(),
        false,
    );
    let b = frame(&v);
    assert!(b.len() < m8s_api::MAX_FRAME);
    p.step(7, 20, &b);
    let out = child.output();
    assert!(out.stdout.is_empty());
    refused(out, "log-follow-output-unavailable");
    p.none();
}
#[test]
fn strict_flags_utf8_and_help_do_not_connect() {
    let p = Peer::new();
    for extra in [
        vec!["--follow"],
        vec!["--json"],
        vec!["--after", "8"],
        vec!["--poll-ms", "50"],
        vec!["--max-pages", "1"],
        vec!["sensitive"],
        vec!["--help"],
    ] {
        let mut args = p.args(1);
        args.extend(extra.into_iter().map(OsString::from));
        refused(
            Client::spawn(&args, false).output(),
            "invalid-log-follow-arguments",
        );
        p.none();
    }
    for (index, bad) in [
        (5, "9223372036854775808"),
        (7, "0"),
        (7, "201"),
        (9, "49"),
        (9, "60001"),
        (11, "0"),
        (11, "10001"),
        (11, "+1"),
        (11, " 1"),
        (11, "18446744073709551616"),
    ] {
        let mut args = p.args(1);
        args[index] = bad.into();
        refused(
            Client::spawn(&args, false).output(),
            "invalid-log-follow-arguments",
        );
        p.none();
    }
    for range in [4..6, 6..8] {
        let mut args = p.args(1);
        args.drain(range);
        refused(
            Client::spawn(&args, false).output(),
            "invalid-log-follow-arguments",
        );
        p.none();
    }
    let mut args = p.args(1);
    args[1] = OsString::from_vec(vec![255]);
    refused(Client::spawn(&args, false).output(), "invalid arguments");
    p.none();
    let out = Client::spawn(&["logs".into(), "--follow".into(), "--help".into()], false).output();
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("NOT a session deadline")
    );
    p.none();
}
