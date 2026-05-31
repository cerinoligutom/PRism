//! Thin wrapper over the `keyring` crate so the storage backend can be
//! swapped for an in-memory mock in tests without touching the OS keychain.

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use secrecy::SecretString;
use thiserror::Error;

use crate::auth::store::AccountId;

const SERVICE: &str = "com.zeferinix.prism";

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

/// Typed surface over the subset of `keyring::Error` variants the renderer
/// cares about. The renderer maps `BackendUnavailable` to a platform-specific
/// install hint and `AccessDenied` to a permission-prompt message; `Corrupted`
/// and `Other` fall through to a generic "something went wrong" line.
///
/// `BackendUnavailable.hint` is OS-flavoured at construction time so the
/// frontend doesn't have to ask the renderer process what platform it's on.
#[derive(Debug, Error)]
pub enum KeychainError {
    /// The OS credential store backend isn't reachable (libsecret/gnome-keyring
    /// missing on Linux, macOS keychain locked, Windows Credential Manager
    /// service stopped). Carries a platform-specific install or unlock hint.
    #[error("keychain backend unavailable: {hint}")]
    BackendUnavailable { hint: String },
    /// The backend is up but refused access (user denied the OS prompt, or
    /// the application's keychain ACL has been revoked).
    #[error("keychain access denied")]
    AccessDenied,
    /// The stored credential exists but is unusable: bad UTF-8, invalid
    /// attribute, or multiple matching entries.
    #[error("keychain entry is corrupted")]
    Corrupted,
    /// Fallback for `keyring::Error` variants that don't fit the buckets above
    /// (e.g. `TooLong`).
    #[error("keychain error: {0}")]
    Other(String),
}

/// Hint copy for `BackendUnavailable` keyed by target OS. Centralised so the
/// keychain layer doesn't sprout `cfg`-conditional message strings at every
/// call site.
fn backend_unavailable_hint() -> String {
    if cfg!(target_os = "linux") {
        "Install libsecret/gnome-keyring and ensure it's running.".into()
    } else if cfg!(target_os = "macos") {
        "The macOS keychain is locked or unavailable. Unlock it via Keychain Access and try again."
            .into()
    } else if cfg!(target_os = "windows") {
        "Windows Credential Manager is unavailable. Make sure the Credential Manager service is running.".into()
    } else {
        "The OS credential store is unavailable.".into()
    }
}

/// Maps a `keyring::Error` onto a `KeychainError` arm. `PlatformFailure` is
/// the ambiguous case: on Linux it's almost always libsecret being down, so we
/// treat it as `BackendUnavailable`. On macOS the prompt race is caught by the
/// retry layer; anything that reaches here post-retry is most likely a locked
/// keychain, also `BackendUnavailable`. Other platforms keep the fallback.
impl From<keyring::Error> for KeychainError {
    fn from(err: keyring::Error) -> Self {
        match err {
            keyring::Error::PlatformFailure(cause) => {
                if cfg!(any(target_os = "linux", target_os = "macos")) {
                    KeychainError::BackendUnavailable {
                        hint: backend_unavailable_hint(),
                    }
                } else {
                    KeychainError::Other(cause.to_string())
                }
            }
            keyring::Error::NoStorageAccess(_) => KeychainError::AccessDenied,
            keyring::Error::BadEncoding(_) | keyring::Error::Invalid(_, _) => {
                KeychainError::Corrupted
            }
            keyring::Error::Ambiguous(_) => KeychainError::Corrupted,
            // `NoEntry` is handled at the call site (mapped to `Ok(None)` for
            // `get`, idempotent no-op for `delete`); if it escapes here treat
            // it as `Other` so the surface is at least loud.
            other => KeychainError::Other(other.to_string()),
        }
    }
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
        keyring::Entry::new(SERVICE, &account.to_string()).map_err(KeychainError::from)
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
            Err(e) => Err(KeychainError::from(e)),
        }
    }

    fn set(&self, account: &AccountId, token: &str) -> Result<(), KeychainError> {
        let entry = Self::entry(account)?;
        entry.set_password(token).map_err(KeychainError::from)
    }

    fn delete(&self, account: &AccountId) -> Result<(), KeychainError> {
        let entry = Self::entry(account)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(KeychainError::from(e)),
        }
    }
}

/// In-memory mock backend for unit tests. Avoids any OS keychain access.
///
/// Tests that need to exercise a specific failure mode can install a fault
/// via `inject_failure`; subsequent `get`/`set`/`delete` calls will return
/// the configured `KeychainError` until `clear_failure` is invoked.
#[derive(Default)]
pub struct MockKeychain {
    inner: Mutex<std::collections::HashMap<AccountId, String>>,
    failure: Mutex<Option<MockFailure>>,
}

/// Failure variant for `MockKeychain`. Encodes a `KeychainError` arm without
/// requiring `Clone` on the error type, so each `get/set/delete` invocation
/// reconstructs a fresh value.
#[derive(Debug, Clone)]
pub enum MockFailure {
    BackendUnavailable { hint: String },
    AccessDenied,
    Corrupted,
    Other(String),
}

impl MockFailure {
    fn build(&self) -> KeychainError {
        match self {
            MockFailure::BackendUnavailable { hint } => {
                KeychainError::BackendUnavailable { hint: hint.clone() }
            }
            MockFailure::AccessDenied => KeychainError::AccessDenied,
            MockFailure::Corrupted => KeychainError::Corrupted,
            MockFailure::Other(s) => KeychainError::Other(s.clone()),
        }
    }
}

