//! Tauri commands exposed to the frontend.
//!
//! Token material never crosses this boundary — `add_account` receives the
//! PAT, validates it, writes it to the keychain, and returns only the
//! sanitised `Account` metadata. `list_accounts` and `remove_account` never
//! see tokens at all.

use std::path::PathBuf;
use std::sync::Arc;

use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use thiserror::Error;

use crate::auth::keychain::OsKeychain;
use crate::auth::store::{Account, AccountId, AccountStore, JsonAccountStore};
use crate::auth::token_source::KeychainTokenSource;
use crate::auth::validation::{validate_token, ValidationError};

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

/// Shared handle the Tauri runtime injects into every command. The store
/// and keychain backend are wrapped in `Arc` so testing can swap them
/// without touching the production builder in `lib.rs`.
pub struct AuthState {
    pub store: Arc<dyn AccountStore>,
    pub token_source: Arc<KeychainTokenSource<OsKeychain>>,
}

impl AuthState {
    pub fn new(data_dir: PathBuf) -> Result<Self, String> {
        let store_path = data_dir.join("accounts.json");
        let store =
            JsonAccountStore::open(&store_path).map_err(|e| format!("open account store: {e}"))?;
        Ok(Self {
            store: Arc::new(store),
            token_source: Arc::new(KeychainTokenSource::new(OsKeychain::new())),
        })
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
    };

    let handle = account.handle();
    state
        .token_source
        .store(&handle, secret_as_str(&secret))
        .map_err(|_| AuthCommandError::Internal)?;

    if let Err(e) = state.store.upsert(account.clone()) {
        // Roll the keychain write back so the account doesn't half-exist.
        let _ = state.token_source.remove(&handle);
        return Err(internal(&format!("persist account metadata: {e}")));
    }

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
    state
        .token_source
        .remove(&handle)
        .map_err(|_| AuthCommandError::Internal)?;

    state
        .store
        .remove(id)
        .map_err(|_| AuthCommandError::Internal)?;
    Ok(())
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
}

/// Standalone validation — used from the onboarding "Connect →" button to
/// surface "Token validated · X scopes" feedback before the user commits.
/// Does not store anything in the keychain.
#[tauri::command]
pub async fn validate_token_cmd(
    input: ValidateTokenInput,
) -> Result<ValidateTokenResult, AuthCommandError> {
    let host = normalise_host(&input.host);
    let secret = SecretString::from(input.token);
    let validated = validate_token(&host, &secret).await?;
    Ok(ValidateTokenResult {
        login: validated.login,
        scopes: validated.scopes,
        expires_at: validated.expires_at,
    })
}

/// Fire-and-forget reauth event the sync worker can call after a 401.
pub fn emit_reauth_required<R: Runtime>(app: &AppHandle<R>, account: &Account) {
    let payload = ReauthRequired {
        account_id: account.id,
        label: account.label.clone(),
    };
    if let Err(e) = app.emit(REAUTH_EVENT, payload) {
        eprintln!("failed to emit {REAUTH_EVENT}: {e}");
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
    eprintln!("auth internal error: {message}");
    AuthCommandError::Internal
}

/// Wires `AuthState` into the running Tauri app. Called from `lib.rs` after
/// the builder is constructed so it can resolve the OS-specific app-data dir.
pub fn install<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolve app data dir: {e}"))?;
    std::fs::create_dir_all(&data_dir).map_err(|e| format!("create app data dir: {e}"))?;
    let state = AuthState::new(data_dir)?;
    app.manage(state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
