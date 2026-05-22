//! Thin wrapper over the `keyring` crate so the storage backend can be
//! swapped for an in-memory mock in tests without touching the OS keychain.

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use secrecy::SecretString;
use thiserror::Error;

use crate::auth::store::AccountId;

const SERVICE: &str = "com.cerinoligutom.prism";

/// Number of attempts for keychain reads that hit transient `PlatformFailure`.
///
/// On macOS the first read after a fresh code signature races the "Allow
/// keychain access?" prompt. The user typically clicks within a second; three
/// attempts at 500ms apart gives them that window without giving up.
const RETRY_ATTEMPTS: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(500);

/// Run `fetch` up to `attempts` times, retrying only on
/// [`keyring::Error::PlatformFailure`] (the macOS prompt race). Other variants
/// return immediately so we don't paper over genuine failures.
fn retry_on_platform_failure<T>(
    attempts: usize,
    delay: Duration,
    mut fetch: impl FnMut() -> Result<T, keyring::Error>,
) -> Result<T, keyring::Error> {
    debug_assert!(attempts >= 1);
    let mut last = None;
    for attempt in 0..attempts {
        match fetch() {
            Ok(v) => return Ok(v),
            Err(keyring::Error::PlatformFailure(e)) => {
                last = Some(keyring::Error::PlatformFailure(e));
                if attempt + 1 < attempts {
                    thread::sleep(delay);
                }
            }
            Err(other) => return Err(other),
        }
    }
    Err(last.expect("retry loop ran at least once"))
}

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
        let result =
            retry_on_platform_failure(RETRY_ATTEMPTS, RETRY_DELAY, || entry.get_password());
        match result {
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

    fn platform_failure() -> keyring::Error {
        keyring::Error::PlatformFailure(Box::new(std::io::Error::other("simulated prompt race")))
    }

    #[test]
    fn retry_succeeds_on_third_attempt_after_two_platform_failures() {
        let mut calls = 0;
        let result = retry_on_platform_failure(3, Duration::from_millis(1), || {
            calls += 1;
            if calls < 3 {
                Err(platform_failure())
            } else {
                Ok("ghp_ok".to_string())
            }
        });
        assert_eq!(result.expect("third attempt should succeed"), "ghp_ok");
        assert_eq!(calls, 3, "should have made exactly three attempts");
    }

    #[test]
    fn retry_returns_platform_failure_after_exhausting_attempts() {
        let mut calls = 0;
        let result = retry_on_platform_failure::<String>(3, Duration::from_millis(1), || {
            calls += 1;
            Err(platform_failure())
        });
        assert!(matches!(result, Err(keyring::Error::PlatformFailure(_))));
        assert_eq!(calls, 3, "should have exhausted all attempts");
    }

    #[test]
    fn retry_does_not_retry_no_entry() {
        let mut calls = 0;
        let result = retry_on_platform_failure::<String>(3, Duration::from_millis(1), || {
            calls += 1;
            Err(keyring::Error::NoEntry)
        });
        assert!(matches!(result, Err(keyring::Error::NoEntry)));
        assert_eq!(calls, 1, "NoEntry should not be retried");
    }

    #[test]
    fn retry_does_not_retry_no_storage_access() {
        let mut calls = 0;
        let result = retry_on_platform_failure::<String>(3, Duration::from_millis(1), || {
            calls += 1;
            Err(keyring::Error::NoStorageAccess(Box::new(
                std::io::Error::other("denied"),
            )))
        });
        assert!(matches!(result, Err(keyring::Error::NoStorageAccess(_))));
        assert_eq!(calls, 1, "NoStorageAccess should not be retried");
    }
}
