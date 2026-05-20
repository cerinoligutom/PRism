//! Account metadata store.
//!
//! Persists the non-secret half of an account (label, host, login, scopes,
//! expiry) to the `accounts` table in the SQLite cache. The PAT itself is
//! never stored here — it lives in the OS keychain under `(SERVICE,
//! account_id)`.
//!
//! The trait exists so tests can substitute an in-memory backing store
//! without touching SQLite.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};
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
    #[error("no account with id {0}")]
    NotFound(AccountId),
}

pub trait AccountStore: Send + Sync {
    fn list(&self) -> Result<Vec<Account>, StoreError>;
    fn upsert(&self, account: Account) -> Result<(), StoreError>;
    fn remove(&self, id: AccountId) -> Result<(), StoreError>;
    fn next_id(&self) -> Result<AccountId, StoreError>;
}

/// SQLite-backed `AccountStore`. The connection is wrapped in a `Mutex`
/// because `rusqlite::Connection` is `!Sync`; the lock window is one
/// parameterised query per call.
pub struct SqlAccountStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqlAccountStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        self.conn
            .lock()
            .map_err(|e| StoreError::Io(format!("account store poisoned: {e}")))
    }
}

impl AccountStore for SqlAccountStore {
    fn list(&self) -> Result<Vec<Account>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, label, host, login, scopes, expires_at
                     FROM accounts
                     ORDER BY id",
            )
            .map_err(|e| StoreError::Io(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_account)
            .map_err(|e| StoreError::Io(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Io(e.to_string()))
    }

    fn upsert(&self, account: Account) -> Result<(), StoreError> {
        let conn = self.lock()?;
        let scopes = encode_scopes(&account.scopes);
        conn.execute(
            "INSERT INTO accounts (id, label, host, login, scopes, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, unixepoch(), ?6)
             ON CONFLICT(id) DO UPDATE SET
                 label      = excluded.label,
                 host       = excluded.host,
                 login      = excluded.login,
                 scopes     = excluded.scopes,
                 expires_at = excluded.expires_at",
            params![
                account.id as i64,
                account.label,
                account.host,
                account.login,
                scopes,
                account.expires_at,
            ],
        )
        .map(|_| ())
        .map_err(|e| StoreError::Io(e.to_string()))
    }

    fn remove(&self, id: AccountId) -> Result<(), StoreError> {
        let conn = self.lock()?;
        let affected = conn
            .execute("DELETE FROM accounts WHERE id = ?1", params![id as i64])
            .map_err(|e| StoreError::Io(e.to_string()))?;
        if affected == 0 {
            Err(StoreError::NotFound(id))
        } else {
            Ok(())
        }
    }

    fn next_id(&self) -> Result<AccountId, StoreError> {
        let conn = self.lock()?;
        let max_id: i64 = conn
            .query_row("SELECT IFNULL(MAX(id), 0) FROM accounts", [], |row| {
                row.get(0)
            })
            .map_err(|e| StoreError::Io(e.to_string()))?;
        Ok((max_id as AccountId) + 1)
    }
}

fn row_to_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    let id: i64 = row.get(0)?;
    let scopes: String = row.get(4)?;
    Ok(Account {
        id: id as AccountId,
        label: row.get(1)?,
        host: row.get(2)?,
        login: row.get(3)?,
        scopes: decode_scopes(&scopes),
        expires_at: row.get(5)?,
    })
}

/// Scopes are stored as a comma-joined string. GitHub scope names contain
/// colons (`read:org`) and word characters only — no commas — so CSV is a
/// safe, trivially round-trippable encoding for the `TEXT` column.
fn encode_scopes(scopes: &[String]) -> String {
    scopes.join(",")
}

fn decode_scopes(raw: &str) -> Vec<String> {
    if raw.is_empty() {
        Vec::new()
    } else {
        raw.split(',').map(|s| s.to_string()).collect()
    }
}

/// One-shot import of the legacy `accounts.json` (the pre-#62 store) into the
/// SQL `accounts` table. If the file is absent, or the SQL table already has
/// rows, this is a no-op. On success the file is renamed to `accounts.json.bak`
/// so the import doesn't run again on subsequent startups.
///
/// Best-effort: anything that goes wrong is logged and swallowed so a corrupt
/// legacy file can't block startup.
pub fn import_legacy_json_if_present(
    store: &SqlAccountStore,
    data_dir: &Path,
) -> Result<(), StoreError> {
    let path = data_dir.join("accounts.json");
    if !path.exists() {
        return Ok(());
    }

    if !store.list()?.is_empty() {
        // Table is already populated — assume the swap has already happened.
        // Leave the legacy file alone so we can inspect it manually if needed.
        return Ok(());
    }

    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("legacy accounts.json: read failed ({e}); skipping import");
            return Ok(());
        }
    };
    if raw.trim().is_empty() {
        let _ = std::fs::rename(&path, path.with_extension("json.bak"));
        return Ok(());
    }
    let parsed: LegacyPersisted = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("legacy accounts.json: parse failed ({e}); skipping import");
            return Ok(());
        }
    };

    for account in &parsed.accounts {
        if let Err(e) = store.upsert(account.clone()) {
            eprintln!(
                "legacy accounts.json: upsert id={} failed: {e}; continuing",
                account.id
            );
        }
    }

    if let Err(e) = std::fs::rename(&path, path.with_extension("json.bak")) {
        eprintln!("legacy accounts.json: rename to .bak failed: {e}");
    }
    Ok(())
}

