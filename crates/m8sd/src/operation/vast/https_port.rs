//! Production `RestartPort`: exact-scope Vast adapters with an injected credential
//! supplier. Construction performs no I/O; each call builds a fresh client,
//! authenticates the original creator account where the adapter requires it, and
//! drops the secret. There is no create, inventory or adoption method.
use super::*;
use m8s_provider::vast::readonly::{recovery::RecoveryLookup, restart::SecretValue};

pub struct HttpsRestartPort<F> {
    supplier: F,
}
impl<F> HttpsRestartPort<F>
where
    F: Fn() -> m8s_provider::Result<SecretValue> + Clone,
{
    pub fn new(supplier: F) -> Self {
        Self { supplier }
    }
    fn cleanup(
        &self,
        recovery: &VastRecovery,
        instance: NonZeroU64,
    ) -> m8s_provider::Result<m8s_provider::vast::readonly::restart::RestartCleanup<F>> {
        // Only the journaled binding may be touched; a caller-supplied ID that
        // differs from it is refused before any request.
        let bound = recovery
            .operation()
            .result
            .as_ref()
            .and_then(|b| b.instance_id.parse::<NonZeroU64>().ok());
        if bound != Some(instance) {
            return Err(m8s_provider::Error::Invalid);
        }
        recovery
            .bound_cleanup(self.supplier.clone())
            .map_err(|_| m8s_provider::Error::Invalid)
    }
}
impl<F> RestartPort for HttpsRestartPort<F>
where
    F: Fn() -> m8s_provider::Result<SecretValue> + Clone,
{
    fn lookup(&mut self, recovery: &VastRecovery) -> m8s_provider::Result<Option<NonZeroU64>> {
        let mut lookup = RecoveryLookup::new(recovery.identity().clone(), self.supplier.clone())?;
        Ok(lookup.find()?.map(|c| c.instance_id()))
    }
    fn destroy(
        &mut self,
        recovery: &VastRecovery,
        instance: NonZeroU64,
    ) -> m8s_provider::Result<()> {
        self.cleanup(recovery, instance)?.destroy().map(|_| ())
    }
    fn confirm_absent(
        &mut self,
        recovery: &VastRecovery,
        instance: NonZeroU64,
    ) -> m8s_provider::Result<Option<Absence>> {
        Ok(self
            .cleanup(recovery, instance)?
            .confirm_absent()?
            .map(Absence::from_provider))
    }
}

/// Explicitly selected dotenv-style file holding one `VAST_API_KEY=` line. Read
/// on every call so nothing secret is retained; never searched for, logged,
/// serialized or placed in argv. Refuses symlinks, non-owned, group/other
/// accessible, hard-linked, oversized or ambiguous files.
pub fn credential_supplier(
    path: std::path::PathBuf,
) -> impl Fn() -> m8s_provider::Result<SecretValue> + Clone {
    move || read_credential(&path)
}

pub fn read_credential(path: &std::path::Path) -> m8s_provider::Result<SecretValue> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let unavailable = || m8s_provider::Error::Unavailable;
    if !path.is_absolute() {
        return Err(unavailable());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| unavailable())?;
    let meta = file.metadata().map_err(|_| unavailable())?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.nlink() != 1
        || meta.len() > 65536
    {
        return Err(unavailable());
    }
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    let text = std::str::from_utf8(&bytes).map_err(|_| unavailable())?;
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some(value) = line.strip_prefix("VAST_API_KEY=") else {
            continue;
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        if found.is_some()
            || value.is_empty()
            || value.len() > 4096
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(unavailable());
        }
        found = Some(SecretValue::new(value.as_bytes().to_vec()));
    }
    found.ok_or_else(unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn file(name: &str, body: &str, mode: u32) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("m8sd-cred-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    #[test]
    fn only_a_private_single_unambiguous_key_is_read() {
        let ok = file(
            "ok",
            "# comment\nexport VAST_API_KEY=\"synthetic-key_1.2\"\n",
            0o600,
        );
        assert_eq!(read_credential(&ok).unwrap().expose(), b"synthetic-key_1.2");
        let supplier = credential_supplier(ok.clone());
        assert_eq!(supplier().unwrap().expose(), b"synthetic-key_1.2");
        for (name, body, mode) in [
            ("open", "VAST_API_KEY=synthetic\n", 0o644),
            ("group", "VAST_API_KEY=synthetic\n", 0o640),
            ("dup", "VAST_API_KEY=a\nVAST_API_KEY=b\n", 0o600),
            ("empty", "VAST_API_KEY=\n", 0o600),
            ("missing", "OTHER=a\n", 0o600),
            ("shell", "VAST_API_KEY=$(whoami)\n", 0o600),
        ] {
            let path = file(name, body, mode);
            assert!(read_credential(&path).is_err(), "{name}");
            std::fs::remove_file(path).unwrap();
        }
        let link = std::env::temp_dir().join(format!("m8sd-cred-{}-link", std::process::id()));
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&ok, &link).unwrap();
        assert!(read_credential(&link).is_err(), "symlink");
        assert!(read_credential(std::path::Path::new("relative")).is_err());
        std::fs::remove_file(link).unwrap();
        std::fs::remove_file(ok).unwrap();
    }

    #[test]
    fn port_refuses_an_instance_other_than_the_journaled_binding_before_any_request() {
        use crate::operation::vast::tests::{executor, setup};
        let (store, _directory) = setup(6_000_000, 1);
        let executor = executor(42);
        let intent = executor.intent("run", "request", "operation").unwrap();
        executor.prepare_at(&store, &intent, || Ok(100)).unwrap();
        let recovery = store.recover_vast("operation").unwrap();
        // The supplier would fail the test if any request path asked for a key.
        let mut port = HttpsRestartPort::new(|| -> m8s_provider::Result<SecretValue> {
            panic!("credential requested without a journaled binding")
        });
        let other = NonZeroU64::new(43).unwrap();
        assert!(port.destroy(&recovery, other).is_err());
        assert!(port.confirm_absent(&recovery, other).is_err());
    }
}
