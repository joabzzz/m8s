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
pub(crate) struct PrivateState {
    pub path: PathBuf,
    directory: File,
    database: File,
}
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
#[cfg(unix)]
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
#[cfg(not(unix))]
pub(crate) fn prepare(_path: &Path) -> Result<PrivateState> {
    Err(Error::PrivateState)
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
