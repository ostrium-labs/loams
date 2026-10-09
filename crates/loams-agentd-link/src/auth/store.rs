//! Where the refresh token lives: the OS keychain (design 37 section 6.5).
//! The access token is never stored; it lives in memory for its lifetime.

use std::collections::HashMap;
use std::sync::Mutex;

use super::AuthError;

/// A place to keep one secret per key (the key is `instance_id` plus
/// principal, so each environment has its own entry).
pub trait TokenStore: Send + Sync {
    /// The stored secret, if any.
    ///
    /// # Errors
    ///
    /// Backend failures other than "nothing stored".
    fn load(&self, key: &str) -> Result<Option<String>, AuthError>;
    /// Stores or replaces the secret.
    ///
    /// # Errors
    ///
    /// Backend failures, for example no Secret Service on Linux.
    fn save(&self, key: &str, secret: &str) -> Result<(), AuthError>;
    /// Removes the secret; absent is not an error.
    ///
    /// # Errors
    ///
    /// Backend failures other than "nothing stored".
    fn delete(&self, key: &str) -> Result<(), AuthError>;
}

/// The OS keychain: macOS Keychain, Windows Credential Manager, the Linux
/// Secret Service.
#[derive(Debug, Clone)]
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    /// A store under `service` (see [`crate::brand::KEYRING_SERVICE`]).
    #[must_use]
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry, AuthError> {
        keyring::Entry::new(&self.service, key).map_err(|e| AuthError::Store(e.to_string()))
    }
}

impl TokenStore for KeyringStore {
    fn load(&self, key: &str) -> Result<Option<String>, AuthError> {
        match self.entry(key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AuthError::Store(e.to_string())),
        }
    }

    fn save(&self, key: &str, secret: &str) -> Result<(), AuthError> {
        self.entry(key)?
            .set_password(secret)
            .map_err(|e| AuthError::Store(e.to_string()))
    }

    fn delete(&self, key: &str) -> Result<(), AuthError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AuthError::Store(e.to_string())),
        }
    }
}

/// An in-memory store for tests and for hosts with no keychain (sign-in then
/// lasts the session only, and the UI says so).
#[derive(Debug, Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl TokenStore for MemoryStore {
    fn load(&self, key: &str) -> Result<Option<String>, AuthError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned())
    }
    fn save(&self, key: &str, secret: &str) -> Result<(), AuthError> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), AuthError> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trips() {
        let s = MemoryStore::default();
        assert_eq!(s.load("k").unwrap(), None);
        s.save("k", "v").unwrap();
        assert_eq!(s.load("k").unwrap().as_deref(), Some("v"));
        s.delete("k").unwrap();
        s.delete("k").unwrap();
        assert_eq!(s.load("k").unwrap(), None);
    }
}
