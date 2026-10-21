use crate::{Error, SecretInfo, SecretName, SecretStore, SecretValue, codec};
use std::sync::Mutex;
use zeroize::Zeroizing;

/// Injected byte driver for ONE namespaced vault item, not arbitrary OS accounts.
/// Drivers must not log errors/values. Errors are deliberately opaque.
/// Atomic replacement is required; external writers must be serialized by the caller.
pub trait KeyringDriver: Send + Sync {
    fn load(&self) -> Result<Option<SecretValue>, Error>;
    fn save(&self, value: &SecretValue) -> Result<(), Error>;
}
/// A single envelope keeps names, values and expiries consistent across item updates.
/// Mutex serializes this object's read-modify-write; independent processes/objects require
/// external coordination. No implicit default constructor and no file fallback.
pub struct KeyringStore<D> {
    driver: Mutex<D>,
}
impl<D: KeyringDriver> KeyringStore<D> {
    pub fn injected(driver: D) -> Self {
        Self {
            driver: Mutex::new(driver),
        }
    }
    fn transaction<T>(
        &self,
        f: impl FnOnce(&mut codec::Entries) -> Result<T, Error>,
        write: bool,
    ) -> Result<T, Error> {
        let driver = self.driver.lock().map_err(|_| Error::Backend)?;
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
