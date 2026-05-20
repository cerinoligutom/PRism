//! Thin wrapper over the `keyring` crate so the storage backend can be
//! swapped for an in-memory mock in tests without touching the OS keychain.

use std::sync::Mutex;

use secrecy::SecretString;
use thiserror::Error;

use crate::auth::store::AccountId;

const SERVICE: &str = "com.cerinoligutom.prism";

#[derive(Debug, Error)]
pub enum KeychainError {
    #[error("keychain error: {0}")]
    Other(String),
}

/// Storage backend for PAT material. Implementations either talk to the OS
/// keychain (production) or hold values in memory (tests).
///
/// Returning `Option<SecretString>` from `get` makes "no entry" a normal
/// result, not an error — the call sites that map this onto user-facing
/// "missing token" UX want to discriminate the two cases.
pub trait KeychainBackend: Send + Sync {
    fn get(&self, account: &AccountId) -> Result<Option<SecretString>, KeychainError>;
    fn set(&self, account: &AccountId, token: &str) -> Result<(), KeychainError>;
    fn delete(&self, account: &AccountId) -> Result<(), KeychainError>;
}

/// Production backend backed by the OS keychain via the `keyring` crate.
pub struct OsKeychain;

impl OsKeychain {
    pub fn new() -> Self {
        Self
    }

    fn entry(account: &AccountId) -> Result<keyring::Entry, KeychainError> {
        keyring::Entry::new(SERVICE, &account.to_string())
            .map_err(|e| KeychainError::Other(e.to_string()))
    }
}

impl Default for OsKeychain {
    fn default() -> Self {
        Self::new()
    }
}

impl KeychainBackend for OsKeychain {
    fn get(&self, account: &AccountId) -> Result<Option<SecretString>, KeychainError> {
        let entry = Self::entry(account)?;
        match entry.get_password() {
            Ok(token) => Ok(Some(SecretString::from(token))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(KeychainError::Other(e.to_string())),
        }
    }

    fn set(&self, account: &AccountId, token: &str) -> Result<(), KeychainError> {
        let entry = Self::entry(account)?;
        entry
            .set_password(token)
            .map_err(|e| KeychainError::Other(e.to_string()))
    }

    fn delete(&self, account: &AccountId) -> Result<(), KeychainError> {
        let entry = Self::entry(account)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(KeychainError::Other(e.to_string())),
        }
    }
}

/// In-memory mock backend for unit tests. Avoids any OS keychain access.
#[derive(Default)]
pub struct MockKeychain {
    inner: Mutex<std::collections::HashMap<AccountId, String>>,
}

impl MockKeychain {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeychainBackend for MockKeychain {
    fn get(&self, account: &AccountId) -> Result<Option<SecretString>, KeychainError> {
        let guard = self.inner.lock().expect("mock keychain poisoned");
        Ok(guard.get(account).cloned().map(SecretString::from))
    }

    fn set(&self, account: &AccountId, token: &str) -> Result<(), KeychainError> {
        let mut guard = self.inner.lock().expect("mock keychain poisoned");
        guard.insert(*account, token.to_string());
        Ok(())
    }

    fn delete(&self, account: &AccountId) -> Result<(), KeychainError> {
        let mut guard = self.inner.lock().expect("mock keychain poisoned");
        guard.remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[test]
    fn mock_get_returns_none_for_missing_account() {
        let kc = MockKeychain::new();
        assert!(kc.get(&1).unwrap().is_none());
    }

    #[test]
    fn mock_set_then_get_returns_token() {
        let kc = MockKeychain::new();
        kc.set(&1, "ghp_abc").unwrap();

        let got = kc.get(&1).unwrap().unwrap();
        assert_eq!(got.expose_secret(), "ghp_abc");
    }

    #[test]
    fn mock_set_overwrites_existing_token() {
        let kc = MockKeychain::new();
        kc.set(&1, "first").unwrap();
        kc.set(&1, "second").unwrap();

        let got = kc.get(&1).unwrap().unwrap();
        assert_eq!(got.expose_secret(), "second");
    }

    #[test]
    fn mock_delete_clears_token() {
        let kc = MockKeychain::new();
        kc.set(&1, "abc").unwrap();
        kc.delete(&1).unwrap();
        assert!(kc.get(&1).unwrap().is_none());
    }

    #[test]
    fn mock_delete_missing_is_a_no_op() {
        let kc = MockKeychain::new();
        kc.delete(&99).expect("delete should be idempotent");
    }
}
