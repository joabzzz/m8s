use super::*;
use std::sync::{Arc, Mutex};
fn name(s: &str) -> SecretName {
    SecretName::new(s).unwrap()
}
fn value() -> SecretValue {
    SecretValue::new(b"synthetic-session-only".to_vec())
}
fn exercise(s: &mut dyn SecretStore) {
    let n = name("test/session");
    assert!(s.get(&n, 0).unwrap().is_none());
    s.put(n.clone(), value(), Some(10)).unwrap();
    assert_eq!(
        s.get(&n, 9).unwrap().unwrap().expose(),
        b"synthetic-session-only"
    );
    assert!(s.get(&n, 10).unwrap().is_none());
    assert_eq!(
        s.list(9).unwrap(),
        vec![SecretInfo {
            name: n.clone(),
            expires_at: Some(10)
        }]
    );
    assert!(s.list(10).unwrap().is_empty());
    assert!(s.delete(&n).unwrap());
    assert!(!s.delete(&n).unwrap());
    s.put(n, value(), None).unwrap();
    assert_eq!(s.list(i64::MAX).unwrap().len(), 1);
}
#[derive(Clone, Default)]
struct MockDriver {
    bytes: Arc<Mutex<Option<SecretValue>>>,
}
impl KeyringDriver for MockDriver {
    fn load(&self) -> Result<Option<SecretValue>, Error> {
        Ok(self
            .bytes
            .lock()
            .unwrap()
            .as_ref()
            .map(|v| SecretValue::new(v.expose().to_vec())))
    }
    fn save(&self, v: &SecretValue) -> Result<(), Error> {
        *self.bytes.lock().unwrap() = Some(SecretValue::new(v.expose().to_vec()));
        Ok(())
    }
}
#[test]
fn keyring_roundtrip_restart_expiry_and_metadata() {
    let driver = MockDriver::default();
    let mut store = KeyringStore::injected(driver.clone());
    exercise(&mut store);
    let store = KeyringStore::injected(driver);
    assert!(store.get(&name("test/session"), 100).unwrap().is_some());
}
struct FailedDriver;
impl KeyringDriver for FailedDriver {
    fn load(&self) -> Result<Option<SecretValue>, Error> {
        Err(Error::Unavailable)
    }
    fn save(&self, _: &SecretValue) -> Result<(), Error> {
        panic!("must not write after failed read")
    }
}
#[test]
fn keyring_errors_are_not_absence_and_never_fall_back() {
    let mut store = KeyringStore::injected(FailedDriver);
    assert!(matches!(
        store.get(&name("test/session"), 0),
        Err(Error::Backend)
    ));
    assert_eq!(store.list(0), Err(Error::Backend));
    assert_eq!(
        store.put(name("test/session"), value(), None),
        Err(Error::Backend)
    );
    assert_eq!(store.delete(&name("test/session")), Err(Error::Backend));
}
struct FailedSaveDriver;
impl KeyringDriver for FailedSaveDriver {
    fn load(&self) -> Result<Option<SecretValue>, Error> {
        Ok(None)
    }
    fn save(&self, _: &SecretValue) -> Result<(), Error> {
        Err(Error::Unavailable)
    }
}
#[test]
fn keyring_write_failure_and_corruption_are_opaque_and_fail_closed() {
    let mut store = KeyringStore::injected(FailedSaveDriver);
    assert_eq!(
        store.put(name("test/session"), value(), None),
        Err(Error::Backend)
    );
    assert!(store.list(0).unwrap().is_empty());
    let driver = MockDriver::default();
    *driver.bytes.lock().unwrap() = Some(SecretValue::new(b"synthetic-invalid-envelope".to_vec()));
    let mut store = KeyringStore::injected(driver);
    assert_eq!(store.list(0), Err(Error::InvalidData));
    assert_eq!(store.delete(&name("test/session")), Err(Error::InvalidData));
    assert_eq!(
        store.put(name("test/session"), value(), None),
        Err(Error::InvalidData)
    );
}
#[test]
fn codec_rejects_corruption_limits_duplicates_and_trailing_bytes() {
    use crate::codec::*;
    let mut entries = Entries::new();
    entries.insert(name("test/session"), (value(), Some(10)));
    let bytes = encode(&entries).unwrap();
    for end in 0..bytes.len() {
        assert!(decode(&bytes[..end]).is_err());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(decode(&trailing).is_err());
    let mut duplicate = bytes.to_vec();
    duplicate[8..12].copy_from_slice(&2u32.to_le_bytes());
    duplicate.extend_from_slice(&bytes[12..]);
    assert!(decode(&duplicate).is_err());
    assert!(decode(&vec![0; MAX_BYTES + 1]).is_err());
    let mut bad_version = bytes.to_vec();
    bad_version[7] = b'2';
    assert!(decode(&bad_version).is_err());
    entries.insert(
        name("test/large"),
        (SecretValue::new(vec![0; MAX_VALUE + 1]), None),
    );
    assert!(encode(&entries).is_err());
    // Deterministic malformed inputs exercise parser bounds without a fuzzing service.
    for size in 0..2048 {
        let bytes: Vec<u8> = (0..size).map(|i| ((i * 31 + size) % 256) as u8).collect();
        assert!(decode(&bytes).is_err());
    }
}
#[test]
fn trait_objects_are_send_sync() {
    fn assert_bounds<T: Send + Sync>() {}
    assert_bounds::<SecretValue>();
    assert_bounds::<EnvStore>();
    assert_bounds::<KeyringStore<MockDriver>>();
    #[cfg(unix)]
    assert_bounds::<FileStore>();
    let _: Box<dyn SecretStore> = Box::new(EnvStore::injected([]));
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
        path::PathBuf,
    };
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        // Explicit /private/tmp is canonical on macOS; never use ambient TMPDIR.
        #[cfg(target_os = "macos")]
        let base = "/private/tmp";
        #[cfg(not(target_os = "macos"))]
        let base = "/tmp";
        let dir = tempfile::Builder::new()
            .prefix("m8s-synthetic-session-")
            .tempdir_in(base)
            .unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("secrets.toml");
        (dir, path)
    }
    #[test]
    fn file_roundtrip_persists_exact_mode() {
        let (_dir, path) = fixture();
        let mut store = FileStore::open(&path).unwrap();
        exercise(&mut store);
        drop(store);
        let store = FileStore::open(&path).unwrap();
        assert!(store.get(&name("test/session"), 0).unwrap().is_some());
        let m = fs::metadata(&path).unwrap();
        assert_eq!(m.mode() & 0o7777, 0o600);
        assert_eq!(m.nlink(), 1);
    }
    #[test]
    fn file_is_toml_and_never_migrates_binary_or_invalid_toml() {
        let (_dir, path) = fixture();
        let mut store = FileStore::open(&path).unwrap();
        store.put(name("test/session"), value(), None).unwrap();
        assert!(fs::read(&path).unwrap().starts_with(b"version = 1\n"));
        let mut entries = codec::Entries::new();
        entries.insert(name("test/session"), (value(), None));
        let binary = codec::encode(&entries).unwrap();
        fs::write(&path, &*binary).unwrap();
        assert!(FileStore::open(&path).is_err());
        assert_eq!(store.delete(&name("test/session")), Err(Error::InvalidData));
        assert!(fs::read(&path).unwrap() == *binary);
        fs::write(&path, b"version = 2\n").unwrap();
        assert_eq!(
            store.put(name("test/other"), value(), None),
            Err(Error::InvalidData)
        );
        assert_eq!(fs::read(&path).unwrap(), b"version = 2\n");
    }
    #[test]
    fn file_rejects_permissions_links_nonregular_and_corruption() {
        let (dir, path) = fixture();
        let mut store = FileStore::open(&path).unwrap();
        store.put(name("test/session"), value(), None).unwrap();
        for mode in [0o644, 0o666, 0o400, 0o700] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert!(store.list(0).is_err());
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&path, dir.path().join("linked")).unwrap();
        assert!(store.list(0).is_err());
        fs::remove_file(dir.path().join("linked")).unwrap();
        fs::rename(&path, dir.path().join("original")).unwrap();
        symlink(dir.path().join("original"), &path).unwrap();
        assert!(store.list(0).is_err());
        assert!(store.put(name("test/other"), value(), None).is_err());
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(store.list(0).is_err());
        fs::remove_dir(&path).unwrap();
        fs::write(&path, b"not-a-secret-envelope").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(store.list(0), Err(Error::InvalidData));
        fs::write(&path, vec![0; codec::MAX_BYTES + 1]).unwrap();
        assert_eq!(store.list(0), Err(Error::InvalidData));
    }
    #[test]
    fn file_rejects_unsafe_parents_git_markers_and_path_swaps() {
        let (dir, path) = fixture();
        assert!(FileStore::open(std::path::Path::new("relative")).is_err());
        for mode in [0o755, 0o770, 0o1777] {
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(mode)).unwrap();
            assert!(FileStore::open(&path).is_err());
        }
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let store = FileStore::open(&path).unwrap();
        // Synthetic git marker: never initialize or write secret data in a git repository.
        fs::write(dir.path().join(".git"), b"synthetic-marker").unwrap();
        assert!(store.list(0).is_err());
        fs::remove_file(dir.path().join(".git")).unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        assert!(FileStore::open(&path).is_err());
        fs::remove_dir(dir.path().join(".git")).unwrap();
        fs::create_dir(dir.path().join("child")).unwrap();
        fs::set_permissions(dir.path().join("child"), fs::Permissions::from_mode(0o700)).unwrap();
        let nested = FileStore::open(&dir.path().join("child/store")).unwrap();
        fs::write(dir.path().join(".git"), b"synthetic-marker").unwrap();
        assert!(nested.list(0).is_err());
        fs::remove_file(dir.path().join(".git")).unwrap();
        symlink(dir.path().join("child"), dir.path().join("alias")).unwrap();
        assert!(FileStore::open(&dir.path().join("alias/store")).is_err());
        let moved = dir.path().with_extension("moved");
        fs::rename(dir.path(), &moved).unwrap();
        fs::create_dir(dir.path()).unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(store.list(0).is_err());
        fs::remove_dir(dir.path()).unwrap();
        fs::rename(moved, dir.path()).unwrap();
    }
    #[test]
    fn lock_file_is_checked_and_updates_are_serialized() {
        let (dir, path) = fixture();
        let lock = dir.path().join("secrets.toml.lock");
        fs::write(&lock, b"").unwrap();
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(FileStore::open(&path).is_err());
        fs::remove_file(&lock).unwrap();
        symlink(dir.path().join("absent"), &lock).unwrap();
        assert!(FileStore::open(&path).is_err());
        fs::remove_file(&lock).unwrap();
        let handles: Vec<_> = (0..12)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut store = FileStore::open(&path).unwrap();
                    store
                        .put(name(&format!("test/session-{i}")), value(), None)
                        .unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(FileStore::open(&path).unwrap().list(0).unwrap().len(), 12);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn extended_acl_cannot_bypass_private_mode_bits() {
        let (dir, path) = fixture();
        let mut store = FileStore::open(&path).unwrap();
        store.put(name("test/session"), value(), None).unwrap();
        // Only synthetic paths, never a keychain or ambient/user keystore.
        let status = std::process::Command::new("/bin/chmod")
            .args(["+a", "everyone allow read"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(store.list(0).is_err());
        let status = std::process::Command::new("/bin/chmod")
            .arg("-N")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(store.list(0).is_ok());
        let status = std::process::Command::new("/bin/chmod")
            .args(["+a", "everyone allow read"])
            .arg(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(store.list(0).is_err());
    }
    #[test]
    fn failed_update_preserves_prior_store() {
        let (_dir, path) = fixture();
        let mut store = FileStore::open(&path).unwrap();
        store.put(name("test/session"), value(), None).unwrap();
        let old = fs::read(&path).unwrap();
        assert_eq!(
            store.put(
                name("test/large"),
                SecretValue::new(vec![0; codec::MAX_VALUE + 1]),
                None
            ),
            Err(Error::InvalidData)
        );
        assert!(fs::read(&path).unwrap() == old);
    }
}
