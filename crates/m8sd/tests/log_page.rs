#![cfg(unix)]
use m8s_api::{LogEvent, Request};
use m8s_provider::Mock;
use m8sd::Store;
use std::{
    fs,
    io::Write,
    os::unix::{fs::DirBuilderExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let p = fs::canonicalize("/tmp").unwrap().join(format!(
            "m8lp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        Self(p)
    }
    fn inventory(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = fs::read_dir(&self.0)
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                let bytes = fs::read(&p).unwrap();
                (p, bytes)
            })
            .collect::<Vec<_>>();
        files.sort();
        files
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn history(store: &Store) -> Vec<LogEvent> {
    let mut cursor = 0;
    let mut all = Vec::new();
    // Fixed test bound, not a product follow/poll API.
    for _ in 0..100 {
        let (page, more) = store.log_page(cursor, 2).unwrap();
        for e in &page {
            assert!(e.seq as u64 > cursor);
            cursor = e.seq as u64;
        }
        all.extend(page);
        if !more {
            return all;
        }
    }
    panic!("unexpected synthetic history size")
}
#[test]
fn store_pages_are_readonly_ordered_snapshot_and_retained_on_reopen() {
    let root = Root::new();
    let path = root.0.join("state.db");
    let mut store = Store::open(&path, "synthetic-log-page-owner").unwrap();
    let mut mock = Mock::default();
    store.scale("test", 2).unwrap();
    store.reconcile(&mut mock, 100).unwrap();
    store.down().unwrap();
    store.reconcile(&mut mock, 101).unwrap();
    store.reconcile(&mut mock, 102).unwrap();
    let old = store.logs().unwrap();
    // Ordinary WAL SELECTs may establish shared-memory reader marks. Warm the
    // existing legacy SELECT before this fixture's byte-stability comparison;
    // paging is not the immutable/sidecar-free historical inspector contract.
    let before = root.inventory();
    let all = history(&store);
    assert!(all.len() > 2);
    assert_eq!(
        all.iter().map(|e| e.message.clone()).collect::<Vec<_>>(),
        old
    );
    assert!(all.iter().any(|e| e.at == 100));
    let (page, more) = store.log_page(0, 1).unwrap();
    assert_eq!(page, all[..1]);
    assert!(more);
    let end = all.last().unwrap().seq as u64;
    assert_eq!(store.log_page(end, 200).unwrap(), (vec![], false));
    assert_eq!(
        store.log_page(i64::MAX as u64, 200).unwrap(),
        (vec![], false)
    );
    for (cursor, limit) in [(0, 0), (0, 201), (i64::MAX as u64 + 1, 1), (u64::MAX, 1)] {
        assert!(store.log_page(cursor, limit).is_err());
    }
    assert!(
        root.inventory() == before,
        "own warmed Store inventory changed"
    );
    assert_eq!(store.logs().unwrap(), old);
    drop(store);
    let reopened = Store::open(&path, "synthetic-log-page-owner").unwrap();
    assert_eq!(history(&reopened), all);
}
struct Daemon(Child);
impl Daemon {
    fn start(root: &Root) -> Self {
        let mut d = Self(
            Command::new(env!("CARGO_BIN_EXE_m8sd"))
                .arg(&root.0)
                .arg("synthetic-log-page-owner")
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            assert!(d.0.try_wait().unwrap().is_none());
            if m8s_api::call(&root.0.join("control.sock"), &Request::Status {}).is_ok() {
                return d;
            }
            assert!(Instant::now() < deadline, "startup deadline");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn shutdown(&mut self, root: &Root) {
        assert!(
            m8s_api::call(&root.0.join("control.sock"), &Request::Shutdown {})
                .unwrap()
                .ok
        );
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            assert!(Instant::now() < deadline, "exit deadline");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn public_daemon_page_protocol_refusal_legacy_and_restart() {
    let root = Root::new();
    let mut d = Daemon::start(&root);
    let path = root.0.join("control.sock");
    let empty = m8s_api::call_log_page(&path, 0, 200).unwrap();
    assert!(empty.ok);
    assert!(empty.records.is_empty());
    assert_eq!(empty.next_cursor, 0);
    assert!(!empty.has_more);
    assert!(
        m8s_api::call(
            &path,
            &Request::Scale {
                tier: "test".into(),
                replicas: 1
            }
        )
        .unwrap()
        .ok
    );
    // Only ordinary timer reconciliation generates this synthetic history.
    let deadline = Instant::now() + Duration::from_secs(6);
    let first = loop {
        let page = m8s_api::call_log_page(&path, 0, 1).unwrap();
        if !page.records.is_empty() {
            break page;
        }
        assert!(Instant::now() < deadline, "history creation deadline");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(first.ok);
    assert_eq!(first.records.len(), 1);
    assert_eq!(first.next_cursor, first.records[0].seq as u64);
    let legacy = m8s_api::call(&path, &Request::Logs {}).unwrap();
    assert!(legacy.ok);
    assert_eq!(legacy.events[0], first.records[0].message);
    for frame in [
        r#"{"command":"logs_page","after":0,"limit":0}"#,
        r#"{"command":"logs_page","after":18446744073709551615,"limit":1}"#,
        r#"{"command":"logs_page","after":0,"limit":201}"#,
    ] {
        let mut s = UnixStream::connect(&path).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        s.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        writeln!(s, "{frame}").unwrap();
        let raw = m8s_api::read_frame(&mut s).unwrap();
        let r: m8s_api::LogPageResponse = serde_json::from_slice(&raw).unwrap();
        assert!(!r.ok);
        assert_eq!(r.error.as_deref(), Some("log-page-unavailable"));
        assert!(r.records.is_empty());
        assert!(!r.has_more);
    }
    assert_eq!(
        m8s_api::call(&path, &Request::Status {}).unwrap().tiers,
        vec![("test".into(), 1)]
    );
    d.shutdown(&root);
    let stored = Store::open(root.0.join("state.db"), "synthetic-log-page-owner").unwrap();
    let all = history(&stored);
    drop(stored);
    let mut restarted = Daemon::start(&root);
    let page = m8s_api::call_log_page(&path, 0, 200).unwrap();
    assert!(page.ok);
    assert_eq!(page.records, all);
    assert!(!page.has_more);
    let end = m8s_api::call_log_page(&path, page.next_cursor, 1).unwrap();
    assert!(end.ok);
    assert!(end.records.is_empty());
    assert_eq!(end.next_cursor, page.next_cursor);
    restarted.shutdown(&root);
}
