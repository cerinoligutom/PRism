//! macOS dock badge for the global `needs_attention` count (ADR 0017 decision 3).
//!
//! Two surfaces:
//!
//! * [`update_badge`] - the cross-platform entry point. `#[cfg(target_os = "macos")]`
//!   reads the count via [`count_global_needs_attention`] and pushes it onto
//!   the main webview window. Every other target is a no-op.
//! * [`BadgeSink`] / [`AppHandleBadge`] - the trait + production impl the sync
//!   worker holds inside `WorkerContext`. Mirrors `EmitSink` and
//!   `ReauthNotifier` so unit tests can capture refresh calls without booting
//!   Tauri.
//!
//! The count source is the unified, account-agnostic predicate from ADR 0018
//! decision 5: every relation with `needs_attention = 1 AND archived_at IS NULL`
//! contributes one. The badge reflects "PRism wants me" globally, independent
//! of the dashboard's account-scope filter, so multi-account users see one
//! number across every tracked host.
//!
//! Trigger surfaces:
//!
//! * The sync worker calls [`BadgeSink::refresh`] once per cycle, after the
//!   auto-archive sweep, so per-account fan-out and the archive sweep both
//!   feed into the same post-cycle update.
//! * The triage write commands (`mark_pr_read`, `mark_pr_unread`,
//!   `mark_pr_archived`, `mark_pr_unarchived`) and the conversation
//!   hydrator's auto-mark-on-open call [`refresh_from_db`] after their
//!   commit so the dock reflects the change without waiting for the next
//!   sync tick.

use rusqlite::Connection;
use tauri::{AppHandle, Runtime};

use crate::db::DbHandle;

/// Hard cap for the on-dock number. macOS renders the badge inside a small
/// circle; anything past three digits is illegible. Counts beyond the cap
/// clamp to 999 so the dock keeps rendering a fixed-width number rather than
/// growing the pill arbitrarily.
const BADGE_MAX: i64 = 999;

/// Push `count` onto the main window's dock badge on macOS. `count == 0`
/// clears the badge. Non-macOS builds are a no-op (logged at trace level via
/// `eprintln!` to match the project's current logging convention).
///
/// Counts above [`BADGE_MAX`] are clamped to 999. The macOS dock formats the
/// number itself; a `set_badge_label` "999+" variant exists, but mixing label
/// and count APIs forfeits the system's auto-formatting (font, padding) for
/// no functional gain at v1 scale.
///
/// Failures inside the Tauri call (missing window, plugin error) are logged
/// and swallowed - the badge is a convenience signal and never blocks the
/// sync loop or a triage command.
pub fn update_badge<R: Runtime>(app: &AppHandle<R>, count: i64) {
    apply_badge(app, count.clamp(0, BADGE_MAX));
}

/// Wrap the `Manager::get_webview_window` + `set_badge_count` call inside the
/// cfg gate so non-macOS builds don't carry the syscall.
#[cfg(target_os = "macos")]
fn apply_badge<R: Runtime>(app: &AppHandle<R>, count: i64) {
    use tauri::Manager;
    let Some(window) = app.get_webview_window("main") else {
        eprintln!("badge: main webview window missing, skipping update");
        return;
    };
    // `Some(0)` and `None` both clear the badge per Tauri's docs; pass `None`
    // explicitly so the intent is legible in stack traces.
    let payload = if count > 0 { Some(count) } else { None };
    if let Err(err) = window.set_badge_count(payload) {
        eprintln!("badge: set_badge_count failed: {err}");
    }
}

#[cfg(not(target_os = "macos"))]
fn apply_badge<R: Runtime>(_app: &AppHandle<R>, _count: i64) {
    // Documented gap (ADR 0017 decision 3). Windows / Linux land post-v1.
}

/// Read the global count from `db` and push it onto the dock. The Tauri
/// command surface (`mark_pr_read`, `mark_pr_unread`, `mark_pr_archived`,
/// `mark_pr_unarchived`, and the conversation hydrator's auto-mark-on-open)
/// call this once per write so the dock reflects the change without waiting
/// for the next sync cycle (ADR 0017 decision 3).
///
/// Errors at every step log and continue - the badge is a convenience signal
/// that should never propagate a failure into a triage command's return path.
pub fn refresh_from_db<R: Runtime>(app: &AppHandle<R>, db: &DbHandle) {
    let count = match db.lock() {
        Ok(conn) => count_global_needs_attention(&conn).unwrap_or_else(|err| {
            eprintln!("badge: count_global_needs_attention failed: {err}");
            0
        }),
        Err(err) => {
            eprintln!("badge: db poisoned: {err}");
            return;
        }
    };
    update_badge(app, count);
}