/// Shape of the legacy JSON file. Matches the schema written by the
/// pre-#62 `JsonAccountStore`.
#[derive(Default, Deserialize)]
struct LegacyPersisted {
    #[serde(default)]
    accounts: Vec<Account>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrate;
    use tempfile::TempDir;

    fn fresh_store() -> SqlAccountStore {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        migrate::run(&mut conn).expect("run migrations");
        SqlAccountStore::new(Arc::new(Mutex::new(conn)))
    }

    fn sample(id: AccountId, label: &str) -> Account {
        Account {
            id,
            label: label.into(),
            host: "github.com".into(),
            login: "ada".into(),
            scopes: vec!["repo".into(), "read:org".into()],
            expires_at: Some("2026-09-01T00:00:00Z".into()),
        }
    }

    #[test]
    fn list_is_empty_on_fresh_db() {
        let store = fresh_store();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn upsert_inserts_then_round_trips() {
        let store = fresh_store();
        store.upsert(sample(1, "Work")).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], sample(1, "Work"));
    }

    #[test]
    fn upsert_updates_existing_account_in_place() {
        let store = fresh_store();
        store.upsert(sample(1, "Work")).unwrap();

        let mut renamed = sample(1, "Renamed");
        renamed.scopes = vec!["repo".into()];
        renamed.expires_at = None;
        store.upsert(renamed.clone()).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], renamed);
    }

    #[test]
    fn list_returns_accounts_ordered_by_id() {
        let store = fresh_store();
        let mut second = sample(2, "B");
        second.login = "grace".into();
        store.upsert(second).unwrap();
        store.upsert(sample(1, "A")).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.iter().map(|a| a.id).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn remove_deletes_only_the_matching_account() {
        let store = fresh_store();
        let mut second = sample(2, "B");
        second.login = "grace".into();
        store.upsert(sample(1, "A")).unwrap();
        store.upsert(second).unwrap();
        store.remove(1).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, 2);
    }

    #[test]
    fn remove_returns_not_found_for_unknown_id() {
        let store = fresh_store();
        let err = store.remove(99).expect_err("expected NotFound");
        assert!(matches!(err, StoreError::NotFound(99)));
    }

    #[test]
    fn next_id_increments_with_each_upsert() {
        let store = fresh_store();
        assert_eq!(store.next_id().unwrap(), 1);

        store.upsert(sample(1, "Work")).unwrap();
        assert_eq!(store.next_id().unwrap(), 2);
    }

    #[test]
    fn scopes_round_trip_when_empty() {
        let store = fresh_store();
        let mut acc = sample(1, "Work");
        acc.scopes = Vec::new();
        store.upsert(acc.clone()).unwrap();

        assert_eq!(store.list().unwrap()[0].scopes, Vec::<String>::new());
    }

    #[test]
    fn legacy_import_populates_empty_db() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.json");
        std::fs::write(
            &path,
            r#"{
                "next_id": 3,
                "accounts": [
                    {"id": 1, "label": "Work", "host": "github.com",
                     "login": "ada", "scopes": ["repo"], "expires_at": null},
                    {"id": 2, "label": "OSS", "host": "github.com",
                     "login": "grace", "scopes": ["repo", "read:org"],
                     "expires_at": "2026-09-01T00:00:00Z"}
                ]
            }"#,
        )
        .unwrap();

        let store = fresh_store();
        import_legacy_json_if_present(&store, dir.path()).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].label, "Work");
        assert_eq!(
            listed[1].scopes,
            vec!["repo".to_string(), "read:org".into()]
        );

        // File should have been renamed.
        assert!(!path.exists());
        assert!(dir.path().join("accounts.json.bak").exists());
    }

    #[test]
    fn legacy_import_is_noop_when_db_already_has_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.json");
        std::fs::write(
            &path,
            r#"{"next_id":1,"accounts":[
                {"id":1,"label":"Stale","host":"github.com",
                 "login":"x","scopes":[],"expires_at":null}
            ]}"#,
        )
        .unwrap();

        let store = fresh_store();
        store.upsert(sample(7, "Live")).unwrap();
        import_legacy_json_if_present(&store, dir.path()).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, 7);
        // Legacy file is left in place so we can inspect it.
        assert!(path.exists());
    }

    #[test]
    fn legacy_import_is_noop_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let store = fresh_store();
        import_legacy_json_if_present(&store, dir.path()).unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