impl MockKeychain {
    pub fn new() -> Self {
        Self::default()
    }

    /// Force the next `get`/`set`/`delete` call to fail with `failure`.
    /// Persists until `clear_failure` so a test can probe multiple ops.
    pub fn inject_failure(&self, failure: MockFailure) {
        *self.failure.lock().expect("mock keychain poisoned") = Some(failure);
    }

    pub fn clear_failure(&self) {
        *self.failure.lock().expect("mock keychain poisoned") = None;
    }

    fn pending_failure(&self) -> Option<KeychainError> {
        self.failure
            .lock()
            .expect("mock keychain poisoned")
            .as_ref()
            .map(MockFailure::build)
    }
}

impl KeychainBackend for MockKeychain {
    fn get(&self, account: &AccountId) -> Result<Option<SecretString>, KeychainError> {
        if let Some(err) = self.pending_failure() {
            return Err(err);
        }
        let guard = self.inner.lock().expect("mock keychain poisoned");
        Ok(guard.get(account).cloned().map(SecretString::from))
    }

    fn set(&self, account: &AccountId, token: &str) -> Result<(), KeychainError> {
        if let Some(err) = self.pending_failure() {
            return Err(err);
        }
        let mut guard = self.inner.lock().expect("mock keychain poisoned");
        guard.insert(*account, token.to_string());
        Ok(())
    }

    fn delete(&self, account: &AccountId) -> Result<(), KeychainError> {
        if let Some(err) = self.pending_failure() {
            return Err(err);
        }
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

    // ────── keyring::Error -> KeychainError variant mapping ──────

    #[test]
    fn platform_failure_maps_to_backend_unavailable_on_linux_and_macos() {
        // PlatformFailure post-retry on Linux/macOS is almost always
        // "backend not running / keychain locked" rather than the
        // first-launch prompt race. Render-friendly arm so the frontend
        // can surface install/unlock copy.
        let err = KeychainError::from(keyring::Error::PlatformFailure(Box::new(
            std::io::Error::other("backend down"),
        )));
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            match err {
                KeychainError::BackendUnavailable { hint } => {
                    assert!(!hint.is_empty(), "hint should carry platform-specific copy")
                }
                other => panic!("expected BackendUnavailable, got {other:?}"),
            }
        } else {
            assert!(matches!(err, KeychainError::Other(_)));
        }
    }

    #[test]
    fn no_storage_access_maps_to_access_denied() {
        let err = KeychainError::from(keyring::Error::NoStorageAccess(Box::new(
            std::io::Error::other("denied"),
        )));
        assert!(matches!(err, KeychainError::AccessDenied));
    }

    #[test]
    fn bad_encoding_maps_to_corrupted() {
        let err = KeychainError::from(keyring::Error::BadEncoding(vec![0xff, 0xfe]));
        assert!(matches!(err, KeychainError::Corrupted));
    }

    #[test]
    fn invalid_attribute_maps_to_corrupted() {
        let err = KeychainError::from(keyring::Error::Invalid("service".into(), "empty".into()));
        assert!(matches!(err, KeychainError::Corrupted));
    }

    #[test]
    fn ambiguous_maps_to_corrupted() {
        let err = KeychainError::from(keyring::Error::Ambiguous(Vec::new()));
        assert!(matches!(err, KeychainError::Corrupted));
    }

    #[test]
    fn too_long_falls_back_to_other() {
        let err = KeychainError::from(keyring::Error::TooLong("service".into(), 32));
        assert!(matches!(err, KeychainError::Other(_)));
    }

    #[test]
    fn backend_unavailable_hint_is_linux_specific_on_linux() {
        // Locks in the renderer-facing copy the frontend keys on: any change
        // here should be reflected in `formatAuthError`.
        if cfg!(target_os = "linux") {
            assert!(backend_unavailable_hint().contains("libsecret"));
        }
    }

    // ────── MockKeychain failure injection ──────

    #[test]
    fn mock_get_returns_injected_failure() {
        let kc = MockKeychain::new();
        kc.inject_failure(MockFailure::BackendUnavailable {
            hint: "install libsecret".into(),
        });
        let err = kc.get(&1).expect_err("expected injected failure");
        match err {
            KeychainError::BackendUnavailable { hint } => assert_eq!(hint, "install libsecret"),
            other => panic!("expected BackendUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn mock_set_returns_injected_failure() {
        let kc = MockKeychain::new();
        kc.inject_failure(MockFailure::AccessDenied);
        let err = kc.set(&1, "t").expect_err("expected injected failure");
        assert!(matches!(err, KeychainError::AccessDenied));
    }

    #[test]
    fn mock_delete_returns_injected_failure() {
        let kc = MockKeychain::new();
        kc.inject_failure(MockFailure::Corrupted);
        let err = kc.delete(&1).expect_err("expected injected failure");
        assert!(matches!(err, KeychainError::Corrupted));
    }

    #[test]
    fn mock_clear_failure_restores_normal_operation() {
        let kc = MockKeychain::new();
        kc.inject_failure(MockFailure::AccessDenied);
        assert!(kc.get(&1).is_err());
        kc.clear_failure();
        // Back to normal: missing entry surfaces as Ok(None).
        assert!(kc.get(&1).unwrap().is_none());
    }
}
