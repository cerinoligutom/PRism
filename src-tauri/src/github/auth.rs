//! GitHub authentication abstractions.
//!
//! The [`TokenSource`] trait decouples the HTTP client from the concrete PAT
//! storage layer. Issue #10 plugs `KeychainTokenSource` in behind this trait;
//! tests use [`StaticTokenSource`].

use secrecy::SecretString;
use std::fmt;
use thiserror::Error;

/// Stable identifier for an account row.
///
/// We use an opaque `u64` rather than a database row id so that the GitHub
/// layer doesn't take a hard dep on the storage layer's primary key type.
pub type AccountId = u64;

/// Per-account context passed to a [`TokenSource`].
///
/// Cloneable so it can be carried alongside the client; contains no secret
/// material (the PAT is fetched on demand from the source).
#[derive(Clone, Debug)]
pub struct AccountHandle {
    pub id: AccountId,
    /// `"github.com"` or an Enterprise host such as `"github.acme.io"`.
    pub host: String,
    /// User-visible label (e.g. login name).
    pub label: String,
}

impl AccountHandle {
    pub fn new(id: AccountId, host: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id,
            host: host.into(),
            label: label.into(),
        }
    }
}

/// Returns the PAT for the given account.
///
/// Implementations should fetch fresh on every call. The token must never be
/// cached on the client struct, written to logs, or surfaced in error messages.
pub trait TokenSource: Send + Sync {
    fn token(&self, account: &AccountHandle) -> Result<SecretString, AuthError>;
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("no token configured for account {0}")]
    Missing(AccountId),
    /// OS keychain failure. Carries the typed `KeychainError` so the renderer
    /// can route `BackendUnavailable` to a platform-specific install hint and
    /// `AccessDenied` to a permission-denied message. Use `Display` (or the
    /// inner variant directly) for user-facing copy; do not `to_string` the
    /// outer `AuthError` if you care about the specific arm.
    #[error("keychain access failed: {0}")]
    Keychain(#[from] crate::auth::keychain::KeychainError),
    #[error("token is empty for account {0}")]
    Empty(AccountId),
}

/// In-process token source for tests and the legacy demo path.
///
/// Holds a single secret applied to every account.
pub struct StaticTokenSource {
    token: SecretString,
}

impl StaticTokenSource {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: SecretString::from(token.into()),
        }
    }
}

impl fmt::Debug for StaticTokenSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticTokenSource").finish_non_exhaustive()
    }
}

impl TokenSource for StaticTokenSource {
    fn token(&self, _account: &AccountHandle) -> Result<SecretString, AuthError> {
        Ok(self.token.clone())
    }
}
