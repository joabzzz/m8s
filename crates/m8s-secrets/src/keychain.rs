use crate::{Error, SecretInfo, SecretName, SecretStore, SecretValue, codec};
use std::sync::Mutex;
use zeroize::Zeroizing;

/// Stable non-secret backend service/account identity. No values or formatting.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyringIdentity {
    service: SecretName,
    account: SecretName,
}
impl KeyringIdentity {
    pub fn new(service: &str, account: SecretName) -> Result<Self, Error> {
        Ok(Self {
            service: SecretName::new(service)?,
            account,
        })
    }
    pub fn service(&self) -> &str {
        self.service.as_str()
    }
    pub fn account(&self) -> &SecretName {
        &self.account
    }
}
/// Injected byte driver for ONE namespaced vault item, not arbitrary OS accounts.
/// Drivers must not log errors/values. Errors are deliberately opaque.
/// Atomic replacement is required. identity() must be pure and immutable; return
/// Some for every backend with independent clients. None is ONLY for isolated
/// synthetic/in-memory drivers, not an escape hatch for OS keyring callers.
/// All external writers must honor the same qualified lock domain; this trait
/// cannot make uncooperative third-party/raw backend clients safe.
pub trait KeyringDriver: Send + Sync {
    fn identity(&self) -> Option<KeyringIdentity> {
        None
    }
    fn load(&self) -> Result<Option<SecretValue>, Error>;
    fn save(&self, value: &SecretValue) -> Result<(), Error>;
}
/// A single envelope keeps names, values and expiries consistent across item updates.
/// Identified backends require coordinated(), holding a shared process mutex and
/// interprocess flock for the whole operation. injected() is isolated mocks only;
/// identified OS drivers fail closed if used through it. No default/fallback.
pub struct KeyringStore<D> {
    driver: Mutex<D>,
    #[cfg(unix)]
    domain: Option<crate::KeyringLockDomain>,
}
impl<D: KeyringDriver> KeyringStore<D> {
    /// Isolated synthetic/in-memory injection only. Identified drivers cannot
    /// perform load/save through this constructor; use coordinated instead.
    pub fn injected(driver: D) -> Self {
        Self {
            driver: Mutex::new(driver),
            #[cfg(unix)]
            domain: None,
        }
    }
    /// Only for an explicitly acknowledged, deployment-qualified exclusive domain.
    /// Identity mismatch or an unidentified driver fails before load/save.
    #[cfg(unix)]
    pub fn coordinated(driver: D, domain: crate::KeyringLockDomain) -> Result<Self, Error> {
        if driver.identity().as_ref() != Some(domain.identity()) {
            return Err(Error::Backend);
        }
        Ok(Self {
            driver: Mutex::new(driver),
            domain: Some(domain),
        })
    }
    fn transaction<T>(
        &self,
        f: impl FnOnce(&mut codec::Entries) -> Result<T, Error>,
        write: bool,
    ) -> Result<T, Error> {
        let operation = || {
            let driver = self.driver.lock().map_err(|_| Error::Backend)?;
            #[cfg(unix)]
            match &self.domain {
                Some(domain) if driver.identity().as_ref() == Some(domain.identity()) => {}
                Some(_) => return Err(Error::Backend),
                None if driver.identity().is_some() => return Err(Error::Unavailable),
                None => {}
            }
            #[cfg(not(unix))]
            if driver.identity().is_some() {
                return Err(Error::Unavailable);
            }
            let mut entries = match driver.load().map_err(|_| Error::Backend)? {
                Some(v) => codec::decode(v.expose())?,
                None => codec::Entries::new(),
            };
            let result = f(&mut entries)?;
            if write {
                let bytes: Zeroizing<Vec<u8>> = codec::encode(&entries)?;
                driver
                    .save(&SecretValue::new(bytes.to_vec()))
                    .map_err(|_| Error::Backend)?;
            }
            Ok(result)
        };
        #[cfg(unix)]
        if let Some(domain) = &self.domain {
            return domain.transaction(operation);
        }
        operation()
    }
}
impl<D: KeyringDriver> SecretStore for KeyringStore<D> {
    fn backend(&self) -> &'static str {
        "keychain"
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

/// Compiled only by explicit feature on macOS, and never into unit tests.
/// Construction itself makes no OS calls. First trait operation is an explicit OS effect.
#[cfg(all(target_os = "macos", feature = "os-keyring", not(test)))]
pub struct MacKeyringDriver {
    namespace: SecretName,
}
#[cfg(all(target_os = "macos", feature = "os-keyring", not(test)))]
impl MacKeyringDriver {
    pub fn new(namespace: SecretName) -> Self {
        Self { namespace }
    }
    fn entry(&self) -> Result<keyring::Entry, Error> {
        keyring::Entry::new("m8s-secret-store-v1", self.namespace.as_str())
            .map_err(|_| Error::Backend)
    }
}
#[cfg(all(target_os = "macos", feature = "os-keyring", not(test)))]
impl KeyringDriver for MacKeyringDriver {
    fn identity(&self) -> Option<KeyringIdentity> {
        Some(KeyringIdentity {
            service: SecretName::new("m8s-secret-store-v1").expect("fixed valid service"),
            account: self.namespace.clone(),
        })
    }
    fn load(&self) -> Result<Option<SecretValue>, Error> {
        match self.entry()?.get_secret() {
            Ok(v) => Ok(Some(SecretValue::new(v))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(Error::Backend),
        }
    }
    fn save(&self, value: &SecretValue) -> Result<(), Error> {
        self.entry()?
            .set_secret(value.expose())
            .map_err(|_| Error::Backend)
    }
}
