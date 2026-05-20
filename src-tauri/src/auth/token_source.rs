//! `KeychainTokenSource` — production [`TokenSource`] backed by the OS keychain.
//!
//! The trait, `AccountHandle`, and `AuthError` live at the canonical path
//! [`crate::github::auth`] (shipped by issue #11). This module re-exports them
//! for convenience and provides the concrete keychain-backed impl.

use secrecy::SecretString;

pub use crate::github::auth::{AccountHandle, AccountId, AuthError, TokenSource};

use crate::auth::keychain::KeychainBackend;

/// Production `TokenSource` backed by the OS keychain. Entries are addressed
/// by `(service, user)` where `service = SERVICE` and `user = account id`.
pub struct KeychainTokenSource<B: KeychainBackend> {
    backend: B,
}

impl<B: KeychainBackend> KeychainTokenSource<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn store(&self, account: &AccountHandle, token: &str) -> Result<(), AuthError> {
        self.backend
            .set(&account.id, token)
            .map_err(|e| AuthError::Keychain(e.to_string()))
    }

    pub fn remove(&self, account: &AccountHandle) -> Result<(), AuthError> {
        self.backend
            .delete(&account.id)
            .map_err(|e| AuthError::Keychain(e.to_string()))
    }
}

impl<B: KeychainBackend> TokenSource for KeychainTokenSource<B> {
    fn token(&self, account: &AccountHandle) -> Result<SecretString, AuthError> {
        match self.backend.get(&account.id) {
            Ok(Some(secret)) => Ok(secret),
            Ok(None) => Err(AuthError::Missing(account.id)),
            Err(e) => Err(AuthError::Keychain(e.to_string())),
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
}
