//! Own finite synthetic Unix peers only; no daemon/provider or peer-trust claims.
use m8s_api::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    os::unix::{fs::DirBuilderExt, net::UnixListener},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Peer {
    root: PathBuf,
    listener: UnixListener,
    path: PathBuf,
}
impl Peer {
    fn new() -> Self {
        let root = fs::canonicalize("/tmp").unwrap().join(format!(
            "m8api-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let path = root.join("sensitive");
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self {
            root,
            listener,
            path,
        }
    }
    fn serve<T>(
        &self,
        reply: Vec<u8>,
        call: impl FnOnce(&Path) -> io::Result<T>,
    ) -> (io::Result<T>, Value) {
        std::thread::scope(|scope| {
            let server = scope.spawn(|| {
                let end = Instant::now() + Duration::from_secs(4);
                let mut stream = loop {
                    match self.listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(_) => panic!("synthetic accept failed"),
                    }
                    assert!(Instant::now() < end, "synthetic accept deadline");
                    std::thread::sleep(Duration::from_millis(1));
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let request =
                    serde_json::from_slice::<Value>(&read_frame(&mut stream).unwrap()).unwrap();
                let _ = stream.write_all(&reply);
                drop(stream);
                request
            });
            let result = call(&self.path);
            let request = server.join().unwrap();
            assert_eq!(
                self.listener.accept().unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            ); // no reconnect
            (result, request)
        })
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn frame(value: Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    bytes
}
fn status() -> Value {
    json!({"ok":true,"mode":"mock-only","nodes":[],"tiers":[],"events":[],"error":null,"secure_bootstrap":"unavailable"})
}
#[test]
fn all_four_public_calls_use_one_request_and_accept_final_buffered_close() {
    let p = Peer::new();
    let (r, q) = p.serve(frame(status()), |path| call(path, &Request::Status {}));
    assert!(r.unwrap().ok);
    assert_eq!(q, json!({"command":"status"}));
    let mut large = frame(status());
    large.pop();
    large.resize(MAX_FRAME - 1, b' ');
    large.push(b'\n');
    assert!(
        p.serve(large, |path| call(path, &Request::Status {}))
            .0
            .unwrap()
            .ok
    );
    let page = json!({"ok":true,"mode":"mock-only","inference":"not-carried","error":null,"records":[],"next_cursor":7,"has_more":false});
    let (r, q) = p.serve(frame(page), |path| call_log_page(path, 7, 2));
    assert_eq!(r.unwrap().next_cursor, 7);
    assert_eq!(q, json!({"command":"logs_page","after":7,"limit":2}));
    let config = json!({"ok":true,"mode":"mock-only","scope":"mock-control-admission-only","error":null,"configured":false,"tiers":[]});
    let (r, q) = p.serve(frame(config), call_config_status);
    assert!(!r.unwrap().configured);
    assert_eq!(q, json!({"command":"config_status"}));
    let check = json!({"schema":"m8s.reload-check/v1","mode":"mock-only","scope":"mock-control-admission-only","ok":true,"error":null,"check_only":true,"applied":false,"later_reload_guaranteed":false,"active":[{"id":"a","min":0,"max":1}],"candidate":[{"id":"a","min":0,"max":1}],"bounds_changed":false});
    let (r, q) = p.serve(frame(check), call_reload_check);
    assert_eq!(r.unwrap().bounds_changed, Some(false));
    assert_eq!(q, json!({"command":"reload_config_check"}));
}
#[test]
fn malformed_partial_eof_and_reply_loss_never_resend_mutations() {
    let p = Peer::new();
    for reply in [
        vec![],
        b"{\"ok\":true".to_vec(),
        b"sensitive-malformed\n".to_vec(),
        vec![b'x'; MAX_FRAME + 1],
    ] {
        let (r, q) = p.serve(reply, |path| call(path, &Request::Down {}));
        let e = r.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(!e.to_string().contains("sensitive"));
        assert_eq!(q, json!({"command":"down"}));
    }
    // Match legacy server read_frame: first complete frame, suffix ignored.
    let mut bytes = frame(status());
    bytes.extend(b"ignored suffix");
    assert!(
        p.serve(bytes, |path| call(path, &Request::Status {}))
            .0
            .is_ok()
    );
}
#[test]
fn oversized_request_refuses_before_connect_and_errors_are_static() {
    let p = Peer::new();
    let e = call(
        &p.path,
        &Request::Up {
            tier: "sensitive".repeat(MAX_FRAME),
        },
    )
    .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    assert_eq!(e.to_string(), "invalid control frame");
    assert_eq!(
        p.listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    let e = call(&p.root.join("sensitive-missing"), &Request::Status {}).unwrap_err();
    assert!(!e.to_string().contains("sensitive"));
}
