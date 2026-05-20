//! SQLite-backed implementation of the shared `EtagStore` contract.
//!
//! The canonical trait and `EtagEntry` definitions live in
//! `src-tauri/src/github/etag.rs` (shipped by issue #11). This module
//! provides the production `SqliteEtagStore` impl backed by the `etags`
//! table from migration `0001_init.sql`.
//!
//! Key naming convention (from docs/contracts/github-client.md):
//!   * REST    -> `{account_id}:{method}:{path}`
//!   * GraphQL -> `{account_id}:gql:{query_hash}`

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::github::etag::{EtagEntry, EtagStore};

/// SQLite-backed `EtagStore`. The connection is wrapped in a `Mutex` because
/// `rusqlite::Connection` is `!Sync`; the lock window is one parameterised
/// query per call.
pub struct SqliteEtagStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteEtagStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }
}

impl EtagStore for SqliteEtagStore {
    fn get(&self, key: &str) -> Option<EtagEntry> {
        let conn = self.conn.lock().ok()?;
        let row: Option<(String, i64, Option<Vec<u8>>)> = conn
            .query_row(
                "SELECT etag, last_seen_at, body_sha256 FROM etags WHERE key = ?1",
                params![key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .ok()
            .flatten();

        row.map(|(etag, last_seen_at, body)| EtagEntry {
            etag,
            last_seen_at: from_unix_seconds(last_seen_at),
            body_sha256: body.and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok()),
        })
    }

    fn put(&self, key: &str, entry: EtagEntry) {
        let Ok(conn) = self.conn.lock() else { return };
        let last_seen_at = to_unix_seconds(entry.last_seen_at);
        let body_sha256: Option<&[u8]> = entry.body_sha256.as_ref().map(|b| b.as_slice());

        let _ = conn.execute(
            "INSERT INTO etags (key, etag, last_seen_at, body_sha256)
                 VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET
                 etag = excluded.etag,
                 last_seen_at = excluded.last_seen_at,
                 body_sha256 = excluded.body_sha256",
            params![key, entry.etag, last_seen_at, body_sha256],
        );
    }
}

fn to_unix_seconds(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        // Pre-epoch times round to 0; we never produce them in practice.
        Err(_) => 0,
    }
}

fn from_unix_seconds(secs: i64) -> SystemTime {
    if secs >= 0 {
        UNIX_EPOCH + Duration::from_secs(secs as u64)
    } else {
        UNIX_EPOCH
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrate;
    use std::sync::{Arc, Mutex};

    fn fresh_store() -> SqliteEtagStore {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate::run(&mut conn).unwrap();
        SqliteEtagStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn get_returns_none_for_missing_key() {
        let store = fresh_store();
        assert!(store.get("missing").is_none());
    }

    #[test]
    fn put_then_get_round_trips_entry_with_hash() {
        let store = fresh_store();
        let entry = EtagEntry {
            etag: "W/\"abc\"".to_string(),
            last_seen_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            body_sha256: Some([7u8; 32]),
        };

        store.put("1:gql:hash", entry.clone());
        let got = store.get("1:gql:hash").expect("entry present");

        assert_eq!(got, entry);
    }

    #[test]
    fn put_then_get_round_trips_entry_without_hash() {
        let store = fresh_store();
        let entry = EtagEntry {
            etag: "abc".to_string(),
            last_seen_at: UNIX_EPOCH + Duration::from_secs(42),
            body_sha256: None,
        };
        store.put("1:GET:/repos/foo/bar", entry.clone());
        assert_eq!(store.get("1:GET:/repos/foo/bar"), Some(entry));
    }

    #[test]
    fn put_upserts_existing_key() {
        let store = fresh_store();
        let key = "1:GET:/user";
        store.put(
            key,
            EtagEntry {
                etag: "old".into(),
                last_seen_at: UNIX_EPOCH + Duration::from_secs(1),
                body_sha256: None,
            },
        );
        store.put(
            key,
            EtagEntry {
                etag: "new".into(),
                last_seen_at: UNIX_EPOCH + Duration::from_secs(2),
                body_sha256: Some([1u8; 32]),
            },
        );

        let got = store.get(key).unwrap();
        assert_eq!(got.etag, "new");
        assert_eq!(got.body_sha256, Some([1u8; 32]));

        // No duplicate rows.
        let count: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM etags", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
