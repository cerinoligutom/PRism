//! `KeychainTokenSource` — production [`TokenSource`] backed by the OS keychain.
//!
//! The trait, `AccountHandle`, and `AuthError` live at the canonical path
//! [`crate::github::auth`] (shipped by issue #11). This module re-exports them
//! for convenience and provides the concrete keychain-backed impl.

use std::sync::Mutex;

use secrecy::SecretString;

pub use crate::github::auth::{AccountHandle, AccountId, AuthError, TokenSource};

use crate::auth::keychain::KeychainBackend;

/// Production `TokenSource` backed by the OS keychain. Entries are addressed
/// by `(service, user)` where `service = SERVICE` and `user = account id`.
///
/// `prompt_lock` serialises concurrent `token()` calls across accounts so the
/// OS "Allow keychain access?" prompt fires once per launch instead of once
/// per per-account sync loop. The lock holds only across the backend read
/// (sync, brief) so it is safe to use a `std::sync::Mutex` from async tasks.
pub struct KeychainTokenSource<B: KeychainBackend> {
    backend: B,
    prompt_lock: Mutex<()>,
}

impl<B: KeychainBackend> KeychainTokenSource<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            prompt_lock: Mutex::new(()),
        }
    }

    pub fn store(&self, account: &AccountHandle, token: &str) -> Result<(), AuthError> {
        self.backend
            .set(&account.id, token)
            .map_err(AuthError::from)
    }

    pub fn remove(&self, account: &AccountHandle) -> Result<(), AuthError> {
        self.backend.delete(&account.id).map_err(AuthError::from)
    }
}

impl<B: KeychainBackend> TokenSource for KeychainTokenSource<B> {
    fn token(&self, account: &AccountHandle) -> Result<SecretString, AuthError> {
        // Recover from a poisoned lock: the inner data is `()`, so a panic in
        // a previous holder cannot have left state inconsistent.
        let _guard = self
            .prompt_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match self.backend.get(&account.id) {
            Ok(Some(secret)) => Ok(secret),
            Ok(None) => Err(AuthError::Missing(account.id)),
            Err(e) => Err(AuthError::from(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::keychain::MockKeychain;

    fn handle(id: AccountId) -> AccountHandle {
        AccountHandle {
            id,
            host: "github.com".into(),
            label: "Test".into(),
        }
    }

    #[test]
    fn store_then_fetch_returns_the_token() {
        let backend = MockKeychain::new();
        let src = KeychainTokenSource::new(backend);
        let h = handle(7);

        src.store(&h, "ghp_secret").unwrap();
        let got = src.token(&h).unwrap();

        use secrecy::ExposeSecret;
        assert_eq!(got.expose_secret(), "ghp_secret");
    }

    #[test]
    fn fetch_returns_missing_when_no_token_stored() {
        let backend = MockKeychain::new();
        let src = KeychainTokenSource::new(backend);

        let err = src.token(&handle(42)).expect_err("expected missing");
        assert!(matches!(err, AuthError::Missing(42)));
    }

    #[test]
    fn remove_clears_the_token() {
        let backend = MockKeychain::new();
        let src = KeychainTokenSource::new(backend);
        let h = handle(1);

        src.store(&h, "abc").unwrap();
        src.remove(&h).unwrap();

        let err = src.token(&h).expect_err("expected missing after remove");
        assert!(matches!(err, AuthError::Missing(1)));
    }

    /// Backend that records the peak number of concurrently-active `get`
    /// calls, with a short sleep inside each read to widen the race window.
    struct ConcurrencyProbe {
        active: std::sync::atomic::AtomicUsize,
        peak: std::sync::atomic::AtomicUsize,
        token: String,
    }

    impl ConcurrencyProbe {
        fn new(token: impl Into<String>) -> Self {
            Self {
                active: std::sync::atomic::AtomicUsize::new(0),
                peak: std::sync::atomic::AtomicUsize::new(0),
                token: token.into(),
            }
        }

        fn peak(&self) -> usize {
            self.peak.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl crate::auth::keychain::KeychainBackend for ConcurrencyProbe {
        fn get(
            &self,
            _account: &crate::auth::store::AccountId,
        ) -> Result<Option<SecretString>, crate::auth::keychain::KeychainError> {
            use std::sync::atomic::Ordering::SeqCst;
            let now = self.active.fetch_add(1, SeqCst) + 1;
            // Track the high-water mark of concurrent callers inside `get`.
            self.peak.fetch_max(now, SeqCst);
            // Stall long enough that, absent serialisation, other threads
            // would observe each other inside the critical section.
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.active.fetch_sub(1, SeqCst);
            Ok(Some(SecretString::from(self.token.clone())))
        }

        fn set(
            &self,
            _account: &crate::auth::store::AccountId,
            _token: &str,
        ) -> Result<(), crate::auth::keychain::KeychainError> {
            Ok(())
        }

        fn delete(
            &self,
            _account: &crate::auth::store::AccountId,
        ) -> Result<(), crate::auth::keychain::KeychainError> {
            Ok(())
        }
    }

    #[test]
    fn concurrent_token_calls_serialise_through_prompt_lock() {
        use std::sync::Arc;

        let src = Arc::new(KeychainTokenSource::new(ConcurrencyProbe::new("ghp_xyz")));
        let mut handles = Vec::new();
        for id in 0..8u64 {
            let src = Arc::clone(&src);
            handles.push(std::thread::spawn(move || {
                src.token(&handle(id)).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(
            src.backend.peak(),
            1,
            "prompt_lock should hold concurrent token() calls to one at a time"
        );
    }
}
