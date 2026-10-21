//! Synchronous secret stores. Backends are selected explicitly; never fall back implicitly.
//! FileStore uses the strict bounded versioned TOML schema described in README.md.
mod codec;
mod keychain;
#[cfg(unix)]
mod toml;
pub mod ux;
#[cfg(all(target_os = "macos", feature = "os-keyring", not(test)))]
pub use keychain::MacKeyringDriver;
pub use keychain::{KeyringDriver, KeyringStore};
#[cfg(unix)]
mod acl;
#[cfg(unix)]
mod file;
#[cfg(unix)]
pub use file::FileStore;
use std::collections::BTreeMap;
use zeroize::Zeroize;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SecretName(String);
impl SecretName {
    pub fn new(name: &str) -> Result<Self, Error> {
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'/')
            || name.starts_with('/')
            || name.ends_with('/')
            || name.contains("//")
        {
            return Err(Error::InvalidName);
        }
        Ok(Self(name.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
/// Intentionally no Debug, Display, Clone, or Serialize.
/// ```compile_fail
/// let value = m8s_secrets::SecretValue::new(vec![]);
/// println!("{value:?}");
/// ```
/// ```compile_fail
/// let value = m8s_secrets::SecretValue::new(vec![]);
/// println!("{value}");
/// ```
pub struct SecretValue(Vec<u8>);
impl SecretValue {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    /// Explicit exposure at the final consumer boundary only.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}
impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretInfo {
    pub name: SecretName,
    pub expires_at: Option<i64>,
}
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid secret name")]
    InvalidName,
    #[error("unsafe secret store path or permissions")]
    UnsafePath,
    #[error("secret store I/O failed")]
    Io,
    #[error("invalid or oversized secret store data")]
    InvalidData,
    #[error("secret backend failed; no fallback was attempted")]
    Backend,
    #[error("backend is read-only")]
    ReadOnly,
    #[error("requested backend is unavailable; no fallback was attempted")]
    Unavailable,
}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}
pub trait SecretStore: Send + Sync {
    fn backend(&self) -> &'static str;
    fn get(&self, name: &SecretName, now: i64) -> Result<Option<SecretValue>, Error>;
    fn put(
        &mut self,
        name: SecretName,
        value: SecretValue,
        expires_at: Option<i64>,
    ) -> Result<(), Error>;
    fn delete(&mut self, name: &SecretName) -> Result<bool, Error>;
    fn list(&self, now: i64) -> Result<Vec<SecretInfo>, Error>;
}
/// Inject only the intended environment keys. Does not inspect the ambient process environment.
pub struct EnvStore {
    entries: BTreeMap<SecretName, (SecretValue, Option<i64>)>,
}
impl EnvStore {
    pub fn injected(
        entries: impl IntoIterator<Item = (SecretName, SecretValue, Option<i64>)>,
    ) -> Self {
        Self {
            entries: entries.into_iter().map(|(n, v, e)| (n, (v, e))).collect(),
        }
    }
}
impl SecretStore for EnvStore {
    fn backend(&self) -> &'static str {
        "env"
    }
    fn get(&self, name: &SecretName, now: i64) -> Result<Option<SecretValue>, Error> {
        Ok(self
            .entries
            .get(name)
            .filter(|(_, expiry)| expiry.is_none_or(|e| now < e))
            .map(|(v, _)| SecretValue::new(v.expose().to_vec())))
    }
    fn put(&mut self, _: SecretName, _: SecretValue, _: Option<i64>) -> Result<(), Error> {
        Err(Error::ReadOnly)
    }
    fn delete(&mut self, _: &SecretName) -> Result<bool, Error> {
        Err(Error::ReadOnly)
    }
    fn list(&self, now: i64) -> Result<Vec<SecretInfo>, Error> {
        Ok(self
            .entries
            .iter()
            .filter(|(_, (_, e))| e.is_none_or(|e| now < e))
            .map(|(n, (_, e))| SecretInfo {
                name: n.clone(),
                expires_at: *e,
            })
            .collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_expiry_are_enforced_without_values_in_metadata() {
        for name in ["", "UPPER", "a_b", "../x", "/x", "x/", "x//y"] {
            assert!(SecretName::new(name).is_err());
        }
        let n = SecretName::new("test/session").unwrap();
        let mut s = EnvStore::injected([(
            n.clone(),
            SecretValue::new(b"synthetic-only".to_vec()),
            Some(10),
        )]);
        assert_eq!(s.get(&n, 9).unwrap().unwrap().expose(), b"synthetic-only");
        assert!(s.get(&n, 10).unwrap().is_none());
        assert!(s.list(10).unwrap().is_empty());
        assert_eq!(s.delete(&n), Err(Error::ReadOnly));
        assert_eq!(
            s.put(n, SecretValue::new(vec![]), None),
            Err(Error::ReadOnly)
        );
    }
}

#[cfg(test)]
mod backend_tests;
