//! Tauri command surface for the persistent notifications inbox.
//!
//! Three commands ship in the foundation slice (#378):
//!
//! * [`list_notifications`] - read the inbox, newest first.
//! * [`delete_notification`] - drop one row by id.
//! * [`clear_all_notifications`] - wipe every row.
//!
//! Read-after-write is the renderer's job: the v1 store calls `load()` after
//! a delete / clear rather than threading the post-write state through the
//! command return. This keeps the surface narrow and parallels the existing
//! triage commands.
//!
//! Errors fold into the same opaque shape every other command in this crate
//! uses so internals never leak to the renderer (CLAUDE.md security rule).

use serde::Serialize;
use tauri::State;
use thiserror::Error;

use crate::db::DbHandle;
use crate::notifications::store;
use crate::notifications::types::Notification;

/// User-facing error shape for `notifications::*` commands. Mirrors the
/// triage / dashboard convention: one opaque variant so rusqlite messages
/// don't leak to the renderer.
#[derive(Debug, Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NotificationsCommandError {
    #[error("an unexpected error occurred")]
    Internal,
}

fn internal(message: &str) -> NotificationsCommandError {
    tracing::error!(message, "notifications command internal error");
    NotificationsCommandError::Internal
}

/// Read the inbox, newest first.
///
/// `limit = None` returns every row; the v1 inbox has no cap on size and the
/// row count stays bounded by the two-trigger surface. `before_id` is the
/// seed for the follow-up paginated load (#379) - a `Some(id)` returns rows
/// strictly older than `id`.
#[tauri::command]
pub fn list_notifications(
    limit: Option<i64>,
    before_id: Option<i64>,
    db: State<'_, DbHandle>,
) -> Result<Vec<Notification>, NotificationsCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    store::list(&conn, limit, before_id)
        .map_err(|e| internal(&format!("list_notifications: {e}")))
}

/// Drop one inbox row. Returning `()` keeps the parallel with the triage
/// surface; the frontend re-reads the list to settle.
#[tauri::command]
pub fn delete_notification(
    id: i64,
    db: State<'_, DbHandle>,
) -> Result<(), NotificationsCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    store::delete_one(&conn, id)
        .map(|_| ())
        .map_err(|e| internal(&format!("delete_notification: {e}")))
}

/// Wipe every inbox row.
#[tauri::command]
pub fn clear_all_notifications(
    db: State<'_, DbHandle>,
) -> Result<(), NotificationsCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    store::delete_all(&conn)
        .map(|_| ())
        .map_err(|e| internal(&format!("clear_all_notifications: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::types::NotificationInsert;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    fn fresh_db() -> DbHandle {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);",
        )
        .unwrap();
        Arc::new(Mutex::new(conn))
    }

    fn seed(db: &DbHandle, number: i64) -> i64 {
        let conn = db.lock().unwrap();
        store::insert(
            &conn,
            &NotificationInsert {
                kind: "needs_attention".to_string(),
                account_id: 1,
                pull_request_id: None,
                owner: "owner".to_string(),
                repo: "web".to_string(),
                pr_number: number,
                pr_node_id: None,
                pr_title: format!("PR {number}"),
                title: "Needs your attention".to_string(),
                body: Some(format!("owner/web #{number}")),
            },
        )
        .unwrap()
    }

    fn invoke_list(
        db: &DbHandle,
        limit: Option<i64>,
        before_id: Option<i64>,
    ) -> Result<Vec<Notification>, NotificationsCommandError> {
        let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
        store::list(&conn, limit, before_id)
            .map_err(|e| internal(&format!("list_notifications: {e}")))
    }

    fn invoke_delete(db: &DbHandle, id: i64) -> Result<(), NotificationsCommandError> {
        let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
        store::delete_one(&conn, id)
            .map(|_| ())
            .map_err(|e| internal(&format!("delete_notification: {e}")))
    }

    fn invoke_clear(db: &DbHandle) -> Result<(), NotificationsCommandError> {
        let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
        store::delete_all(&conn)
            .map(|_| ())
            .map_err(|e| internal(&format!("clear_all_notifications: {e}")))
    }

    #[test]
    fn list_returns_newest_first() {
        let db = fresh_db();
        seed(&db, 1);
        seed(&db, 2);
        seed(&db, 3);
        let rows = invoke_list(&db, None, None).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].pr_number, 3);
    }

    #[test]
    fn delete_removes_the_row() {
        let db = fresh_db();
        let a = seed(&db, 1);
        let b = seed(&db, 2);
        invoke_delete(&db, a).unwrap();
        let rows = invoke_list(&db, None, None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, b);
    }

    #[test]
    fn clear_drops_every_row() {
        let db = fresh_db();
        for n in 1..=3 {
            seed(&db, n);
        }
        invoke_clear(&db).unwrap();
        let rows = invoke_list(&db, None, None).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn internal_variant_serialises_without_payload() {
        // CLAUDE.md: internal failure detail never reaches the renderer. The
        // variant carries no body so the JSON output exposes only the kind.
        let err = internal("rusqlite: column 'secret' does not exist");
        let serialised = serde_json::to_string(&err).expect("serialise");
        assert_eq!(serialised, r#"{"kind":"internal"}"#);
        assert!(!serialised.contains("rusqlite"));
        assert!(!serialised.contains("secret"));
    }
}
