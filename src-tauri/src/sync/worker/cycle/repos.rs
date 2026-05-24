//! Row-shape DTOs and read helpers for the per-cycle repo + PR queries. The
//! cycle reads `repos` for the active account and the `pull_requests` slice
//! for each repo to drive the enrichment fan-out.

use rusqlite::params;

use crate::db::DbHandle;
use crate::github::AccountId;

#[derive(Debug)]
pub struct RepoRow {
    pub id: i64,
    pub owner: String,
    pub name: String,
}

#[derive(Debug)]
pub struct PrRow {
    pub id: i64,
    pub number: i64,
    /// Mirror of `pull_requests.updated_at` (unix seconds) at the moment the
    /// enrichment loop reads the row. Compared against the previous-cycle
    /// `pr-detail:{pr_id}` marker (stored via `client.cache_graphql_body`) to
    /// skip the GraphQL PR-detail round trip when nothing upstream has moved
    /// (issue #232).
    pub updated_at: i64,
}

pub fn list_repos_for_account(
    db: &DbHandle,
    account_id: AccountId,
) -> Result<Vec<RepoRow>, rusqlite::Error> {
    let conn = crate::db::lock_db(db)?;
    let mut stmt = conn
        .prepare("SELECT id, owner, name FROM repos WHERE account_id = ?1 ORDER BY owner, name")?;
    let rows = stmt
        .query_map(params![account_id as i64], |row| {
            Ok(RepoRow {
                id: row.get(0)?,
                owner: row.get(1)?,
                name: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn list_prs_for_repo(db: &DbHandle, repo_id: i64) -> Result<Vec<PrRow>, rusqlite::Error> {
    let conn = crate::db::lock_db(db)?;
    let mut stmt =
        conn.prepare("SELECT id, number, updated_at FROM pull_requests WHERE repo_id = ?1")?;
    let rows = stmt
        .query_map(params![repo_id], |row| {
            Ok(PrRow {
                id: row.get(0)?,
                number: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Cache key for the previous-cycle `updated_at` marker, scoped per PR. The
/// helper hides the format so callers don't grow string-formatting copies.
pub(super) fn pr_detail_marker_key(pr_id: i64) -> String {
    format!("pr-detail:{pr_id}")
}

/// Canonical bytes for the previous-cycle marker. Big-endian gives a stable
/// representation across hosts; we hash these bytes to compare against the
/// `body_sha256` slot the GraphQL cache stores.
pub(super) fn pr_detail_marker_bytes(updated_at: i64) -> [u8; 8] {
    updated_at.to_be_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    /// Poison the DB mutex by panicking inside a thread that holds the lock,
    /// then assert the worker helpers surface a `rusqlite::Error` instead of
    /// propagating the panic up the cycle. The cycle's caller already converts
    /// these into `CycleOutcome::Failed` so the worker loop continues on the
    /// next interval (issue #238).
    #[test]
    fn list_repos_returns_error_when_mutex_poisoned() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        let db: DbHandle = Arc::new(Mutex::new(conn));

        let db_clone = db.clone();
        let handle = std::thread::spawn(move || {
            let _guard = db_clone.lock().expect("acquire lock");
            panic!("poison the mutex");
        });
        let _ = handle.join();
        assert!(
            db.lock().is_err(),
            "background panic should have poisoned the mutex"
        );

        let result = list_repos_for_account(&db, 1);
        assert!(
            result.is_err(),
            "list_repos_for_account should surface the poison as an error"
        );
    }

    #[test]
    fn list_prs_returns_error_when_mutex_poisoned() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        let db: DbHandle = Arc::new(Mutex::new(conn));

        let db_clone = db.clone();
        let _ = std::thread::spawn(move || {
            let _guard = db_clone.lock().expect("acquire lock");
            panic!("poison the mutex");
        })
        .join();

        let result = list_prs_for_repo(&db, 1);
        assert!(result.is_err());
    }
}
