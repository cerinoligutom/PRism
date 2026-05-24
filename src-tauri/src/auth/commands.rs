//! Tauri commands exposed to the frontend.
//!
//! Token material never crosses this boundary — `add_account` receives the
//! PAT, validates it, writes it to the keychain, and returns only the
//! sanitised `Account` metadata. `list_accounts` and `remove_account` never
//! see tokens at all.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use thiserror::Error;

/// Hot-add / hot-remove / hot-refresh hook called by the auth commands so the
/// sync worker (or anything else that cares about the account roster) can spin
/// up / tear down per-account resources or wake a parked loop without waiting
/// for the next app restart.
///
/// The trait is defined here in `auth` so this module has no compile-time
/// dependency on `sync` — the implementation lives in `sync::worker`.
pub trait AccountChangeListener: Send + Sync {
    fn on_added(&self, account: &Account);
    fn on_removed(&self, account_id: AccountId);
    /// Fired after the keychain entry for `account_id` has been rewritten with
    /// a fresh PAT. Implementations nudge the per-account loop so a
    /// `SyncPhase::Unauthorized` slot exits its suspend branch on the next
    /// cycle without waiting for the next interval tick.
    ///
    /// Default impl is a no-op so existing listeners (and the `NoopAccountListener`
    /// used in tests / headless dev) don't need to opt in.
    fn on_token_updated(&self, _account_id: AccountId) {}
}

/// Default listener used when no live worker is wired (tests, headless dev).
pub struct NoopAccountListener;

impl AccountChangeListener for NoopAccountListener {
    fn on_added(&self, _account: &Account) {}
    fn on_removed(&self, _account_id: AccountId) {}
}

use crate::auth::keychain::OsKeychain;
use crate::auth::store::{
    import_legacy_json_if_present, Account, AccountHandle, AccountId, AccountStore, SqlAccountStore,
};
use crate::auth::token_source::KeychainTokenSource;
use crate::auth::validation::{
    check_permissions, validate_token, PermissionChecks, ValidationError,
};
use crate::db::DbHandle;

/// Emitted whenever any sync-path call returns 401, so the frontend can show
/// the re-auth banner. Wired through `emit_reauth_required` so callers
/// outside this module can fire it without depending on Tauri internals.
pub const REAUTH_EVENT: &str = "auth://reauth-required";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReauthRequired {
    pub account_id: AccountId,
    pub label: String,
}

/// User-facing error shape for the `auth::*` commands. Internal errors are
/// folded into a single opaque variant so internal details don't surface to
/// the frontend (CLAUDE.md security rule).
#[derive(Debug, Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthCommandError {
    #[error("token rejected by GitHub. Check that it hasn't expired or been revoked.")]
    Unauthorized,
    #[error("token doesn't have the required permissions.")]
    Forbidden,
    #[error("could not reach {host}.")]
    Network { host: String },
    #[error("account not found.")]
    NotFound,
    /// `update_token` refused the PAT because it authenticated as a different
    /// GitHub login than the one stored against `account_id`. Surfaced as a
    /// distinct variant so the renderer can prompt the user to Remove + Add
    /// instead of silently switching the identity. Carries the expected and
    /// actual logins for the inline error message; neither is a secret.
    #[error("token authenticated as {actual_login} but this account is {expected_login}.")]
    LoginMismatch {
        expected_login: String,
        actual_login: String,
    },
    /// `add_account` refused the PAT because the resolved `(host, login)`
    /// already maps to a connected account. The `accounts` table's
    /// `UNIQUE (host, login)` constraint backs this: login uniqueness per
    /// host is intentional (ADR 0005, ADR 0016). Carries the existing label
    /// so the renderer can name the account the user is colliding with.
    /// None of the three fields are secrets.
    #[error("an account for {login} on {host} is already connected ({existing_label}).")]
    DuplicateAccount {
        existing_label: String,
        login: String,
        host: String,
    },
    /// The OS credential store backend isn't reachable (libsecret missing on
    /// Linux, macOS keychain locked, Windows Credential Manager service
    /// stopped). Carries a platform-specific hint string assembled by the
    /// keychain layer so the renderer doesn't need to know the host OS.
    #[error("keychain backend unavailable: {hint}")]
    KeychainUnavailable { hint: String },
    /// The keychain refused access (user denied the OS prompt, or the
    /// application's keychain ACL has been revoked).
    #[error("keychain access denied.")]
    KeychainAccessDenied,
    #[error("an unexpected error occurred. Try again, or check the application logs.")]
    Internal,
}

