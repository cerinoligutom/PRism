//! Tauri command surface for the Settings -> Repositories panel.
//!
//! Three commands cover the panel's needs:
//! - [`list_repos_for_account`] — read every `repos` row for one account.
//! - [`set_repo_tracked`] — flip the `is_tracked` opt-in for one repo.
//! - [`refresh_account_repos`] — call GitHub's `/user/repos` and upsert.
//!
//! Token material never crosses the command boundary; the refresh command goes
//! through the shared [`GitHubClient`] which reads PATs from the OS keychain
//! per request.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use thiserror::Error;

use crate::auth::commands::AuthState;
use crate::auth::store::Account;
use crate::db::{DbHandle, SqliteEtagStore};
use crate::github::auth::TokenSource;
use crate::github::rest::{list_user_repos, ListRepos};
use crate::github::{EtagStore, GitHubClient, GitHubError};
use crate::repos::store;
use crate::repos::types::RepoSummary;

/// User-facing error shape for `repos::*` commands. Mirrors the
/// `AuthCommandError` pattern: internal failures fold into a single opaque
/// variant so internals never leak to the renderer (CLAUDE.md security rule).
#[derive(Debug, Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReposCommandError {
    #[error("account not found")]
    AccountNotFound,
    #[error("repo not found")]
    RepoNotFound,
    #[error("github token rejected")]
    Unauthorized,
    #[error("github rate limited")]
    RateLimited,
    #[error("could not reach {host}")]
    Network { host: String },
    #[error("an unexpected error occurred")]
    Internal,
}

impl From<GitHubError> for ReposCommandError {
    fn from(err: GitHubError) -> Self {
        match err {
            GitHubError::Unauthorized => ReposCommandError::Unauthorized,
            GitHubError::RateLimited { .. } => ReposCommandError::RateLimited,
            GitHubError::NotFound => ReposCommandError::RepoNotFound,
            GitHubError::Network(e) => ReposCommandError::Network {
                host: e
                    .url()
                    .and_then(|u| u.host_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| "github".into()),
            },
            other => internal(&format!("github request: {other}")),
        }
    }
}

#[tauri::command]
pub fn list_repos_for_account(
    account_id: i64,
    db: State<'_, DbHandle>,
) -> Result<Vec<RepoSummary>, ReposCommandError> {
    let conn = db.lock().map_err(|_| ReposCommandError::Internal)?;
    store::list_for_account(&conn, account_id).map_err(|e| internal(&format!("list repos: {e}")))
}

#[tauri::command]
pub fn set_repo_tracked(
    repo_id: i64,
    tracked: bool,
    db: State<'_, DbHandle>,
) -> Result<(), ReposCommandError> {
    let conn = db.lock().map_err(|_| ReposCommandError::Internal)?;
    let affected = store::set_tracked(&conn, repo_id, tracked)
        .map_err(|e| internal(&format!("update repo: {e}")))?;
    if affected == 0 {
        return Err(ReposCommandError::RepoNotFound);
    }
    Ok(())
}

/// Walk `/user/repos` for the given account and upsert into `repos`.
///
/// A 304 on the first page short-circuits to the existing rows (no upsert
/// needed). Any other 200-page result drives the upsert, then returns the full
/// post-write list.
#[tauri::command]
pub async fn refresh_account_repos(
    account_id: i64,
    auth: State<'_, AuthState>,
    db: State<'_, DbHandle>,
) -> Result<Vec<RepoSummary>, ReposCommandError> {
    let account = find_account(&auth, account_id)?;
    let client = build_client(db.inner().clone(), &auth, &account)?;

    let result = list_user_repos(&client).await?;
    let mut db_guard = db.lock().map_err(|_| ReposCommandError::Internal)?;

    match result {
        ListRepos::NotModified => store::list_for_account(&db_guard, account_id)
            .map_err(|e| internal(&format!("list repos: {e}"))),
        ListRepos::Repos(repos) => store::upsert_for_account(&mut db_guard, account_id, &repos)
            .map_err(|e| internal(&format!("upsert repos: {e}"))),
    }
}

fn find_account(auth: &AuthState, account_id: i64) -> Result<Account, ReposCommandError> {
    let id_u64 = u64::try_from(account_id).map_err(|_| ReposCommandError::AccountNotFound)?;
    let accounts = auth
        .store
        .list()
        .map_err(|e| internal(&format!("list accounts: {e}")))?;
    accounts
        .into_iter()
        .find(|a| a.id == id_u64)
        .ok_or(ReposCommandError::AccountNotFound)
}

fn build_client(
    db: DbHandle,
    auth: &AuthState,
    account: &Account,
) -> Result<GitHubClient, ReposCommandError> {
    let token_source: Arc<dyn TokenSource> = auth.token_source.clone();
    let etag_store: Arc<dyn EtagStore> = Arc::new(SqliteEtagStore::new(db));
    GitHubClient::builder()
        .account(account.handle())
        .token_source(token_source)
        .etag_store(etag_store)
        .build()
        .map_err(|e| internal(&format!("build client: {e}")))
}

fn internal(message: &str) -> ReposCommandError {
    eprintln!("repos command internal error: {message}");
    ReposCommandError::Internal
}
