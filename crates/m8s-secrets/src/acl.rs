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
