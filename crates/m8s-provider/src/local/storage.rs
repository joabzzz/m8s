//! Private descriptor-anchored journal. No repair or stale-lock replacement.
use crate::{Error, Result};
use fs2::FileExt;
use std::{
    ffi::CStr,
    fs::{File, Metadata, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

pub(crate) const MAX_BYTES: usize = 1024 * 1024;
type Identity = (u64, u64);
fn identity(m: &Metadata) -> Identity {
    (m.dev(), m.ino())
}
fn private(m: &Metadata, directory: bool) -> Result<()> {
    // SAFETY: geteuid has no pointer arguments or side effects.
    private_with_uid(m, directory, unsafe { libc::geteuid() })
}
fn private_with_uid(m: &Metadata, directory: bool, uid: u32) -> Result<()> {
    if m.uid() != uid
        || m.mode() & 0o7777 != if directory { 0o700 } else { 0o600 }
        || if directory {
            !m.is_dir()
        } else {
            !m.is_file() || m.nlink() != 1
        }
    {
        return Err(Error::Storage);
    }
    Ok(())
}
fn open_directory(path: &Path) -> Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| Error::Storage)?;
    private(&f.metadata().map_err(|_| Error::Storage)?, true)?;
    Ok(f)
}
fn relative(dir: &File, name: &CStr, flags: i32) -> std::io::Result<File> {
    // SAFETY: dir is a live directory descriptor; name is a constant single
    // component C string. Every successful returned descriptor is owned once.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: openat just returned a fresh valid descriptor owned by this call.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn checked(dir: &File, name: &CStr) -> Result<Option<File>> {
    match relative(dir, name, libc::O_RDONLY) {
        Ok(f) => {
            private(&f.metadata().map_err(|_| Error::Storage)?, false)?;
            Ok(Some(f))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Error::Storage),
    }
}

pub(crate) struct Store {
    path: PathBuf,
    dir: File,
    directory_id: Identity,
    lock_id: Identity,
    state_id: Option<Identity>,
    _lock: File,
}
impl Store {
    pub(crate) fn open(path: &Path) -> Result<(Self, Option<Vec<u8>>)> {
        // Resolve relative spelling only, not symlinks. Final component must be
        // a real directory. Ancestors must be trusted; validate catches anchor
        // rename/replacement at each API boundary and before/after persistence.
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()
                .map_err(|_| Error::Storage)?
                .join(path)
        };
        let path: PathBuf = path.components().collect();
        let dir = open_directory(&path)?;
        let directory_id = identity(&dir.metadata().map_err(|_| Error::Storage)?);
        let (lock, fresh) =
            match relative(&dir, c"lock", libc::O_RDWR | libc::O_CREAT | libc::O_EXCL) {
                Ok(f) => (f, true),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (
                    relative(&dir, c"lock", libc::O_RDWR).map_err(|_| Error::Storage)?,
                    false,
                ),
                Err(_) => return Err(Error::Storage),
            };
        let metadata = lock.metadata().map_err(|_| Error::Storage)?;
        private(&metadata, false)?;
        let lock_id = identity(&metadata);
        lock.try_lock_exclusive().map_err(|_| Error::Unavailable)?;
        let state = checked(&dir, c"state.json")?;
        // Neither missing old state nor pre-existing state without its original
        // lock is a fresh journal. Do not reset identity or repair either case.
        if fresh == state.is_some() {
            return Err(Error::Storage);
        }
        let state_id = state
            .as_ref()
            .map(|f| f.metadata().map(|m| identity(&m)))
            .transpose()
            .map_err(|_| Error::Storage)?;
        let bytes = state
            .map(|f| {
                if f.metadata().map_err(|_| Error::Storage)?.len() > MAX_BYTES as u64 {
                    return Err(Error::Storage);
                }
                let mut bytes = Vec::new();
                f.take(MAX_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| Error::Storage)?;
                if bytes.len() > MAX_BYTES {
                    return Err(Error::Storage);
                }
                Ok(bytes)
            })
            .transpose()?;
        let store = Self {
            path,
            dir,
            directory_id,
            lock_id,
            state_id,
            _lock: lock,
        };
        store.validate()?;
        Ok((store, bytes))
    }
    pub(crate) fn namespace_bytes(&self) -> &[u8] {
        self.path.as_os_str().as_encoded_bytes()
    }
    pub(crate) fn validate(&self) -> Result<()> {
        private(&self.dir.metadata().map_err(|_| Error::Storage)?, true)?;
        let anchor = open_directory(&self.path)?;
        if identity(&anchor.metadata().map_err(|_| Error::Storage)?) != self.directory_id {
            return Err(Error::Storage);
        }
        private(&self._lock.metadata().map_err(|_| Error::Storage)?, false)?;
        let lock = checked(&self.dir, c"lock")?.ok_or(Error::Storage)?;
        if identity(&lock.metadata().map_err(|_| Error::Storage)?) != self.lock_id {
            return Err(Error::Storage);
        }
        let state_id = checked(&self.dir, c"state.json")?
            .map(|f| f.metadata().map(|m| identity(&m)))
            .transpose()
            .map_err(|_| Error::Storage)?;
        if state_id != self.state_id {
            return Err(Error::Storage);
        }
        Ok(())
    }
    pub(crate) fn save(&mut self, bytes: &[u8]) -> Result<()> {
        self.validate()?;
        if bytes.len() > MAX_BYTES {
            return Err(Error::Storage);
        }
        let mut file = relative(
            &self.dir,
            c"state.pending",
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )
        .map_err(|_| Error::Storage)?;
        let metadata = file.metadata().map_err(|_| Error::Storage)?;
        private(&metadata, false)?;
        let pending_id = identity(&metadata);
        file.write_all(bytes).map_err(|_| Error::Storage)?;
        file.sync_all().map_err(|_| Error::Storage)?;
        self.validate()?;
        let pending = checked(&self.dir, c"state.pending")?.ok_or(Error::Storage)?;
        if identity(&pending.metadata().map_err(|_| Error::Storage)?) != pending_id {
            return Err(Error::Storage);
        }
        // SAFETY: both names are constant single components under the same live
        // directory descriptor; no ambient path resolution or unlink repair.
        if unsafe {
            libc::renameat(
                self.dir.as_raw_fd(),
                c"state.pending".as_ptr(),
                self.dir.as_raw_fd(),
                c"state.json".as_ptr(),
            )
        } != 0
        {
            return Err(Error::Storage);
        }
        self.state_id = Some(pending_id);
        self.dir.sync_all().map_err(|_| Error::Storage)?;
        self.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fstat_owner_mismatch_and_descriptor_inheritance_fail_closed() {
        use std::os::unix::fs::PermissionsExt;
        let path =
            std::env::temp_dir().join(format!("m8s-local-storage-test-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (store, _) = Store::open(&path).unwrap();
        for (file, directory) in [(&store.dir, true), (&store._lock, false)] {
            let m = file.metadata().unwrap();
            assert!(private_with_uid(&m, directory, m.uid()).is_ok());
            assert!(private_with_uid(&m, directory, m.uid().wrapping_add(1)).is_err());
            // SAFETY: fcntl queries flags on our live owned fixture descriptor.
            assert_ne!(
                unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
                0
            );
        }
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
}
