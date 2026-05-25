//! Tauri command surface for the persistent notifications inbox.
//!
//! Foundation slice (#378) shipped the read / delete / clear commands; the
//! read/unread slice (#379) adds the three mark / count commands:
//!
//! * [`list_notifications`] - read the inbox, newest first.
//! * [`delete_notification`] - drop one row by id.
//! * [`clear_all_notifications`] - wipe every row.
//! * [`mark_notification_read`] - stamp `read_at` on one row.
//! * [`mark_all_notifications_read`] - stamp `read_at` on every unread row.
//! * [`unread_notification_count`] - read the current unread total.
//!
//! Read-after-write is the renderer's job: the v1 store calls `load()` after
//! a delete / clear rather than threading the post-write state through the
//! command return. This keeps the surface narrow and parallels the existing
//! triage commands. The mark-read commands return the just-marked count so
//! the sidebar chip can settle without a separate count round-trip when
//! it's already in sync.
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
/// row count stays bounded by the two-trigger surface. `before_id` seeds a
/// future paginated load - a `Some(id)` returns rows strictly older than
/// `id`.
#[tauri::command]
pub fn list_notifications(
    limit: Option<i64>,
    before_id: Option<i64>,
    db: State<'_, DbHandle>,
) -> Result<Vec<Notification>, NotificationsCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    store::list(&conn, limit, before_id).map_err(|e| internal(&format!("list_notifications: {e}")))
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
pub fn clear_all_notifications(db: State<'_, DbHandle>) -> Result<(), NotificationsCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    store::delete_all(&conn)
        .map(|_| ())
        .map_err(|e| internal(&format!("clear_all_notifications: {e}")))
}

/// Mark one row read. Idempotent: a double click on the same row keeps the
/// original `read_at` so the "when did the user see this" signal stays
/// truthful. Returns `()` because the frontend optimistically updates its
/// local row state and only refetches the unread count.
#[tauri::command]
pub fn mark_notification_read(
    id: i64,
    db: State<'_, DbHandle>,
) -> Result<(), NotificationsCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    store::mark_read(&conn, id)
        .map(|_| ())
        .map_err(|e| internal(&format!("mark_notification_read: {e}")))
}

/// Mark every unread row read. Returns the rows actually updated so the
/// caller can avoid a round-trip when the list was already fully read.
#[tauri::command]
pub fn mark_all_notifications_read(
    db: State<'_, DbHandle>,
) -> Result<i64, NotificationsCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    store::mark_all_read(&conn)
        .map(|n| n as i64)
        .map_err(|e| internal(&format!("mark_all_notifications_read: {e}")))
}

/// Current unread total. The sidebar chip uses this to refresh independently
/// of `list_notifications`, so a count tick doesn't have to drag the whole
/// list across the IPC boundary.
#[tauri::command]
pub fn unread_notification_count(
    db: State<'_, DbHandle>,
) -> Result<i64, NotificationsCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    store::count_unread(&conn).map_err(|e| internal(&format!("unread_notification_count: {e}")))
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

    fn invoke_mark_read(db: &DbHandle, id: i64) -> Result<(), NotificationsCommandError> {
        let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
        store::mark_read(&conn, id)
            .map(|_| ())
            .map_err(|e| internal(&format!("mark_notification_read: {e}")))
    }

    fn invoke_mark_all_read(db: &DbHandle) -> Result<i64, NotificationsCommandError> {
        let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
        store::mark_all_read(&conn)
            .map(|n| n as i64)
            .map_err(|e| internal(&format!("mark_all_notifications_read: {e}")))
    }

    fn invoke_unread_count(db: &DbHandle) -> Result<i64, NotificationsCommandError> {
        let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
        store::count_unread(&conn).map_err(|e| internal(&format!("unread_notification_count: {e}")))
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
    fn list_surfaces_read_at_after_mark_read() {
        // Issue #379: the list endpoint must carry the new column so the
        // frontend store can render the unread indicator without a separate
        // round-trip.
        let db = fresh_db();
        let a = seed(&db, 1);
        invoke_mark_read(&db, a).unwrap();
        let rows = invoke_list(&db, None, None).unwrap();
        let row = rows.iter().find(|r| r.id == a).expect("row");
        assert!(row.read_at.is_some(), "list must carry read_at");
    }

    #[test]
    fn unread_count_tracks_inserts_and_marks() {
        let db = fresh_db();
        assert_eq!(invoke_unread_count(&db).unwrap(), 0);
        let a = seed(&db, 1);
        seed(&db, 2);
        assert_eq!(invoke_unread_count(&db).unwrap(), 2);
        invoke_mark_read(&db, a).unwrap();
        assert_eq!(invoke_unread_count(&db).unwrap(), 1);
    }

    #[test]
    fn mark_all_read_returns_count_and_zeros_unread() {
        let db = fresh_db();
        seed(&db, 1);
        seed(&db, 2);
        seed(&db, 3);
        let marked = invoke_mark_all_read(&db).unwrap();
        assert_eq!(marked, 3);
        assert_eq!(invoke_unread_count(&db).unwrap(), 0);
        // Calling again returns 0 because there's nothing left unread; the
        // frontend uses that to skip the redundant refresh.
        assert_eq!(invoke_mark_all_read(&db).unwrap(), 0);
    }

    #[test]
    fn mark_read_is_idempotent_across_invocations() {
        let db = fresh_db();
        let a = seed(&db, 1);
        invoke_mark_read(&db, a).unwrap();
        // Second call must succeed without bumping the count.
        invoke_mark_read(&db, a).unwrap();
        assert_eq!(invoke_unread_count(&db).unwrap(), 0);
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
