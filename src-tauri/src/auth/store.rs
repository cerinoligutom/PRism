//! Account metadata store.
//!
//! Persists the non-secret half of an account (label, host, login, scopes,
//! expiry) to a JSON file under the app data directory. When PR #9 lands
//! its SQLite schema, this whole file is swapped for a sqlx-backed
//! `AccountRepository` against the `accounts` table — the public surface
//! (the `AccountStore` trait) is what the rest of the crate depends on.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use crate::github::auth::{AccountHandle, AccountId};

/// Non-secret account metadata. The PAT itself is never stored here — it
/// lives in the OS keychain under `(SERVICE, account_id)`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub id: AccountId,
    pub label: String,
    pub host: String,
    pub login: String,
    pub scopes: Vec<String>,
    /// RFC-3339 timestamp from the GitHub `github-authentication-token-expiration`
    /// response header, or `None` if the header was absent (classic PATs
    /// without an expiry, or fine-grained PATs the user has not set one on).
    pub expires_at: Option<String>,
}

impl Account {
    /// Build the per-account handle the GitHub HTTP layer expects.
    /// Replaces the `impl From<&Account> for AccountHandle` we used while the
    /// canonical type lived locally — orphan rules forbid that direction now.
    pub fn handle(&self) -> AccountHandle {
        AccountHandle {
            id: self.id,
            host: self.host.clone(),
            label: self.label.clone(),
        }
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("account store I/O: {0}")]
    Io(String),
    #[error("account store serialise: {0}")]
    Serialise(String),
    #[error("no account with id {0}")]
    NotFound(AccountId),
}

pub trait AccountStore: Send + Sync {
    fn list(&self) -> Result<Vec<Account>, StoreError>;
    fn upsert(&self, account: Account) -> Result<(), StoreError>;
    fn remove(&self, id: AccountId) -> Result<(), StoreError>;
    fn next_id(&self) -> Result<AccountId, StoreError>;
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    next_id: AccountId,
    accounts: Vec<Account>,
}

/// JSON-backed file store. The whole file is rewritten on every mutation —
/// fine for the cardinality expected (single-digit accounts). The internal
/// `Mutex` makes concurrent commands safe.
pub struct JsonAccountStore {
    path: PathBuf,
    inner: Mutex<Persisted>,
}

impl JsonAccountStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let state = if path.exists() {
            let raw = std::fs::read_to_string(&path).map_err(|e| StoreError::Io(e.to_string()))?;
            if raw.trim().is_empty() {
                Persisted::default()
            } else {
                serde_json::from_str(&raw).map_err(|e| StoreError::Serialise(e.to_string()))?
            }
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| StoreError::Io(e.to_string()))?;
            }
            Persisted::default()
        };
        Ok(Self {
            path,
            inner: Mutex::new(state),
        })
    }

    fn flush(&self, state: &Persisted) -> Result<(), StoreError> {
        let raw = serde_json::to_string_pretty(state)
            .map_err(|e| StoreError::Serialise(e.to_string()))?;
        std::fs::write(&self.path, raw).map_err(|e| StoreError::Io(e.to_string()))
    }
}

impl AccountStore for JsonAccountStore {
    fn list(&self) -> Result<Vec<Account>, StoreError> {
        let guard = self.inner.lock().expect("account store poisoned");
        Ok(guard.accounts.clone())
    }

    fn upsert(&self, account: Account) -> Result<(), StoreError> {
        let mut guard = self.inner.lock().expect("account store poisoned");
        if let Some(existing) = guard.accounts.iter_mut().find(|a| a.id == account.id) {
            *existing = account;
        } else {
            if account.id >= guard.next_id {
                guard.next_id = account.id + 1;
            }
            guard.accounts.push(account);
        }
        self.flush(&guard)
    }

    fn remove(&self, id: AccountId) -> Result<(), StoreError> {
        let mut guard = self.inner.lock().expect("account store poisoned");
        let before = guard.accounts.len();
        guard.accounts.retain(|a| a.id != id);
        if guard.accounts.len() == before {
            return Err(StoreError::NotFound(id));
        }
        self.flush(&guard)
    }

    fn next_id(&self) -> Result<AccountId, StoreError> {
        let mut guard = self.inner.lock().expect("account store poisoned");
        let id = guard.next_id.max(1);
        guard.next_id = id + 1;
        self.flush(&guard)?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn account(id: AccountId, label: &str) -> Account {
        Account {
            id,
            label: label.into(),
            host: "github.com".into(),
            login: "ada".into(),
            scopes: vec!["repo".into()],
            expires_at: None,
        }
    }

    #[test]
    fn open_creates_empty_store_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let store = JsonAccountStore::open(dir.path().join("accounts.json")).unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn upsert_persists_account_across_reopen() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.json");
        {
            let store = JsonAccountStore::open(&path).unwrap();
            store.upsert(account(1, "Work")).unwrap();
        }
        let reopened = JsonAccountStore::open(&path).unwrap();
        let listed = reopened.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].label, "Work");
    }

    #[test]
    fn upsert_updates_existing_account_in_place() {
        let dir = TempDir::new().unwrap();
        let store = JsonAccountStore::open(dir.path().join("a.json")).unwrap();
        store.upsert(account(1, "Work")).unwrap();
        store.upsert(account(1, "Renamed")).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].label, "Renamed");
    }

    #[test]
    fn remove_deletes_only_the_matching_account() {
        let dir = TempDir::new().unwrap();
        let store = JsonAccountStore::open(dir.path().join("a.json")).unwrap();
        store.upsert(account(1, "A")).unwrap();
        store.upsert(account(2, "B")).unwrap();
        store.remove(1).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, 2);
    }

    #[test]
    fn remove_returns_not_found_for_unknown_id() {
        let dir = TempDir::new().unwrap();
        let store = JsonAccountStore::open(dir.path().join("a.json")).unwrap();
        let err = store.remove(99).expect_err("expected NotFound");
        assert!(matches!(err, StoreError::NotFound(99)));
    }

    #[test]
    fn next_id_increments_and_persists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.json");
        let first = {
            let store = JsonAccountStore::open(&path).unwrap();
            store.next_id().unwrap()
        };
        let second = {
            let store = JsonAccountStore::open(&path).unwrap();
            store.next_id().unwrap()
        };
        assert_eq!(first, 1);
        assert_eq!(second, 2);
    }
}
