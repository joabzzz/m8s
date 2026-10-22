//! Reject extended ACLs: mode bits alone do not establish a private file.
use crate::Error;
use std::{fs::File, os::fd::AsRawFd};

#[cfg(target_os = "macos")]
pub(crate) fn reject_extended(file: &File) -> Result<(), Error> {
    use std::ffi::c_void;
    unsafe extern "C" {
        fn acl_get_fd_np(fd: i32, kind: u32) -> *mut c_void;
        fn acl_free(obj: *mut c_void) -> i32;
    }
    // SAFETY: descriptor is borrowed, ACL_TYPE_EXTENDED is defined by sys/acl.h.
    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
    if acl.is_null() {
        // A valid open descriptor with no extended ACL returns ENOENT on Darwin.
        return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            Ok(())
        } else {
            Err(Error::UnsafePath)
        };
    }
    // Reject any extended ACL, even an empty one; never interpret allow/deny semantics.
    // SAFETY: the allocated ACL is freed exactly once.
    unsafe {
        acl_free(acl);
    }
    Err(Error::UnsafePath)
}
#[cfg(target_os = "linux")]
pub(crate) fn reject_extended(file: &File) -> Result<(), Error> {
    // SAFETY: name is NUL-terminated; null buffer/zero length queries only the size.
    let ret = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            c"system.posix_acl_access".as_ptr(),
            std::ptr::null_mut(),
            0,
        )
    };
    if ret >= 0 {
        return Err(Error::UnsafePath);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ENODATA) | Some(libc::ENOTSUP) => Ok(()),
        _ => Err(Error::UnsafePath),
    }
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn reject_extended(_: &File) -> Result<(), Error> {
    Err(Error::Unavailable)
}

// Composition seam tests denial without creating or interpreting actual ACLs.
fn check_queries(
    access: impl FnOnce() -> Result<(), Error>,
    default: impl FnOnce() -> Result<(), Error>,
) -> Result<(), Error> {
    access()?;
    default()
}
pub(crate) fn check_no_extended(file: &File) -> Result<(), Error> {
    check_queries(
        || reject_extended(file),
        || {
            #[cfg(target_os = "linux")]
            return reject_default(file);
            // Darwin's existing extended-ACL rejection includes inheritance flags.
            // Other Unix platforms already fail closed in reject_extended above.
            #[cfg(not(target_os = "linux"))]
            Ok(())
        },
    )
}
#[cfg(any(target_os = "linux", test))]
fn default_query_result(ret: isize, missing: bool, unsupported: bool) -> Result<(), Error> {
    if ret == -1 && (missing || unsupported) {
        Ok(())
    } else {
        // Any ACL (including zero-length/deny-only) or unknown query failure refuses.
        Err(Error::UnsafePath)
    }
}
#[cfg(target_os = "linux")]
fn reject_default(file: &File) -> Result<(), Error> {
    // SAFETY: borrowed valid fd, constant NUL-terminated name, null buffer/zero
    // length query only size. No ACL bytes are read, modified or interpreted.
    let ret = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            c"system.posix_acl_default".as_ptr(),
            std::ptr::null_mut(),
            0,
        )
    };
    let code = std::io::Error::last_os_error().raw_os_error();
    default_query_result(
        ret,
        code == Some(libc::ENODATA),
        code == Some(libc::ENOTSUP),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn injected_access_default_and_unknown_failures_are_not_absence() {
        for (access, default) in [
            (Err(Error::UnsafePath), Ok(())),
            (Ok(()), Err(Error::UnsafePath)),
            (Err(Error::Unavailable), Ok(())),
            (Ok(()), Err(Error::Unavailable)),
        ] {
            assert!(check_queries(|| access, || default).is_err());
        }
        assert_eq!(check_queries(|| Ok(()), || Ok(())), Ok(()));
        let called = std::cell::Cell::new(false);
        assert!(
            check_queries(
                || Err(Error::UnsafePath),
                || {
                    called.set(true);
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!called.get());
    }
    #[test]
    fn pure_default_query_classification_rejects_presence_and_unknown_errors() {
        for ret in [0, 1, 1024] {
            assert_eq!(
                default_query_result(ret, false, false),
                Err(Error::UnsafePath)
            );
        }
        assert_eq!(
            default_query_result(-1, false, false),
            Err(Error::UnsafePath)
        );
        assert_eq!(default_query_result(-1, true, false), Ok(()));
        assert_eq!(default_query_result(-1, false, true), Ok(()));
    }
    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn actual_borrowed_own_no_acl_file_and_directory() {
        let file = tempfile::tempfile().unwrap();
        crate::check_no_extended_acl(&file).unwrap();
        let dir = tempfile::tempdir().unwrap();
        crate::check_no_extended_acl(&File::open(dir.path()).unwrap()).unwrap();
    }
}