/// Count the global `needs_attention` total used by the dock badge.
///
/// Account-agnostic: every relation with `needs_attention = 1 AND archived_at
/// IS NULL` contributes one. Sums across every tracked account so a viewer
/// with two accounts sees the union, mirroring the description in ADR 0017
/// decision 3.
///
/// Reads a single COUNT against the partial index
/// `idx_pr_viewer_relations_attention`; no network round-trip.
pub fn count_global_needs_attention(conn: &Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(*) FROM pull_request_viewer_relations
          WHERE needs_attention = 1 AND archived_at IS NULL",
        [],
        |row| row.get::<_, i64>(0),
    )
}

/// Fire-and-forget surface the sync worker calls into. Mirrors `EmitSink` /
/// `ReauthNotifier`: no `Result`, no async, no boxed futures - a failed
/// badge update logs and continues so the loop never stalls.
pub trait BadgeSink: Send + Sync {
    /// Recompute the global count and push it onto the dock.
    fn refresh(&self);
}

/// Production [`BadgeSink`] wired to a Tauri `AppHandle` + shared
/// [`DbHandle`]. Hands the count straight to [`update_badge`].
pub struct AppHandleBadge<R: Runtime> {
    handle: AppHandle<R>,
    db: DbHandle,
}

impl<R: Runtime> AppHandleBadge<R> {
    pub fn new(handle: AppHandle<R>, db: DbHandle) -> Self {
        Self { handle, db }
    }
}

impl<R: Runtime> BadgeSink for AppHandleBadge<R> {
    fn refresh(&self) {
        refresh_from_db(&self.handle, &self.db);
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the count source. The Tauri-bound write path is
    //! exercised at app run time; here we only verify the SQL matches the
    //! ADR 0018 archive predicate.
    use super::*;
    use rusqlite::Connection;

    fn fresh_conn() -> Connection {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        conn
    }

    /// Seed an account / repo / PR / relation row. Each call adds one PR with
    /// the requested `needs_attention` + `archived_at` shape so the COUNT can
    /// be asserted incrementally across the table.
    fn seed_relation(conn: &Connection, pr_id: i64, needs_attention: i64, archived: bool) {
        // The first call inserts the account and repo; subsequent calls reuse
        // them. `INSERT OR IGNORE` keeps the helper idempotent for the
        // multi-row tests.
        conn.execute_batch(
            "INSERT OR IGNORE INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT OR IGNORE INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');",
        )
        .unwrap();
        let archived_sql = if archived {
            "strftime('%s','now')".to_string()
        } else {
            "NULL".to_string()
        };
        conn.execute_batch(&format!(
            "INSERT INTO pull_requests
                (id, repo_id, number, title, state, draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES ({pr_id}, 10, {pr_id}, 't', 'open', 0, 'bob',
                        0, 0, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, last_seen_at,
                 needs_attention, archived_at)
                VALUES (1, {pr_id}, 0, {needs_attention}, {archived_sql});"
        ))
        .unwrap();
    }

    #[test]
    fn count_global_needs_attention_returns_zero_on_empty_table() {
        let conn = fresh_conn();
        let count = count_global_needs_attention(&conn).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn count_global_needs_attention_counts_unarchived_attention_rows() {
        let conn = fresh_conn();
        seed_relation(&conn, 100, 1, false);
        seed_relation(&conn, 101, 1, false);
        let count = count_global_needs_attention(&conn).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn count_global_needs_attention_excludes_archived_rows() {
        // ADR 0018 decision 5: the badge predicate must drop archived
        // attention rows the same way the sidebar count chip does. Otherwise
        // archiving a PR would leave the dock bouncing forever.
        let conn = fresh_conn();
        seed_relation(&conn, 100, 1, false);
        seed_relation(&conn, 101, 1, true);
        seed_relation(&conn, 102, 1, true);
        let count = count_global_needs_attention(&conn).unwrap();
        assert_eq!(count, 1, "only the unarchived attention row contributes");
    }

    #[test]
    fn count_global_needs_attention_excludes_non_attention_rows() {
        let conn = fresh_conn();
        seed_relation(&conn, 100, 0, false);
        seed_relation(&conn, 101, 1, false);
        let count = count_global_needs_attention(&conn).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn count_global_needs_attention_sums_across_accounts() {
        // Cross-account union: two accounts both flag the same PR via
        // separate relation rows, the badge counts both. ADR 0017 calls out
        // that the dock reflects the global state, not a per-account scope.
        let conn = fresh_conn();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (2, 'b', 'github.com', 'bob', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 0, 'bob',
                        0, 0, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, last_seen_at, needs_attention)
                VALUES (1, 100, 0, 1);
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, last_seen_at, needs_attention)
                VALUES (2, 100, 0, 1);",
        )
        .unwrap();
        let count = count_global_needs_attention(&conn).unwrap();
        assert_eq!(count, 2, "both accounts' relation rows contribute");
    }
}