impl From<ValidationError> for AuthCommandError {
    fn from(value: ValidationError) -> Self {
        match value {
            ValidationError::Unauthorized => AuthCommandError::Unauthorized,
            ValidationError::Forbidden => AuthCommandError::Forbidden,
            ValidationError::Network { host, .. } => AuthCommandError::Network { host },
            ValidationError::Unexpected(_) => AuthCommandError::Internal,
        }
    }
}

/// Threads typed `KeychainError` arms through `AuthError::Keychain` onto the
/// renderer-facing `AuthCommandError`. `Missing` and `Empty` collapse onto
/// `Unauthorized` because the rendered copy ("token rejected; check it hasn't
/// expired") matches the reauth path the user is already on.
impl From<crate::github::auth::AuthError> for AuthCommandError {
    fn from(value: crate::github::auth::AuthError) -> Self {
        use crate::auth::keychain::KeychainError;
        use crate::github::auth::AuthError;
        match value {
            AuthError::Missing(_) | AuthError::Empty(_) => AuthCommandError::Unauthorized,
            AuthError::Keychain(KeychainError::BackendUnavailable { hint }) => {
                AuthCommandError::KeychainUnavailable { hint }
            }
            AuthError::Keychain(KeychainError::AccessDenied) => {
                AuthCommandError::KeychainAccessDenied
            }
            // `Corrupted` and `Other` carry no actionable user-facing copy
            // beyond "something went wrong"; fold to the opaque internal arm.
            AuthError::Keychain(_) => AuthCommandError::Internal,
        }
    }
}

/// Shared handle the Tauri runtime injects into every command. The store
/// and keychain backend are wrapped in `Arc` so testing can swap them
/// without touching the production builder in `lib.rs`.
pub struct AuthState {
    pub store: Arc<dyn AccountStore>,
    pub token_source: Arc<KeychainTokenSource<OsKeychain>>,
    /// Set once during `lib.rs::setup` after the sync worker is constructed.
    /// Reads return `None` until that wiring happens, which is fine — the
    /// commands fall back to `NoopAccountListener` semantics in that window.
    listener: OnceLock<Arc<dyn AccountChangeListener>>,
}

impl AuthState {
    pub fn new(db: DbHandle, data_dir: &Path) -> Result<Self, String> {
        let store = SqlAccountStore::new(db);
        // One-shot import of any pre-#62 accounts.json the user may still
        // have on disk. Best-effort: errors log but don't block startup.
        if let Err(e) = import_legacy_json_if_present(&store, data_dir) {
            tracing::warn!(err = %e, "legacy accounts.json import failed");
        }
        Ok(Self {
            store: Arc::new(store),
            token_source: Arc::new(KeychainTokenSource::new(OsKeychain::new())),
            listener: OnceLock::new(),
        })
    }

    /// Wire the account-change listener (e.g. the sync worker). Called once
    /// during app setup. Subsequent calls are ignored.
    pub fn set_listener(&self, listener: Arc<dyn AccountChangeListener>) {
        let _ = self.listener.set(listener);
    }

    fn notify_added(&self, account: &Account) {
        if let Some(l) = self.listener.get() {
            l.on_added(account);
        }
    }

    fn notify_removed(&self, account_id: AccountId) {
        if let Some(l) = self.listener.get() {
            l.on_removed(account_id);
        }
    }

