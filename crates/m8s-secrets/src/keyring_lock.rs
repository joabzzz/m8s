//! Advisory whole-envelope coordination for explicitly qualified exclusive writers.
//! No data or credentials are stored in these stable, empty lock files.
use crate::{
    Error, KeyringIdentity,
    file::{FileStore, open_at, private_file},
};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::File,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, TryLockError, Weak},
    time::{Duration, Instant},
};

#[derive(PartialEq, Eq)]
struct Fingerprint {
    directory: (u64, u64),
    lock: (u64, u64),
}
struct ProcessDomain {
    fingerprint: Fingerprint,
    gate: Mutex<()>,
}
type Registry = BTreeMap<KeyringIdentity, Weak<ProcessDomain>>;
static DOMAINS: OnceLock<Mutex<Registry>> = OnceLock::new();

/// A security-contract acknowledgement, NOT proof of external-client exclusivity.
/// Every writer for this service/account must use this same private directory,
/// identity and protocol. No raw driver, other application, alternate directory,
/// lock replacement or legacy writer may access the item concurrently. Qualify
/// that operational contract before production use. CLI defaults stay unavailable.
/// No Debug/Serialize and no secret bytes. Dropping this does not delete the lock.
pub struct KeyringLockDomain {
    identity: KeyringIdentity,
    path: PathBuf,
    directory: File,
    name: CString,
    pinned_lock: File,
    process: Arc<ProcessDomain>,
    wait_budget: Duration,
}
fn inode(file: &File) -> Result<(u64, u64), Error> {
    let m = file.metadata().map_err(|_| Error::Io)?;
    Ok((m.dev(), m.ino()))
}
fn validate_lock(file: &File) -> Result<(), Error> {
    private_file(file)?;
    if file.metadata().map_err(|_| Error::Io)?.len() != 0 {
        return Err(Error::UnsafePath);
    }
    Ok(())
}
fn retry_before(deadline: Instant) -> Result<(), Error> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(Error::LockTimeout)?;
    std::thread::sleep(remaining.min(Duration::from_millis(2)));
    if Instant::now() >= deadline {
        return Err(Error::LockTimeout);
    }
    Ok(())
}
impl KeyringLockDomain {
    /// Explicit acknowledgement of the exclusive-writer deployment contract.
    /// The caller must provision the trusted owner-0700 directory outside Git;
    /// this creates only the identity-derived empty owner-0600 stable lock file.
    /// No ambient discovery, directory creation, permission repair or OS keyring access.
    /// Concurrent live conflicting domains for this identity in this process fail.
    /// Across applications/processes, exclusivity remains an operational obligation.
    pub fn acknowledge_exclusive_writers(
        directory: &Path,
        identity: KeyringIdentity,
    ) -> Result<Self, Error> {
        Self::acknowledge_exclusive_writers_with_timeout(
            directory,
            identity,
            Duration::from_secs(5),
        )
    }
    /// Same security acknowledgement, with an explicit bounded contention budget
    /// (0..=30 seconds, shared by mutex and flock acquisition). Zero means try once.
    /// This does not interrupt filesystem I/O or an already running driver call.
    pub fn acknowledge_exclusive_writers_with_timeout(
        directory: &Path,
        identity: KeyringIdentity,
        wait_budget: Duration,
    ) -> Result<Self, Error> {
        if wait_budget > Duration::from_secs(30) {
            return Err(Error::InvalidData);
        }
        let mut hash = Sha256::new();
        hash.update(b"m8s-keyring-lock-v1\0");
        for part in [identity.service(), identity.account().as_str()] {
            hash.update((part.len() as u32).to_le_bytes());
            hash.update(part.as_bytes());
        }
        // Digest contains identity metadata, never values.
        let filename = format!("keyring-v1-{:x}.lock", hash.finalize());
        let path = directory.join(filename);
        let (directory, name) = FileStore::parent(&path)?;
        let pinned_lock =
            open_at(&directory, &name, libc::O_RDWR | libc::O_CREAT).map_err(|_| Error::Io)?;
        validate_lock(&pinned_lock)?;
        pinned_lock.sync_all().map_err(|_| Error::Io)?;
        directory.sync_all().map_err(|_| Error::Io)?;
        let fingerprint = Fingerprint {
            directory: inode(&directory)?,
            lock: inode(&pinned_lock)?,
        };
        let mut domains = DOMAINS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|_| Error::Backend)?;
        domains.retain(|_, domain| domain.strong_count() != 0);
        let process = match domains.get(&identity).and_then(Weak::upgrade) {
            Some(domain) if domain.fingerprint == fingerprint => domain,
            Some(_) => return Err(Error::UnsafePath),
            None => {
                let domain = Arc::new(ProcessDomain {
                    fingerprint,
                    gate: Mutex::new(()),
                });
                domains.insert(identity.clone(), Arc::downgrade(&domain));
                domain
            }
        };
        drop(domains);
        let domain = Self {
            identity,
            path,
            directory,
            name,
            pinned_lock,
            process,
            wait_budget,
        };
        domain.revalidate()?;
        Ok(domain)
    }
    pub(crate) fn identity(&self) -> &KeyringIdentity {
        &self.identity
    }
    fn revalidate(&self) -> Result<File, Error> {
        // Rewalk all ancestors, ownership/ACL/noGit checks and pinned directory.
        let (directory, _) = FileStore::parent(&self.path)?;
        if inode(&directory)? != self.process.fingerprint.directory
            || inode(&self.directory)? != self.process.fingerprint.directory
        {
            return Err(Error::UnsafePath);
        }
        validate_lock(&self.pinned_lock)?;
        let lock = open_at(&self.directory, &self.name, libc::O_RDWR).map_err(|_| Error::Io)?;
        validate_lock(&lock)?;
        if inode(&lock)? != self.process.fingerprint.lock {
            return Err(Error::UnsafePath);
        }
        Ok(lock)
    }
    pub(crate) fn transaction<T>(
        &self,
        operation: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        let deadline = Instant::now()
            .checked_add(self.wait_budget)
            .ok_or(Error::InvalidData)?;
        let _process = loop {
            match self.process.gate.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::Poisoned(_)) => return Err(Error::Backend),
                Err(TryLockError::WouldBlock) => retry_before(deadline)?,
            }
        };
        // Fresh independent descriptor per operation, never a shared/fork-inherited
        // flock description. Keep both guards through load, mutation and save.
        let lock = self.revalidate()?;
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    retry_before(deadline)?
                }
                Err(_) => return Err(Error::Io),
            }
        }
        self.revalidate()?; // Also reject replacement/permission changes while waiting.
        let result = operation();
        // An error may mean the backend saved; never promise rollback or absence.
        // Drop releases flock even on driver/codec error or unwind.
        drop(lock);
        result
    }
}

#[cfg(test)]
#[path = "keyring_lock_tests.rs"]
mod tests;
