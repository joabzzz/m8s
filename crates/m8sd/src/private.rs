//! Private state boundary. Ancestors above the canonical private directory must
//! be trusted not to rename/replace it; hostile same-eUID processes are outside
//! this discretionary filesystem boundary. No extended ACLs are accepted.
use crate::{Error, Result};
#[cfg(unix)]
use std::os::{
    fd::AsRawFd,
    unix::fs::{MetadataExt, OpenOptionsExt},
};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

#[cfg(unix)]
fn attributes(uid: u32, mode: u32, links: u64, directory: bool, euid: u32) -> Result<()> {
    if uid != euid
        || mode & 0o7777 != if directory { 0o700 } else { 0o600 }
        || (!directory && links != 1)
    {
        return Err(Error::PrivateState);
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn no_acl(file: &File) -> Result<()> {
    use std::ffi::c_void;
    unsafe extern "C" {
        fn acl_get_fd_np(fd: i32, kind: i32) -> *mut c_void;
        fn acl_free(acl: *mut c_void) -> i32;
    }
    // Darwin ACL_TYPE_EXTENDED: a live fd with no extended ACL reports
    // ENOENT. Reject every returned ACL (including empty allocated ACLs), and
    // every other query error. The fd is validated before and after this query.
    unsafe {
        let acl = acl_get_fd_np(file.as_raw_fd(), 0x100);
        if acl.is_null() {
            if std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT) {
                return Err(Error::PrivateState);
            }
        } else {
            acl_free(acl);
            return Err(Error::PrivateState);
        }
    }
    Ok(())
}
#[cfg(target_os = "linux")]
fn no_acl(file: &File) -> Result<()> {
    for name in [c"system.posix_acl_access", c"system.posix_acl_default"] {
        let n =
            unsafe { libc::fgetxattr(file.as_raw_fd(), name.as_ptr(), std::ptr::null_mut(), 0) };
        if n >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ENODATA) {
            return Err(Error::PrivateState);
        }
    }
    Ok(())
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn no_acl(_file: &File) -> Result<()> {
    Err(Error::PrivateState)
}
#[cfg(unix)]
fn validate(file: &File, directory: bool) -> Result<()> {
    let m = file.metadata().map_err(|_| Error::PrivateState)?;
    if (directory && !m.is_dir()) || (!directory && !m.is_file()) {
        return Err(Error::PrivateState);
    }
    // geteuid has no memory safety preconditions; do not infer ownership from
    // HOME, environment variables, or the UID of a caller-supplied path.
    attributes(m.uid(), m.mode(), m.nlink(), directory, unsafe {
        libc::geteuid()
    })?;
    no_acl(file)?;
    let after = file.metadata().map_err(|_| Error::PrivateState)?;
    attributes(
        after.uid(),
        after.mode(),
        after.nlink(),
        directory,
        unsafe { libc::geteuid() },
    )
}
#[cfg(unix)]
fn open_checked(path: &Path, directory: bool) -> Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(
            libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | libc::O_NONBLOCK
                | if directory { libc::O_DIRECTORY } else { 0 },
        )
        .open(path)
        .map_err(|_| Error::PrivateState)?;
    validate(&f, directory)?;
    same_path(&f, path)?;
    Ok(f)
}
#[cfg(unix)]
fn same_path(file: &File, path: &Path) -> Result<()> {
    let fd = file.metadata().map_err(|_| Error::PrivateState)?;
    let path = fs::symlink_metadata(path).map_err(|_| Error::PrivateState)?;
    if path.file_type().is_symlink() || fd.dev() != path.dev() || fd.ino() != path.ino() {
        return Err(Error::PrivateState);
    }
    Ok(())
}
#[cfg(not(unix))]
fn open_checked(_path: &Path, _directory: bool) -> Result<File> {
    Err(Error::PrivateState)
}
#[cfg(not(unix))]
fn validate(_file: &File, _directory: bool) -> Result<()> {
    Err(Error::PrivateState)
}
#[cfg(not(unix))]
fn same_path(_file: &File, _path: &Path) -> Result<()> {
    Err(Error::PrivateState)
}