    fn notify_token_updated(&self, account_id: AccountId) {
        if let Some(l) = self.listener.get() {
            l.on_token_updated(account_id);
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AddAccountInput {
    pub label: String,
    pub host: String,
    pub token: String,
}

/// Validates the PAT, then commits both metadata + keychain entry atomically
/// from the user's perspective — if either step fails the other is rolled
/// back so we never persist half an account.
#[tauri::command]
pub async fn add_account(
    state: State<'_, AuthState>,
    input: AddAccountInput,
) -> Result<Account, AuthCommandError> {
    if input.token.trim().is_empty() {
        return Err(AuthCommandError::Unauthorized);
    }
    let host = normalise_host(&input.host);
    let secret = SecretString::from(input.token);
    let validated = validate_token(&host, &secret).await?;

    // Catch the `UNIQUE (host, login)` collision early so the error surface
    // is specific (named existing account) rather than a generic "something
    // went wrong". Pre-check avoids interpreting rusqlite's extended error
    // codes and keeps the keychain entirely untouched on the rejection.
    if let Some(existing) = find_duplicate(state.store.as_ref(), &host, &validated.login)? {
        return Err(AuthCommandError::DuplicateAccount {
            existing_label: existing.label,
            login: validated.login,
            host,
        });
    }

    let id = state
        .store
        .next_id()
        .map_err(|_| AuthCommandError::Internal)?;
    let account = Account {
        id,
        label: input.label.trim().to_string(),
        host,
        login: validated.login,
        scopes: validated.scopes,
        expires_at: validated.expires_at,
        // Avatar resolves at read time via `list_accounts`. A freshly added
        // account has no `users` row until the first sync cycle populates it.
        avatar_url: None,
    };

    let handle = account.handle();
    // `?` flows the typed `AuthError` through `From<AuthError>` so a
    // `BackendUnavailable` arm surfaces as a platform-specific hint rather
    // than the generic `Internal` arm.
    state.token_source.store(&handle, secret_as_str(&secret))?;

    if let Err(e) = state.store.upsert(account.clone()) {
        // Roll the keychain write back so the account doesn't half-exist.
        let _ = state.token_source.remove(&handle);
        return Err(internal(&format!("persist account metadata: {e}")));
    }

    // Hot-add the new account to the sync worker so it starts polling without
    // waiting for an app restart. Best-effort; the listener swallows failures.
    state.notify_added(&account);

    Ok(account)
}

#[tauri::command]
pub fn list_accounts(state: State<'_, AuthState>) -> Result<Vec<Account>, AuthCommandError> {
    state.store.list().map_err(|_| AuthCommandError::Internal)
}

#[tauri::command]
pub fn remove_account(state: State<'_, AuthState>, id: AccountId) -> Result<(), AuthCommandError> {
    let accounts = state.store.list().map_err(|_| AuthCommandError::Internal)?;
    let account = accounts
        .into_iter()
        .find(|a| a.id == id)
        .ok_or(AuthCommandError::NotFound)?;

    let handle = account.handle();
    // Remove the keychain entry first; if it fails the metadata stays so the
    // user can retry. The reverse order would leave a token with no owner.
    // `?` flows typed `AuthError` through `From<AuthError>` for the same
    // hint-aware surface `add_account` returns.
    state.token_source.remove(&handle)?;

    state
        .store
        .remove(id)
        .map_err(|_| AuthCommandError::Internal)?;

    // Stop the sync worker's per-account loop for this id and clear its slot
    // from the state map. Best-effort; the listener swallows failures.
    state.notify_removed(id);

    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct UpdateTokenInput {
    pub account_id: AccountId,
    pub token: String,
}

/// Swap the PAT for an existing account. The new token is validated against
/// the account's stored host and rejected unless it authenticates as the same
/// login: re-authing across identities is Remove + Add territory (per the
/// issue + ADR 0016 host immutability).
///
/// On success the keychain entry under `account_id` is rewritten and the sync
/// worker is nudged so a parked `SyncPhase::Unauthorized` loop exits its
/// suspend branch on the next cycle. Token material never crosses back to the
/// renderer: the command returns `()` on success and a sanitised error on
/// failure (CLAUDE.md security rule).
#[tauri::command]
pub async fn update_token(
    state: State<'_, AuthState>,
    input: UpdateTokenInput,
) -> Result<(), AuthCommandError> {
    if input.token.trim().is_empty() {
        return Err(AuthCommandError::Unauthorized);
    }

    let account = find_account(&state, input.account_id)?;
    let secret = SecretString::from(input.token);
    let validated = validate_token(&account.host, &secret).await?;

    apply_token_swap(
        state.store.as_ref(),
        |handle, token| {
            state
                .token_source
                .store(handle, token)
                .map_err(AuthCommandError::from)
        },
        &account,
        &validated,
        &secret,
    )?;

    // Nudge the worker so the parked loop runs a cycle immediately instead of
    // waiting for the next interval tick. Best-effort; the listener swallows
    // failures (e.g. running outside the desktop shell in tests).
    state.notify_token_updated(input.account_id);

    Ok(())
}

/// Post-validation persistence step shared between `update_token` and its
/// unit tests. Refuses the swap on a login mismatch (without touching the
/// keychain), writes the token via `keychain_write`, and refreshes the
/// account's `scopes` / `expires_at` from the validation response.
///
/// The keychain operation flows through a closure so the test path can
/// substitute an in-memory mock without smuggling an Arc into `AuthState`.
/// The closure signature matches `KeychainTokenSource::store` so production
/// callers can pass it directly.
fn apply_token_swap<F>(
    store: &dyn AccountStore,
    keychain_write: F,
    account: &Account,
    validated: &crate::auth::validation::ValidatedToken,
    secret: &SecretString,
) -> Result<(), AuthCommandError>
where
    F: FnOnce(&AccountHandle, &str) -> Result<(), AuthCommandError>,
{
    if validated.login != account.login {
        // Don't write anything. The keychain entry for `account_id` is
        // untouched; the next sync cycle keeps using the existing token.
        return Err(AuthCommandError::LoginMismatch {
            expected_login: account.login.clone(),
            actual_login: validated.login.clone(),
        });
    }

    let handle = account.handle();
    keychain_write(&handle, secret_as_str(secret))?;

    // Refresh metadata (scopes, expiry) from the validation response so the
    // Settings card reflects the new token without waiting for the next list.
    // Identity fields (id, label, host, login) are preserved.
    let refreshed = Account {
        scopes: validated.scopes.clone(),
        expires_at: validated.expires_at.clone(),
        ..account.clone()
    };
    if let Err(e) = store.upsert(refreshed) {
        // Metadata refresh failure isn't fatal: the new token is already in
        // the keychain and will work. Log and continue so the user's re-auth
        // doesn't appear to fail because of a side-effect.
        tracing::warn!(err = %e, "update_token: refresh metadata failed");
    }

    Ok(())
}

fn find_account(state: &AuthState, account_id: AccountId) -> Result<Account, AuthCommandError> {
    let accounts = state.store.list().map_err(|_| AuthCommandError::Internal)?;
    accounts
        .into_iter()
        .find(|a| a.id == account_id)
        .ok_or(AuthCommandError::NotFound)
}

/// Returns the existing account row that already owns `(host, login)`, if
/// any. Lifted out of `add_account` so the test path can exercise the lookup
/// without standing up the full Tauri command. Host comparison is
/// case-insensitive on the stored side via `normalise_host`; login matches
/// verbatim because GitHub logins are case-normalised by the server.
fn find_duplicate(
    store: &dyn AccountStore,
    host: &str,
    login: &str,
) -> Result<Option<Account>, AuthCommandError> {
    let accounts = store.list().map_err(|_| AuthCommandError::Internal)?;
    Ok(accounts
        .into_iter()
        .find(|a| a.host == host && a.login == login))
}

#[derive(Debug, Deserialize)]
pub struct ValidateTokenInput {
    pub host: String,
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct ValidateTokenResult {
    pub login: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<String>,
    pub permissions: PermissionChecks,
}

/// Standalone validation — used from the onboarding flow to surface token
/// status and per-permission grant state before the user commits. Does
/// not store anything in the keychain.
#[tauri::command]
pub async fn validate_token_cmd(
    input: ValidateTokenInput,
) -> Result<ValidateTokenResult, AuthCommandError> {
    let host = normalise_host(&input.host);
    let secret = SecretString::from(input.token);
    let validated = validate_token(&host, &secret).await?;
    let permissions = check_permissions(&validated.scopes);
    Ok(ValidateTokenResult {
        login: validated.login,
        scopes: validated.scopes,
        expires_at: validated.expires_at,
        permissions,
    })
}

/// Fire-and-forget reauth event the sync worker can call after a 401.
pub fn emit_reauth_required<R: Runtime>(app: &AppHandle<R>, account: &Account) {
    let payload = ReauthRequired {
        account_id: account.id,
        label: account.label.clone(),
    };
    if let Err(e) = app.emit(REAUTH_EVENT, payload) {
        tracing::warn!(event = REAUTH_EVENT, err = %e, "failed to emit reauth event");
    }
}

fn normalise_host(host: &str) -> String {
    let trimmed = host
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let trimmed = trimmed.trim_end_matches('/');
    if trimmed.is_empty() {
        "github.com".into()
    } else {
        trimmed.to_lowercase()
    }
}

fn secret_as_str(secret: &SecretString) -> &str {
    use secrecy::ExposeSecret;
    secret.expose_secret()
}

fn internal(message: &str) -> AuthCommandError {
    tracing::error!(message, "auth internal error");
    AuthCommandError::Internal
}

/// Wires `AuthState` into the running Tauri app. Called from `lib.rs::setup`
/// after the SQLite cache is open so we can share its connection handle.
pub fn install<R: Runtime>(app: &AppHandle<R>, db: DbHandle) -> Result<(), String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolve app data dir: {e}"))?;
    std::fs::create_dir_all(&data_dir).map_err(|e| format!("create app data dir: {e}"))?;
    let state = AuthState::new(db, &data_dir)?;
    app.manage(state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::keychain::{KeychainBackend, MockKeychain};
    use crate::auth::store::{SqlAccountStore, StoreError};
    use crate::auth::validation::ValidatedToken;
    use crate::db::migrate;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    #[test]
    fn host_normalisation_strips_scheme_and_trailing_slash() {
        assert_eq!(normalise_host("https://github.com/"), "github.com");
        assert_eq!(
            normalise_host("http://GitHub.Acme.Corp"),
            "github.acme.corp"
        );
        assert_eq!(normalise_host("  github.com  "), "github.com");
    }

    #[test]
    fn host_normalisation_falls_back_to_github_com_when_empty() {
        assert_eq!(normalise_host(""), "github.com");
        assert_eq!(normalise_host("   "), "github.com");
    }

    // ────── update_token: post-validation persistence ──────

    fn fresh_store() -> Arc<SqlAccountStore> {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        migrate::run(&mut conn).expect("run migrations");
        Arc::new(SqlAccountStore::new(Arc::new(Mutex::new(conn))))
    }

    fn seed(store: &dyn AccountStore, id: AccountId, login: &str) -> Account {
        let account = Account {
            id,
            label: "Test".into(),
            host: "github.com".into(),
            login: login.into(),
            scopes: vec!["repo".into()],
            expires_at: Some("2026-01-01T00:00:00Z".into()),
            avatar_url: None,
        };
        store.upsert(account.clone()).expect("seed account");
        account
    }

    fn validated(login: &str, scopes: &[&str], expires_at: Option<&str>) -> ValidatedToken {
        ValidatedToken {
            login: login.into(),
            scopes: scopes.iter().map(|s| (*s).to_string()).collect(),
            expires_at: expires_at.map(|s| s.into()),
        }
    }

    #[test]
    fn apply_swap_writes_new_token_and_refreshes_metadata_on_login_match() {
        // Validate-OK + login-match path: the keychain entry under
        // `account_id` is rewritten and scopes / expiry refresh from the
        // validation response. This is the wiremock "validate-OK" outcome
        // routed into the persistence helper without re-running the network
        // path (validation.rs already wiremocks GET /user end-to-end).
        let store = fresh_store();
        let account = seed(store.as_ref(), 1, "ada");
        let keychain = MockKeychain::new();
        keychain.set(&account.id, "old-token").unwrap();

        let new = SecretString::from("new-token".to_string());
        let result = apply_token_swap(
            store.as_ref(),
            |handle, token| {
                keychain
                    .set(&handle.id, token)
                    .map_err(|_| AuthCommandError::Internal)
            },
            &account,
            &validated("ada", &["repo", "read:org"], Some("2027-06-01T00:00:00Z")),
            &new,
        );
        assert!(result.is_ok());

        use secrecy::ExposeSecret;
        let stored = keychain.get(&account.id).unwrap().expect("token persists");
        assert_eq!(stored.expose_secret(), "new-token");

        let refreshed = store
            .list()
            .unwrap()
            .into_iter()
            .find(|a| a.id == account.id)
            .unwrap();
        assert_eq!(
            refreshed.scopes,
            vec!["repo".to_string(), "read:org".into()]
        );
        assert_eq!(
            refreshed.expires_at.as_deref(),
            Some("2027-06-01T00:00:00Z")
        );
        // Identity fields are preserved across the swap.
        assert_eq!(refreshed.login, "ada");
        assert_eq!(refreshed.host, "github.com");
        assert_eq!(refreshed.label, "Test");
    }

    #[test]
    fn apply_swap_refuses_login_mismatch_and_leaves_keychain_untouched() {
        // Login-mismatch path: the PAT belongs to a different identity. The
        // existing keychain entry is preserved (the next sync cycle keeps
        // using whatever was there) and no metadata is written.
        let store = fresh_store();
        let account = seed(store.as_ref(), 1, "ada");
        let keychain = MockKeychain::new();
        keychain.set(&account.id, "old-token").unwrap();

        let new = SecretString::from("intruder-token".to_string());
        let result = apply_token_swap(
            store.as_ref(),
            |handle, token| {
                keychain
                    .set(&handle.id, token)
                    .map_err(|_| AuthCommandError::Internal)
            },
            &account,
            &validated("grace", &["repo"], None),
            &new,
        );

        match result {
            Err(AuthCommandError::LoginMismatch {
                expected_login,
                actual_login,
            }) => {
                assert_eq!(expected_login, "ada");
                assert_eq!(actual_login, "grace");
            }
            other => panic!("expected LoginMismatch, got {other:?}"),
        }

        // Keychain entry is untouched.
        use secrecy::ExposeSecret;
        let stored = keychain.get(&account.id).unwrap().expect("token preserved");
        assert_eq!(stored.expose_secret(), "old-token");

        // Metadata is untouched.
        let unchanged = store
            .list()
            .unwrap()
            .into_iter()
            .find(|a| a.id == account.id)
            .unwrap();
        assert_eq!(unchanged.scopes, vec!["repo".to_string()]);
        assert_eq!(
            unchanged.expires_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
    }

    #[test]
    fn apply_swap_login_match_is_case_sensitive() {
        // GitHub logins are normalised lowercase by the API, so a literal
        // mismatch (even on case) means the PAT belongs to a different
        // identity. Conservative: refuse the swap rather than silently
        // changing the row's login casing.
        let store = fresh_store();
        let account = seed(store.as_ref(), 1, "Ada");
        let keychain = MockKeychain::new();

        let new = SecretString::from("tok".to_string());
        let result = apply_token_swap(
            store.as_ref(),
            |handle, token| {
                keychain
                    .set(&handle.id, token)
                    .map_err(|_| AuthCommandError::Internal)
            },
            &account,
            &validated("ada", &[], None),
            &new,
        );

        assert!(matches!(
            result,
            Err(AuthCommandError::LoginMismatch { .. })
        ));
        assert!(keychain.get(&account.id).unwrap().is_none());
    }

    /// Store impl that fails on `upsert` so the test can assert the helper
    /// treats the metadata refresh as best-effort.
    struct UpsertFailingStore {
        inner: Arc<SqlAccountStore>,
    }

    impl AccountStore for UpsertFailingStore {
        fn list(&self) -> Result<Vec<Account>, StoreError> {
            self.inner.list()
        }
        fn upsert(&self, _account: Account) -> Result<(), StoreError> {
            Err(StoreError::Io("simulated".into()))
        }
        fn remove(&self, id: AccountId) -> Result<(), StoreError> {
            self.inner.remove(id)
        }
        fn next_id(&self) -> Result<AccountId, StoreError> {
            self.inner.next_id()
        }
    }

    #[test]
    fn apply_swap_succeeds_even_if_metadata_refresh_fails() {
        // The new token is what matters: a metadata refresh failure (e.g.
        // DB temporarily locked) must not be reported as a re-auth failure
        // because the keychain write already succeeded.
        let inner = fresh_store();
        let account = seed(inner.as_ref(), 1, "ada");
        let store = UpsertFailingStore {
            inner: inner.clone(),
        };
        let keychain = MockKeychain::new();

        let new = SecretString::from("tok".to_string());
        let result = apply_token_swap(
            &store,
            |handle, token| {
                keychain
                    .set(&handle.id, token)
                    .map_err(|_| AuthCommandError::Internal)
            },
            &account,
            &validated("ada", &["repo"], None),
            &new,
        );
        assert!(result.is_ok());

        use secrecy::ExposeSecret;
        let stored = keychain.get(&account.id).unwrap().expect("token persists");
        assert_eq!(stored.expose_secret(), "tok");
    }

    #[test]
    fn on_token_updated_default_impl_is_a_noop() {
        // The trait default exists so existing listeners (and the test
        // `NoopAccountListener`) don't need to opt in. Exercising it here
        // documents the contract.
        let listener = NoopAccountListener;
        listener.on_token_updated(42);
    }

    // ────── add_account: duplicate (host, login) detection ──────

    #[test]
    fn find_duplicate_returns_none_on_empty_store() {
        let store = fresh_store();
        let got = find_duplicate(store.as_ref(), "github.com", "ada").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn find_duplicate_returns_existing_account_on_match() {
        // The connected (github.com, ada) account collides with a new PAT
        // resolved to the same identity. The renderer needs the existing
        // label to name the colliding account.
        let store = fresh_store();
        let _seeded = seed(store.as_ref(), 1, "ada");
        let got = find_duplicate(store.as_ref(), "github.com", "ada")
            .unwrap()
            .expect("expected duplicate");
        assert_eq!(got.id, 1);
        assert_eq!(got.label, "Test");
    }

    #[test]
    fn find_duplicate_does_not_match_different_host() {
        // Same login on a different host is a distinct identity. The
        // schema's UNIQUE (host, login) constraint permits this.
        let store = fresh_store();
        seed(store.as_ref(), 1, "ada");

        let got = find_duplicate(store.as_ref(), "github.acme.corp", "ada").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn find_duplicate_does_not_match_different_login() {
        let store = fresh_store();
        seed(store.as_ref(), 1, "ada");

        let got = find_duplicate(store.as_ref(), "github.com", "grace").unwrap();
        assert!(got.is_none());
    }

    // ────── AuthError -> AuthCommandError mapping (issue #240) ──────

    #[test]
    fn auth_command_error_from_backend_unavailable_preserves_hint() {
        // The renderer keys on `kind: "keychain_unavailable"` to show a
        // platform-specific install hint. The hint string must survive the
        // From<AuthError> conversion verbatim.
        use crate::auth::keychain::KeychainError;
        use crate::github::auth::AuthError;
        let err: AuthCommandError = AuthError::Keychain(KeychainError::BackendUnavailable {
            hint: "Install libsecret/gnome-keyring and ensure it's running.".into(),
        })
        .into();
        match err {
            AuthCommandError::KeychainUnavailable { hint } => {
                assert!(hint.contains("libsecret"));
            }
            other => panic!("expected KeychainUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn auth_command_error_from_access_denied_maps_to_distinct_arm() {
        use crate::auth::keychain::KeychainError;
        use crate::github::auth::AuthError;
        let err: AuthCommandError = AuthError::Keychain(KeychainError::AccessDenied).into();
        assert!(matches!(err, AuthCommandError::KeychainAccessDenied));
    }

    #[test]
    fn auth_command_error_from_corrupted_falls_back_to_internal() {
        // `Corrupted` carries no actionable user-facing copy beyond
        // "something went wrong"; the renderer surfaces the generic line.
        use crate::auth::keychain::KeychainError;
        use crate::github::auth::AuthError;
        let err: AuthCommandError = AuthError::Keychain(KeychainError::Corrupted).into();
        assert!(matches!(err, AuthCommandError::Internal));
    }

    #[test]
    fn auth_command_error_from_other_falls_back_to_internal() {
        use crate::auth::keychain::KeychainError;
        use crate::github::auth::AuthError;
        let err: AuthCommandError =
            AuthError::Keychain(KeychainError::Other("unspecified".into())).into();
        assert!(matches!(err, AuthCommandError::Internal));
    }

    #[test]
    fn auth_command_error_from_missing_maps_to_unauthorized() {
        use crate::github::auth::AuthError;
        let err: AuthCommandError = AuthError::Missing(1).into();
        assert!(matches!(err, AuthCommandError::Unauthorized));
    }

    // ────── End-to-end: KeychainTokenSource + MockKeychain failure ──────

    #[test]
    fn store_token_surfaces_backend_unavailable_through_typed_chain() {
        // Drives the full chain: MockKeychain returns BackendUnavailable,
        // KeychainTokenSource wraps it in AuthError::Keychain, the command
        // boundary's `?` flows it through From<AuthError> to AuthCommandError.
        // This guards against regressions in the typed propagation across
        // any of the three hops.
        use crate::auth::keychain::MockFailure;
        use crate::auth::token_source::KeychainTokenSource;
        use crate::github::auth::AccountHandle;

        let mock = MockKeychain::new();
        mock.inject_failure(MockFailure::BackendUnavailable {
            hint: "Install libsecret/gnome-keyring and ensure it's running.".into(),
        });
        let src = KeychainTokenSource::new(mock);
        let handle = AccountHandle::new(1, "github.com", "ada");

        let auth_err = src.store(&handle, "tok").expect_err("expected failure");
        let cmd_err: AuthCommandError = auth_err.into();
        match cmd_err {
            AuthCommandError::KeychainUnavailable { hint } => {
                assert!(hint.contains("libsecret"));
            }
            other => panic!("expected KeychainUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn store_token_surfaces_access_denied_through_typed_chain() {
        use crate::auth::keychain::MockFailure;
        use crate::auth::token_source::KeychainTokenSource;
        use crate::github::auth::AccountHandle;

        let mock = MockKeychain::new();
        mock.inject_failure(MockFailure::AccessDenied);
        let src = KeychainTokenSource::new(mock);
        let handle = AccountHandle::new(1, "github.com", "ada");

        let auth_err = src.store(&handle, "tok").expect_err("expected failure");
        let cmd_err: AuthCommandError = auth_err.into();
        assert!(matches!(cmd_err, AuthCommandError::KeychainAccessDenied));
    }

    #[test]
    fn token_get_surfaces_corrupted_through_typed_chain() {
        // `Corrupted` collapses to Internal at the command boundary because
        // there's no actionable user-facing copy. This test guards the
        // collapse so a future variant addition doesn't accidentally route
        // the wrong arm.
        use crate::auth::keychain::MockFailure;
        use crate::auth::token_source::KeychainTokenSource;
        use crate::github::auth::{AccountHandle, TokenSource};

        let mock = MockKeychain::new();
        mock.inject_failure(MockFailure::Corrupted);
        let src = KeychainTokenSource::new(mock);
        let handle = AccountHandle::new(1, "github.com", "ada");

        let auth_err = src.token(&handle).expect_err("expected failure");
        let cmd_err: AuthCommandError = auth_err.into();
        assert!(matches!(cmd_err, AuthCommandError::Internal));
    }

    #[test]
    fn keychain_unavailable_serialises_with_snake_case_kind_and_hint() {
        // Smoke-test the serialised shape the renderer keys on. If the serde
        // tag rename or the field name changes, this test catches it before
        // the renderer silently falls through to its generic branch.
        let err = AuthCommandError::KeychainUnavailable {
            hint: "Install libsecret/gnome-keyring and ensure it's running.".into(),
        };
        let json = serde_json::to_value(&err).expect("serialise");
        assert_eq!(json["kind"], "keychain_unavailable");
        assert!(json["hint"].as_str().unwrap().contains("libsecret"));
    }

    #[test]
    fn keychain_access_denied_serialises_with_snake_case_kind() {
        let err = AuthCommandError::KeychainAccessDenied;
        let json = serde_json::to_value(&err).expect("serialise");
        assert_eq!(json["kind"], "keychain_access_denied");
    }
}
