//! Synthetic only: no keychain drivers, ambient credentials or owner keystore.
use super::*;
use crate::{KeyringDriver, KeyringStore, SecretName, SecretStore, SecretValue};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

fn name(s: &str) -> SecretName {
    SecretName::new(s).unwrap()
}
fn value() -> SecretValue {
    SecretValue::new(b"synthetic-session-only".to_vec())
}
fn fixture() -> (tempfile::TempDir, KeyringIdentity) {
    #[cfg(target_os = "macos")]
    let base = "/private/tmp";
    #[cfg(not(target_os = "macos"))]
    let base = "/tmp";
    let dir = tempfile::Builder::new()
        .prefix("m8s-synthetic-keyring-lock-")
        .tempdir_in(base)
        .unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let identity = KeyringIdentity::new(
        "synthetic-only",
        name(&format!(
            "test/{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )),
    )
    .unwrap();
    (dir, identity)
}
fn domain(dir: &Path, identity: &KeyringIdentity) -> KeyringLockDomain {
    KeyringLockDomain::acknowledge_exclusive_writers(dir, identity.clone()).unwrap()
}
#[derive(Clone)]
struct Memory {
    identity: KeyringIdentity,
    bytes: Arc<Mutex<Option<SecretValue>>>,
    loads: Arc<AtomicUsize>,
}
impl Memory {
    fn new(identity: KeyringIdentity) -> Self {
        Self {
            identity,
            bytes: Arc::new(Mutex::new(None)),
            loads: Arc::new(AtomicUsize::new(0)),
        }
    }
}
impl KeyringDriver for Memory {
    fn identity(&self) -> Option<KeyringIdentity> {
        Some(self.identity.clone())
    }
    fn load(&self) -> Result<Option<SecretValue>, Error> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        let bytes = self
            .bytes
            .lock()
            .unwrap()
            .as_ref()
            .map(|v| SecretValue::new(v.expose().to_vec()));
        // Force a whole-operation race if only load/save were separately locked.
        thread::sleep(Duration::from_millis(2));
        Ok(bytes)
    }
    fn save(&self, value: &SecretValue) -> Result<(), Error> {
        *self.bytes.lock().unwrap() = Some(SecretValue::new(value.expose().to_vec()));
        Ok(())
    }
}
#[test]
fn independent_stores_share_whole_operation_gate_without_lost_records() {
    let (dir, identity) = fixture();
    let driver = Memory::new(identity.clone());
    let mut workers = Vec::new();
    for worker in 0..8 {
        let mut store =
            KeyringStore::coordinated(driver.clone(), domain(dir.path(), &identity)).unwrap();
        workers.push(thread::spawn(move || {
            for record in 0..8 {
                store
                    .put(name(&format!("test/{worker}-{record}")), value(), None)
                    .unwrap();
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    let mut store = KeyringStore::coordinated(driver, domain(dir.path(), &identity)).unwrap();
    assert_eq!(store.list(0).unwrap().len(), 64);
    store.put(name("test/expired"), value(), Some(10)).unwrap();
    assert_eq!(store.list(9).unwrap().len(), 65);
    assert_eq!(store.list(10).unwrap().len(), 64);
    assert!(store.get(&name("test/expired"), 10).unwrap().is_none());
    assert!(store.delete(&name("test/expired")).unwrap());
    assert!(!store.delete(&name("test/expired")).unwrap());
}
#[test]
fn identity_mismatch_uncoordinated_driver_and_live_split_domain_fail_before_load() {
    let (dir, identity) = fixture();
    let (other, _) = fixture();
    let driver = Memory::new(identity.clone());
    let mut uncoordinated = KeyringStore::injected(driver.clone());
    assert_eq!(uncoordinated.list(0), Err(Error::Unavailable));
    assert_eq!(
        uncoordinated.put(name("test/no"), value(), None),
        Err(Error::Unavailable)
    );
    assert_eq!(
        uncoordinated.delete(&name("test/no")),
        Err(Error::Unavailable)
    );
    let lock = domain(dir.path(), &identity);
    assert!(matches!(
        KeyringLockDomain::acknowledge_exclusive_writers(other.path(), identity.clone()),
        Err(Error::UnsafePath)
    ));
    let wrong = KeyringIdentity::new("synthetic-other", name("test/account")).unwrap();
    assert!(KeyringStore::coordinated(driver.clone(), domain(dir.path(), &wrong)).is_err());
    assert_eq!(driver.loads.load(Ordering::SeqCst), 0);
    let same = domain(dir.path(), &identity);
    assert!(Arc::ptr_eq(&lock.process, &same.process));
}
#[test]
fn directories_lock_metadata_links_git_and_path_swaps_are_revalidated() {
    let (dir, identity) = fixture();
    assert!(
        KeyringLockDomain::acknowledge_exclusive_writers(Path::new("relative"), identity.clone())
            .is_err()
    );
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        KeyringLockDomain::acknowledge_exclusive_writers(dir.path(), identity.clone()).is_err()
    );
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let lock = domain(dir.path(), &identity);
    let path = lock.path.clone();
    assert!(fs::read(&path).unwrap().is_empty());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    let driver = Memory::new(identity);
    let store = KeyringStore::coordinated(driver.clone(), lock).unwrap();
    for mode in [0o644, 0o400, 0o700] {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        assert!(store.list(0).is_err());
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.path().join("synthetic-link");
    fs::hard_link(&path, &link).unwrap();
    assert!(store.list(0).is_err());
    fs::remove_file(&link).unwrap();
    fs::write(dir.path().join(".git"), b"synthetic-marker").unwrap();
    assert!(store.list(0).is_err());
    fs::remove_file(dir.path().join(".git")).unwrap();
    let moved = dir.path().with_extension("moved");
    fs::rename(dir.path(), &moved).unwrap();
    fs::create_dir(dir.path()).unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(store.list(0).is_err());
    fs::remove_dir(dir.path()).unwrap();
    fs::rename(moved, dir.path()).unwrap();
    fs::rename(&path, &link).unwrap();
    symlink(&link, &path).unwrap();
    assert!(store.list(0).is_err());
    fs::remove_file(&path).unwrap();
    // A fresh regular replacement is NOT the pinned lock, even with correct permissions.
    fs::write(&path, b"").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(store.list(0).is_err());
    assert_eq!(driver.loads.load(Ordering::SeqCst), 0);
}
#[test]
fn lock_wait_holds_entire_operation_and_rechecks_after_acquire() {
    let (dir, identity) = fixture();
    let lock = domain(dir.path(), &identity);
    let foreign = lock.revalidate().unwrap();
    foreign.lock_exclusive().unwrap();
    let path = lock.path.clone();
    let driver = Memory::new(identity);
    let store = KeyringStore::coordinated(driver.clone(), lock).unwrap();
    let (started, ready) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        started.send(()).unwrap();
        store.list(0)
    });
    ready.recv().unwrap();
    thread::sleep(Duration::from_millis(30));
    assert_eq!(driver.loads.load(Ordering::SeqCst), 0);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    drop(foreign);
    assert!(worker.join().unwrap().is_err());
    assert_eq!(driver.loads.load(Ordering::SeqCst), 0);
}
#[test]
fn poisoned_shared_process_gate_fails_closed_across_new_objects() {
    let (dir, identity) = fixture();
    let lock = domain(dir.path(), &identity);
    let process = lock.process.clone();
    let result = std::panic::catch_unwind(|| {
        let _guard = process.gate.lock().unwrap();
        panic!("synthetic gate poison");
    });
    assert!(result.is_err());
    let driver = Memory::new(identity.clone());
    let store = KeyringStore::coordinated(driver.clone(), domain(dir.path(), &identity)).unwrap();
    assert_eq!(store.list(0), Err(Error::Backend));
    assert_eq!(driver.loads.load(Ordering::SeqCst), 0);
}
#[cfg(target_os = "macos")]
#[test]
fn extended_acl_on_lock_or_directory_is_rejected_before_load() {
    let (dir, identity) = fixture();
    let lock = domain(dir.path(), &identity);
    let path = lock.path.clone();
    let driver = Memory::new(identity);
    let store = KeyringStore::coordinated(driver.clone(), lock).unwrap();
    for path in [&path, &dir.path().to_owned()] {
        assert!(
            Command::new("/bin/chmod")
                .args(["+a", "everyone allow read"])
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
        assert!(store.list(0).is_err());
        assert!(
            Command::new("/bin/chmod")
                .arg("-N")
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }
    assert_eq!(driver.loads.load(Ordering::SeqCst), 0);
}

#[test]
fn backend_errors_are_uncertain_never_absence_and_release_the_lock() {
    use std::sync::atomic::AtomicBool;
    struct FailedRead {
        memory: Memory,
        fail: Arc<AtomicBool>,
    }
    impl KeyringDriver for FailedRead {
        fn identity(&self) -> Option<KeyringIdentity> {
            self.memory.identity()
        }
        fn load(&self) -> Result<Option<SecretValue>, Error> {
            if self.fail.load(Ordering::SeqCst) {
                Err(Error::Unavailable)
            } else {
                self.memory.load()
            }
        }
        fn save(&self, value: &SecretValue) -> Result<(), Error> {
            self.memory.save(value)
        }
    }
    let (dir, identity) = fixture();
    let memory = Memory::new(identity.clone());
    let fail = Arc::new(AtomicBool::new(false));
    let mut store = KeyringStore::coordinated(
        FailedRead {
            memory: memory.clone(),
            fail: fail.clone(),
        },
        domain(dir.path(), &identity),
    )
    .unwrap();
    store.put(name("test/retained"), value(), None).unwrap();
    fail.store(true, Ordering::SeqCst);
    assert_eq!(store.list(0), Err(Error::Backend));
    assert!(matches!(
        store.get(&name("test/retained"), 0),
        Err(Error::Backend)
    ));
    assert_eq!(store.delete(&name("test/retained")), Err(Error::Backend));
    assert_eq!(
        store.put(name("test/no"), value(), None),
        Err(Error::Backend)
    );
    fail.store(false, Ordering::SeqCst);
    assert!(store.get(&name("test/retained"), 0).unwrap().is_some());
    assert_eq!(store.list(0).unwrap().len(), 1);
    // Invalid envelope is not an empty backend and cannot be overwritten.
    *memory.bytes.lock().unwrap() = Some(SecretValue::new(b"synthetic-invalid-envelope".to_vec()));
    assert_eq!(
        store.delete(&name("test/retained")),
        Err(Error::InvalidData)
    );
    assert_eq!(
        store.put(name("test/no"), value(), None),
        Err(Error::InvalidData)
    );
    assert!(memory.bytes.lock().unwrap().is_some());
}

// Real subprocess fixture driver: file-backed synthetic envelope item, never Keychain.
struct SyntheticDisk {
    identity: KeyringIdentity,
    backend: Mutex<FileStore>,
    block: Option<PathBuf>,
    ambiguous: bool,
}
impl KeyringDriver for SyntheticDisk {
    fn identity(&self) -> Option<KeyringIdentity> {
        Some(self.identity.clone())
    }
    fn load(&self) -> Result<Option<SecretValue>, Error> {
        if let Some(marker) = &self.block {
            fs::write(marker, b"synthetic-lock-held").map_err(|_| Error::Io)?;
            loop {
                thread::park_timeout(Duration::from_secs(1));
            }
        }
        let bytes = self
            .backend
            .lock()
            .unwrap()
            .get(&name("synthetic/envelope"), 0)?;
        thread::sleep(Duration::from_millis(3));
        Ok(bytes)
    }
    fn save(&self, value: &SecretValue) -> Result<(), Error> {
        self.backend.lock().unwrap().put(
            name("synthetic/envelope"),
            SecretValue::new(value.expose().to_vec()),
            None,
        )?;
        if self.ambiguous {
            Err(Error::Backend)
        } else {
            Ok(())
        }
    }
}
fn disk_store(
    dir: &Path,
    identity: &KeyringIdentity,
    block: Option<PathBuf>,
    ambiguous: bool,
) -> KeyringStore<SyntheticDisk> {
    let driver = SyntheticDisk {
        identity: identity.clone(),
        backend: Mutex::new(FileStore::open(&dir.join("synthetic-backend.toml")).unwrap()),
        block,
        ambiguous,
    };
    KeyringStore::coordinated(driver, domain(dir, identity)).unwrap()
}
const WORKER: &str = "keyring_lock::tests::synthetic_subprocess_worker";
struct SyntheticChild(Child);
impl std::ops::Deref for SyntheticChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}
impl std::ops::DerefMut for SyntheticChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}
impl Drop for SyntheticChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn spawn(dir: &Path, identity: &KeyringIdentity, mode: &str, worker: usize) -> SyntheticChild {
    SyntheticChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", WORKER])
            // Environment is cleared; read only these explicitly injected NON-SECRET fixture parameters.
            .env_clear()
            .env("M8S_SYNTHETIC_WORKER_DIR", dir)
            .env("M8S_SYNTHETIC_WORKER_ACCOUNT", identity.account().as_str())
            .env("M8S_SYNTHETIC_WORKER_MODE", mode)
            .env("M8S_SYNTHETIC_WORKER_ID", worker.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}
fn wait(child: &mut SyntheticChild) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("synthetic worker timeout");
        }
        thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn synthetic_subprocess_worker() {
    let Some(dir) = std::env::var_os("M8S_SYNTHETIC_WORKER_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let account = std::env::var("M8S_SYNTHETIC_WORKER_ACCOUNT").unwrap();
    let mode = std::env::var("M8S_SYNTHETIC_WORKER_MODE").unwrap();
    let worker: usize = std::env::var("M8S_SYNTHETIC_WORKER_ID")
        .unwrap()
        .parse()
        .unwrap();
    let identity = KeyringIdentity::new("synthetic-only", name(&account)).unwrap();
    let marker = (mode == "block").then(|| dir.join("synthetic-entered"));
    let mut store = disk_store(&dir, &identity, marker, mode == "ambiguous");
    if mode == "block" {
        let _ = store.list(0);
        panic!("blocking fixture unexpectedly returned");
    }
    if mode == "ambiguous" {
        assert_eq!(
            store.put(name("test/ambiguous"), value(), None),
            Err(Error::Backend)
        );
        return;
    }
    for record in 0..12 {
        store
            .put(name(&format!("test/{worker}-{record}")), value(), None)
            .unwrap();
    }
}
#[test]
fn real_process_writers_crash_restart_and_ambiguous_save_preserve_evidence() {
    let (dir, identity) = fixture();
    let mut children: Vec<_> = (0..6)
        .map(|worker| spawn(dir.path(), &identity, "write", worker))
        .collect();
    for child in &mut children {
        wait(child);
    }
    let mut store = disk_store(dir.path(), &identity, None, false);
    assert_eq!(store.list(0).unwrap().len(), 72);
    let mut ambiguous = spawn(dir.path(), &identity, "ambiguous", 0);
    wait(&mut ambiguous);
    assert!(store.get(&name("test/ambiguous"), 0).unwrap().is_some());
    // Crash while backend.load is running under the whole-operation flock.
    let mut blocker = spawn(dir.path(), &identity, "block", 0);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !dir.path().join("synthetic-entered").exists() {
        if Instant::now() > deadline {
            let _ = blocker.kill();
            let _ = blocker.wait();
            panic!("synthetic blocker did not acquire lock");
        }
        thread::sleep(Duration::from_millis(5));
    }
    let mut blocked = spawn(dir.path(), &identity, "write", 99);
    thread::sleep(Duration::from_millis(50));
    assert!(blocked.try_wait().unwrap().is_none());
    blocker.kill().unwrap();
    assert!(!blocker.wait().unwrap().success());
    wait(&mut blocked);
    assert_eq!(store.list(0).unwrap().len(), 85);
    store.put(name("test/expired"), value(), Some(10)).unwrap();
    assert!(store.get(&name("test/expired"), 10).unwrap().is_none());
    assert!(store.delete(&name("test/expired")).unwrap());
    drop(store);
    assert_eq!(
        disk_store(dir.path(), &identity, None, false)
            .list(0)
            .unwrap()
            .len(),
        85
    );
}