fn sidecars(path: &Path) -> [PathBuf; 2] {
    ["-wal", "-shm"].map(|suffix| {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        PathBuf::from(p)
    })
}
#[cfg(test)]
pub(crate) fn verify_sidecars(path: &Path) -> Result<()> {
    for path in sidecars(path) {
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                open_checked(&path, false)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(Error::PrivateState),
        }
    }
    Ok(())
}
#[cfg(test)]
pub(crate) struct PrivateState {
    pub path: PathBuf,
    directory: File,
    database: File,
}
#[cfg(test)]
impl PrivateState {
    pub fn recheck(&self) -> Result<()> {
        validate(&self.directory, true)?;
        same_path(
            &self.directory,
            self.path.parent().ok_or(Error::PrivateState)?,
        )?;
        validate(&self.database, false)?;
        same_path(&self.database, &self.path)?;
        verify_sidecars(&self.path)
    }
}
#[cfg(all(test, unix))]
pub(crate) fn prepare(path: &Path) -> Result<PrivateState> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(Error::PrivateState)?;
    let directory = open_checked(parent, true)?;
    // Permit trusted system ancestor aliases (/var on macOS), not a symlink at
    // the actual private directory. Retain descriptors across SQLite's open.
    let parent = fs::canonicalize(parent).map_err(|_| Error::PrivateState)?;
    same_path(&directory, &parent)?;
    let path = parent.join(path.file_name().ok_or(Error::PrivateState)?);
    let mut create = OpenOptions::new();
    create
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    match create.open(&path) {
        Ok(f) => validate(&f, false)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(Error::PrivateState),
    }
    let state = PrivateState {
        database: open_checked(&path, false)?,
        path,
        directory,
    };
    state.recheck()?;
    Ok(state)
}
#[cfg(all(test, not(unix)))]
pub(crate) fn prepare(_path: &Path) -> Result<PrivateState> {
    Err(Error::PrivateState)
}

