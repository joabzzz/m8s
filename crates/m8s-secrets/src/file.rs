//! Unix descriptor-relative storage; no path-based reopen after validation.
use crate::{Error, SecretInfo, SecretName, SecretStore, SecretValue, codec};
use fs2::FileExt;
use std::{
    ffi::{CStr, CString},
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use zeroize::Zeroizing;

pub struct FileStore {
    dir: File,
    path: PathBuf,
    name: CString,
    lock_name: CString,
}
fn io<T>(r: std::io::Result<T>) -> Result<T, Error> {
    r.map_err(|_| Error::Io)
}
fn cstr(s: &[u8]) -> Result<CString, Error> {
    CString::new(s).map_err(|_| Error::UnsafePath)
}
fn open_at(dir: &File, name: &CStr, flags: i32) -> std::io::Result<File> {
    for attempt in 0..16 {
        // SAFETY: valid borrowed descriptor and NUL-terminated name, mode supplied for O_CREAT.
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                0o600,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            // Darwin may return ENOENT during concurrent O_CREAT of the same name.
            // Retry only creation (descriptor-relative, same restrictive flags), bounded.
            if attempt < 15
                && (error.kind() == std::io::ErrorKind::Interrupted
                    || (flags & libc::O_CREAT != 0 && error.raw_os_error() == Some(libc::ENOENT)))
            {
                continue;
            }
            return Err(error);
        }
        // SAFETY: openat returned a fresh owned descriptor.
        return Ok(unsafe { File::from_raw_fd(fd) });
    }
    unreachable!("bounded open loop always returns on the last iteration")
}
fn uid() -> u32 {
    // SAFETY: geteuid has no preconditions or effects.
    unsafe { libc::geteuid() }
}
fn private_file(f: &File) -> Result<(), Error> {
    let m = io(f.metadata())?;
    file_metadata(&m, uid())?;
    crate::acl::reject_extended(f)
}
fn file_metadata(m: &std::fs::Metadata, owner: u32) -> Result<(), Error> {
    if !m.is_file() || m.uid() != owner || m.mode() & 0o7777 != 0o600 || m.nlink() != 1 {
        return Err(Error::UnsafePath);
    }
    Ok(())
}
fn private_dir(f: &File) -> Result<(), Error> {
    let m = io(f.metadata())?;
    if !m.is_dir() || m.uid() != uid() || m.mode() & 0o7777 != 0o700 {
        return Err(Error::UnsafePath);
    }
    crate::acl::reject_extended(f)
}
fn no_git(dir: &File) -> Result<(), Error> {
    let name = c".git";
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat points to sufficient writable storage; no dereference of uninitialized bytes.
    let ret = unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if ret == 0 {
        return Err(Error::UnsafePath);
    }
    if std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT) {
        return Err(Error::Io);
    }
    Ok(())
}
impl FileStore {
    /// Explicit absolute path only. Parent must already exist, be owned and exactly 0700.
    /// All components must be real directories (including /private/tmp on macOS, not /tmp).
    /// No auto-discovery, ambient env reads, directory creation, or permission repair.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let (dir, name) = Self::parent(path)?;
        let lock_name = cstr(&[name.as_bytes(), b".lock"].concat())?;
        let store = Self {
            dir,
            path: path.to_owned(),
            name,
            lock_name,
        };
        store.transaction(|_| Ok(()), false)?;
        Ok(store)
    }
    fn parent(path: &Path) -> Result<(File, CString), Error> {
        if !path.is_absolute() {
            return Err(Error::UnsafePath);
        }
        if path
            .as_os_str()
            .as_bytes()
            .split(|b| *b == b'/')
            .any(|s| s == b"." || s == b"..")
        {
            return Err(Error::UnsafePath);
        }
        let mut components = path.components().peekable();
        if components.next() != Some(Component::RootDir) {
            return Err(Error::UnsafePath);
        }
        let mut dir = io(File::open("/"))?;
        no_git(&dir)?;
        let name;
        loop {
            let component = match components.next() {
                Some(Component::Normal(c)) => c,
                _ => return Err(Error::UnsafePath),
            };
            if components.peek().is_none() {
                name = cstr(component.as_bytes())?;
                break;
            }
            dir = open_at(
                &dir,
                &cstr(component.as_bytes())?,
                libc::O_RDONLY | libc::O_DIRECTORY,
            )?;
            let m = io(dir.metadata())?;
            // Trusted ancestors may be root-owned sticky temporary directories.
            if (m.uid() != 0 && m.uid() != uid())
                || (m.mode() & 0o022 != 0 && !(m.uid() == 0 && m.mode() & 0o1000 != 0))
            {
                return Err(Error::UnsafePath);
            }
            no_git(&dir)?;
        }
        private_dir(&dir)?;
        if name.as_bytes() == b".git" {
            return Err(Error::UnsafePath);
        }
        Ok((dir, name))
    }
    fn load(&self) -> Result<codec::Entries, Error> {
        let mut f = match open_at(&self.dir, &self.name, libc::O_RDONLY) {
            Ok(f) => f,
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => return Ok(codec::Entries::new()),
            Err(_) => return Err(Error::Io),
        };
        private_file(&f)?;
        if io(f.metadata())?.len() > codec::MAX_BYTES as u64 {
            return Err(Error::InvalidData);
        }
        let mut bytes = Zeroizing::new(Vec::with_capacity(codec::MAX_BYTES + 1));
        io((&mut f)
            .take((codec::MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes))?;
        codec::decode(&bytes)
    }
    fn save(&self, entries: &codec::Entries) -> Result<(), Error> {
        let bytes = codec::encode(entries)?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut temporary = None;
        for _ in 0..128 {
            let name = cstr(
                format!(
                    ".m8s-secrets-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                )
                .as_bytes(),
            )?;
            match open_at(
                &self.dir,
                &name,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            ) {
                Ok(f) => {
                    temporary = Some((name, f));
                    break;
                }
                Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {
                    continue;
                }
                Err(_) => return Err(Error::Io),
            }
        }
        let (name, mut file) = temporary.ok_or(Error::Io)?;
        let result = (|| {
            private_file(&file)?;
            io(file.write_all(&bytes))?;
            io(file.sync_all())?;
            // Revalidate the existing destination under the lock, never follow it.
            self.load()?;
            // SAFETY: both names and directory descriptor remain live.
            if unsafe {
                libc::renameat(
                    self.dir.as_raw_fd(),
                    name.as_ptr(),
                    self.dir.as_raw_fd(),
                    self.name.as_ptr(),
                )
            } != 0
            {
                return Err(Error::Io);
            }
            io(self.dir.sync_all())
        })();
        // SAFETY: unlink only our exclusive temporary name, not the destination.
        unsafe {
            libc::unlinkat(self.dir.as_raw_fd(), name.as_ptr(), 0);
        }
        result
    }
    fn transaction<T>(
        &self,
        f: impl FnOnce(&mut codec::Entries) -> Result<T, Error>,
        write: bool,
    ) -> Result<T, Error> {
        let (current, _) = Self::parent(&self.path)?;
        let a = io(current.metadata())?;
        let b = io(self.dir.metadata())?;
        if a.dev() != b.dev() || a.ino() != b.ino() {
            return Err(Error::UnsafePath);
        }
        private_dir(&self.dir)?;
        no_git(&self.dir)?;
        let lock = open_at(&self.dir, &self.lock_name, libc::O_RDWR | libc::O_CREAT)?;
        private_file(&lock)?;
        io(lock.lock_exclusive())?;
        let mut entries = self.load()?;
        let result = f(&mut entries)?;
        if write {
            self.save(&entries)?;
        }
        // File drop releases the lock on all paths.
        Ok(result)
    }
}
impl SecretStore for FileStore {
    fn backend(&self) -> &'static str {
        "file"
    }
    fn get(&self, name: &SecretName, now: i64) -> Result<Option<SecretValue>, Error> {
        self.transaction(|e| Ok(codec::get(e, name, now)), false)
    }
    fn put(
        &mut self,
        name: SecretName,
        value: SecretValue,
        expires_at: Option<i64>,
    ) -> Result<(), Error> {
        self.transaction(
            |e| {
                e.insert(name, (value, expires_at));
                Ok(())
            },
            true,
        )
    }
    fn delete(&mut self, name: &SecretName) -> Result<bool, Error> {
        self.transaction(|e| Ok(e.remove(name).is_some()), true)
    }
    fn list(&self, now: i64) -> Result<Vec<SecretInfo>, Error> {
        self.transaction(|e| Ok(codec::list(e, now)), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_is_not_inferred_from_mode_bits() {
        #[cfg(target_os = "macos")]
        let base = "/private/tmp";
        #[cfg(not(target_os = "macos"))]
        let base = "/tmp";
        let dir = tempfile::Builder::new()
            .prefix("m8s-synthetic-owner-")
            .tempdir_in(base)
            .unwrap();
        let file = open_at(
            &File::open(dir.path()).unwrap(),
            c"synthetic",
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        )
        .unwrap();
        let m = file.metadata().unwrap();
        assert_eq!(
            file_metadata(&m, m.uid().wrapping_add(1)),
            Err(Error::UnsafePath)
        );
    }
}