// These guards coordinate managed Store connections, not arbitrary raw SQLite
// connections or uncooperative same-UID path replacement. Never close an extra
// SQLite-inode FD while any associated managed connection is alive.
pub(crate) use lifetime::{Lease, acquire};
#[cfg(unix)]
mod lifetime {
    use super::*;
    use std::{
        collections::BTreeMap,
        sync::{Mutex, OnceLock},
    };
    type Identity = (u64, u64);
    fn identity(m: &fs::Metadata) -> Identity {
        (m.dev(), m.ino())
    }
    struct Anchor {
        file: File,
        id: Identity,
        #[cfg(test)]
        closed: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
    }
    impl Anchor {
        fn new(file: File, id: Identity) -> Self {
            Self {
                file,
                id,
                #[cfg(test)]
                closed: None,
            }
        }
    }
    #[cfg(test)]
    impl Drop for Anchor {
        fn drop(&mut self) {
            if let Some(closed) = &self.closed {
                closed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }
    struct Entry {
        path: PathBuf,
        directory: File,
        database: Anchor,
        sidecars: [Option<Anchor>; 2],
        leases: usize,
        refused: bool,
    }
    #[derive(Default)]
    struct Registry {
        entries: BTreeMap<Identity, Entry>,
        // A successful open is registered here BEFORE metadata/validation.
        // Unknown identity/race/poison refuses further opens; keep these until
        // every managed connection closes rather than guessing which owns it.
        pending: Vec<File>,
        refused: bool,
    }
    fn registry() -> &'static Mutex<Registry> {
        static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
        REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
    }
    impl Registry {
        fn pending_identity(&mut self, expected: Option<Identity>) -> Result<Identity> {
            let metadata = self.pending.last().ok_or(Error::PrivateState)?.metadata();
            match metadata {
                Ok(m) if expected.is_none_or(|id| id == identity(&m)) => Ok(identity(&m)),
                _ => {
                    self.refused = true;
                    Err(Error::PrivateState)
                }
            }
        }
        fn known_inode(&self, id: Identity) -> bool {
            self.entries
                .values()
                .any(|e| e.database.id == id || e.sidecars.iter().flatten().any(|a| a.id == id))
        }
        fn open_pending(&mut self, path: &Path, expected: Identity) -> Result<Identity> {
            // No validation may drop this FD, even on an ACL/query/path failure.
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(path)
                .map_err(|_| Error::PrivateState)?;
            self.pending.push(file);
            self.pending_identity(Some(expected))
        }
        fn check(&mut self, id: Identity) -> Result<()> {
            if self.refused || self.entries.get(&id).is_none_or(|e| e.refused) {
                return Err(Error::PrivateState);
            }
            let result = self.check_inner(id);
            if result.is_err() {
                self.entries.get_mut(&id).unwrap().refused = true;
            }
            result
        }
        fn check_inner(&mut self, id: Identity) -> Result<()> {
            let entry = self.entries.get(&id).ok_or(Error::PrivateState)?;
            validate(&entry.directory, true)?;
            same_path(
                &entry.directory,
                entry.path.parent().ok_or(Error::PrivateState)?,
            )?;
            validate(&entry.database.file, false)?;
            same_path(&entry.database.file, &entry.path)?;
            let paths = sidecars(&entry.path);
            for (index, path) in paths.iter().enumerate() {
                if let Some(anchor) = &self.entries[&id].sidecars[index] {
                    validate(&anchor.file, false)?;
                    same_path(&anchor.file, path)?;
                    continue;
                }
                let meta = match fs::symlink_metadata(path) {
                    Ok(m) => m,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => return Err(Error::PrivateState),
                };
                // Reject aliases BEFORE any extra open; canonical parent aliases
                // already share the database entry. Cached inode checks also
                // catch a renamed managed inode without creating a close hazard.
                let expected = identity(&meta);
                if !meta.is_file() || meta.nlink() != 1 || self.known_inode(expected) {
                    return Err(Error::PrivateState);
                }
                let actual = self.open_pending(path, expected)?;
                // A race that reaches another registered inode must not close it.
                if self.known_inode(actual) {
                    self.refused = true;
                    return Err(Error::PrivateState);
                }
                let file = self.pending.pop().ok_or(Error::PrivateState)?;
                self.entries.get_mut(&id).unwrap().sidecars[index] =
                    Some(Anchor::new(file, actual));
                let anchor = self.entries[&id].sidecars[index].as_ref().unwrap();
                validate(&anchor.file, false)?;
                same_path(&anchor.file, path)?;
            }
            Ok(())
        }
        fn acquire(&mut self, path: PathBuf, directory: File) -> Result<Identity> {
            if self.refused {
                return Err(Error::PrivateState);
            }
            if let Some(id) = self
                .entries
                .iter()
                .find_map(|(id, e)| (e.path == path).then_some(*id))
            {
                self.check(id)?;
                let entry = self.entries.get_mut(&id).unwrap();
                entry.leases = entry.leases.checked_add(1).ok_or(Error::PrivateState)?;
                return Ok(id);
            }
            let meta = match fs::symlink_metadata(&path) {
                Ok(m) => Some(m),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => return Err(Error::PrivateState),
            };
            let id = if let Some(meta) = meta {
                let expected = identity(&meta);
                if !meta.is_file() || meta.nlink() != 1 || self.known_inode(expected) {
                    return Err(Error::PrivateState);
                }
                self.open_pending(&path, expected)?
            } else {
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                    .open(&path)
                    .map_err(|_| Error::PrivateState)?;
                self.pending.push(file);
                self.pending_identity(None)?
            };
            if self.known_inode(id) {
                self.refused = true;
                return Err(Error::PrivateState);
            }
            let file = self.pending.pop().ok_or(Error::PrivateState)?;
            self.entries.insert(
                id,
                Entry {
                    path,
                    directory,
                    database: Anchor::new(file, id),
                    sidecars: [None, None],
                    leases: 0,
                    refused: false,
                },
            );
            self.check(id)?;
            self.entries.get_mut(&id).unwrap().leases = 1;
            Ok(id)
        }
        fn close_unused(&mut self) {
            // Called under the SAME mutex as acquire: last close cannot race a
            // new connection open. An unknown pending FD pins every entry until
            // the last connection, because its identity cannot safely be guessed.
            if !self.pending.is_empty() && self.entries.values().any(|e| e.leases > 0) {
                return;
            }
            self.entries.retain(|_, e| e.leases > 0);
            if self.entries.is_empty() {
                self.pending.clear();
                self.refused = false;
            }
        }
    }
    pub(crate) struct Lease {
        pub path: PathBuf,
        id: Identity,
    }
    impl Lease {
        pub fn recheck(&self) -> Result<()> {
            let mut guards = registry().lock().map_err(|_| Error::PrivateState)?;
            guards.check(self.id)
        }
    }
    fn release(registry: &Mutex<Registry>, id: Identity) {
        // Recover ownership only to drain already-closed connections. Poisoned
        // mutexes still refuse every acquire/recheck; never panic in Drop.
        let mut guards = match registry.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        if let Some(entry) = guards.entries.get_mut(&id) {
            entry.leases = entry.leases.saturating_sub(1);
        }
        guards.close_unused();
    }
    impl Drop for Lease {
        fn drop(&mut self) {
            release(registry(), self.id);
        }
    }
    pub(crate) fn acquire(path: &Path) -> Result<Lease> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or(Error::PrivateState)?;
        let directory = open_checked(parent, true)?;
        let parent = fs::canonicalize(parent).map_err(|_| Error::PrivateState)?;
        same_path(&directory, &parent)?;
        let path = parent.join(path.file_name().ok_or(Error::PrivateState)?);
        let mut guards = registry().lock().map_err(|_| Error::PrivateState)?;
        let result = guards.acquire(path.clone(), directory);
        guards.close_unused();
        result.map(|id| Lease { path, id })
    }
    #[cfg(test)]
    mod lifecycle_tests {
        use super::*;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        struct Fixture(PathBuf);
        impl Fixture {
            fn new() -> Self {
                use std::os::unix::fs::DirBuilderExt;
                let path = std::env::temp_dir().join(format!(
                    "m8lease-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
                Self(path)
            }
            fn acquire(&self, g: &mut Registry) -> Identity {
                g.acquire(
                    self.0.join("state.db"),
                    open_checked(&self.0, true).unwrap(),
                )
                .unwrap()
            }
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        #[test]
        fn failed_discovered_anchor_is_retained_before_validation_and_poisoned() {
            let f = Fixture::new();
            let mut g = Registry::default();
            let id = f.acquire(&mut g);
            let wal = sidecars(&f.0.join("state.db"))[0].clone();
            drop(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o640)
                    .open(&wal)
                    .unwrap(),
            );
            assert!(g.check(id).is_err());
            let closed = Arc::new(AtomicUsize::new(0));
            g.entries.get_mut(&id).unwrap().sidecars[0]
                .as_mut()
                .unwrap()
                .closed = Some(closed.clone());
            let anchor = g.entries[&id].sidecars[0].as_ref().unwrap().id;
            assert!(g.entries[&id].refused);
            assert!(g.check(id).is_err());
            assert_eq!(g.entries[&id].sidecars[0].as_ref().unwrap().id, anchor);
            assert!(g.pending.is_empty());
            g.close_unused();
            assert_eq!(closed.load(Ordering::SeqCst), 0);
            g.entries.get_mut(&id).unwrap().leases = 0;
            g.close_unused();
            assert_eq!(closed.load(Ordering::SeqCst), 1);
            assert!(g.entries.is_empty());
        }
        #[test]
        fn last_anchor_close_is_serialized_against_next_acquire() {
            let f = Fixture::new();
            let g = Arc::new(Mutex::new(Registry::default()));
            let mut guard = g.lock().unwrap();
            let id = f.acquire(&mut guard);
            let closed = Arc::new(AtomicUsize::new(0));
            guard.entries.get_mut(&id).unwrap().database.closed = Some(closed.clone());
            guard.entries.get_mut(&id).unwrap().leases = 0;
            guard.close_unused();
            assert_eq!(closed.load(Ordering::SeqCst), 1);
            let contender = g.clone();
            // try_lock is bounded: even AFTER actual anchor Drop, acquire is
            // excluded until this same last-close guard is released.
            assert!(
                std::thread::spawn(move || matches!(
                    contender.try_lock(),
                    Err(std::sync::TryLockError::WouldBlock)
                ))
                .join()
                .unwrap()
            );
            drop(guard);
            let mut guard = g.lock().unwrap();
            let new = f.acquire(&mut guard);
            assert_eq!(guard.entries[&new].leases, 1);
        }
        #[test]
        fn poison_and_unknown_pending_fd_do_not_destroy_live_anchors() {
            let f = Fixture::new();
            let g = Arc::new(Mutex::new(Registry::default()));
            let closed = Arc::new(AtomicUsize::new(0));
            let id = {
                let mut guard = g.lock().unwrap();
                let id = f.acquire(&mut guard);
                let entry = guard.entries.get_mut(&id).unwrap();
                entry.leases = 2;
                entry.database.closed = Some(closed.clone());
                guard
                    .pending
                    .push(File::open(f.0.join("state.db")).unwrap());
                assert!(guard.pending_identity(Some((0, 0))).is_err());
                assert!(guard.refused);
                id
            };
            let poisoned = g.clone();
            assert!(
                std::thread::spawn(move || {
                    let _guard = poisoned.lock().unwrap();
                    panic!("synthetic registry unwind");
                })
                .join()
                .is_err()
            );
            assert!(g.lock().is_err());
            release(&g, id);
            assert_eq!(closed.load(Ordering::SeqCst), 0);
            assert_eq!(g.lock().unwrap_or_else(|e| e.into_inner()).pending.len(), 1);
            release(&g, id);
            assert_eq!(closed.load(Ordering::SeqCst), 1);
            let guard = g.lock().unwrap_or_else(|e| e.into_inner());
            assert!(guard.entries.is_empty() && guard.pending.is_empty());
            drop(guard);
            assert!(g.lock().is_err()); // No silent poison recovery/public reopen.
        }
        #[test]
        fn hardlinks_and_live_sidecar_replacement_refuse_until_last_close() {
            let f = Fixture::new();
            let mut g = Registry::default();
            let id = f.acquire(&mut g);
            fs::hard_link(f.0.join("state.db"), f.0.join("alias.db")).unwrap();
            assert!(
                g.acquire(f.0.join("alias.db"), open_checked(&f.0, true).unwrap())
                    .is_err()
            );
            assert_eq!(g.entries.len(), 1);
            assert!(g.pending.is_empty());
            // New independent fixture for the live sidecar generation check.
            let other = Fixture::new();
            let id2 = other.acquire(&mut g);
            let wal = sidecars(&other.0.join("state.db"))[0].clone();
            drop(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&wal)
                    .unwrap(),
            );
            g.check(id2).unwrap();
            let old = g.entries[&id2].sidecars[0].as_ref().unwrap().id;
            fs::rename(&wal, wal.with_extension("old")).unwrap();
            drop(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&wal)
                    .unwrap(),
            );
            assert!(g.check(id2).is_err());
            assert_eq!(g.entries[&id2].sidecars[0].as_ref().unwrap().id, old);
            assert!(
                g.acquire(
                    other.0.join("state.db"),
                    open_checked(&other.0, true).unwrap()
                )
                .is_err()
            );
            g.entries.get_mut(&id2).unwrap().leases = 0;
            g.close_unused();
            let next = other.acquire(&mut g);
            assert_ne!(g.entries[&next].sidecars[0].as_ref().unwrap().id, old);
            assert_eq!(g.entries[&id].leases, 1);
        }
    }
}
#[cfg(not(unix))]
mod lifetime {
    use super::*;
    pub(crate) struct Lease {
        pub path: PathBuf,
    }
    impl Lease {
        pub fn recheck(&self) -> Result<()> {
            Err(Error::PrivateState)
        }
    }
    pub(crate) fn acquire(_: &Path) -> Result<Lease> {
        Err(Error::PrivateState)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthetic_uid_and_special_mode_policy() {
        assert!(attributes(42, 0o700, 1, true, 42).is_ok());
        assert!(attributes(43, 0o700, 1, true, 42).is_err());
        assert!(attributes(43, 0o600, 1, false, 42).is_err());
        for mode in [0o4600, 0o2600, 0o1600, 0o640, 0o400, 0o777] {
            assert!(attributes(42, mode, 1, false, 42).is_err());
        }
        assert!(attributes(42, 0o600, 2, false, 42).is_err());
    }
}

#[cfg(all(test, unix))]
mod filesystem_tests {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn fixture() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "m8sd-private-review-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        p.join("state.db")
    }
    #[test]
    fn descriptor_recheck_detects_replacement_and_mode_change() {
        let p = fixture();
        let state = prepare(&p).unwrap();
        fs::rename(&p, p.with_extension("old")).unwrap();
        let replacement = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&p)
            .unwrap();
        assert!(state.recheck().is_err());
        drop(replacement);
        let state = prepare(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(state.recheck().is_err());
        assert!(prepare(&p).is_err());
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        let state = prepare(&p).unwrap();
        let parent = p.parent().unwrap();
        let old = parent.with_extension("moved");
        fs::rename(parent, &old).unwrap();
        fs::DirBuilder::new().mode(0o700).create(parent).unwrap();
        assert!(state.recheck().is_err());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn extended_acl_on_private_directory_database_or_sidecar_is_refused() {
        for target in [0, 1, 2] {
            let p = fixture();
            let state = prepare(&p).unwrap();
            let acl_path = match target {
                0 => p.parent().unwrap().to_path_buf(),
                1 => p.clone(),
                _ => {
                    let path = sidecars(&p)[0].clone();
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&path)
                        .unwrap();
                    path
                }
            };
            let result = std::process::Command::new("/bin/chmod")
                .args(["+a", "everyone allow read"])
                .arg(&acl_path)
                .status()
                .unwrap();
            assert!(
                result.success(),
                "synthetic ACL fixture could not be created"
            );
            assert!(state.recheck().is_err());
            assert!(prepare(&p).is_err());
        }
    }
}
