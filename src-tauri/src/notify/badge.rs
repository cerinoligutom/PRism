//! macOS dock badge for the global unread-PR count (ADR 0017 decision 3,
//! refined post-M6 smoke to match Slack-style semantics).
//!
//! Two surfaces:
//!
//! * [`update_badge`] - the cross-platform entry point. `#[cfg(target_os = "macos")]`
//!   reads the count via [`count_global_unread`] and pushes it onto the main
//!   webview window. Every other target is a no-op.
//! * [`BadgeSink`] / [`AppHandleBadge`] - the trait + production impl the sync
//!   worker holds inside `WorkerContext`. Mirrors `EmitSink` and
//!   `ReauthNotifier` so unit tests can capture refresh calls without booting
//!   Tauri.
//!
//! The count source is the unified, account-agnostic unread predicate: every
//! open, unarchived PR with at least one relation flagged unread (per ADR 0015:
//! `read_at IS NULL OR pull_requests.updated_at > read_pr_updated_at`)
//! contributes one. The badge reflects "PRs you haven't caught up on" globally
//! (matching the row dot indicator) and is independent of the dashboard's
//! account-scope filter. Multi-account viewers see one number; a PR visible
//! from two accounts and unread on either counts once (`DISTINCT pr.id`).
//!
//! Why unread, not `needs_attention`: the M6 first cut wired the badge to the
//! curated `needs_attention` flag, but that means opening a row clears its
//! left-rail dot without dropping the badge unless the PR also crossed the
//! attention bar. The dot and the badge should track the same thing or users
//! lose trust in the dock signal.
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
        Ok(conn) => count_global_unread(&conn).unwrap_or_else(|err| {
            eprintln!("badge: count_global_unread failed: {err}");
            0
        }),
        Err(err) => {
            eprintln!("badge: db poisoned: {err}");
            return;
        }
    };
    update_badge(app, count);
}

/// Count the global unread total used by the dock badge.
///
/// DISTINCT over `pull_requests.id` so a PR visible from two accounts and
/// unread on either contributes one - the badge counts PRs (analogous to
/// Slack channels), not relation rows. Excludes archived rows (ADR 0018
/// decision 5) and closed / merged PRs (post-M6 default: only open work
/// contributes to the unread signal).
///
/// The unread predicate is ADR 0015's:
/// `read_at IS NULL OR pull_requests.updated_at > read_pr_updated_at`.
/// The same condition drives the row's left-rail dot, so the dock and the
/// row stay in sync.
pub fn count_global_unread(conn: &Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(DISTINCT pr.id)
           FROM pull_requests pr
           JOIN pull_request_viewer_relations rel
             ON rel.pull_request_id = pr.id
          WHERE rel.archived_at IS NULL
            AND pr.state = 'open'
            AND (rel.read_at IS NULL
                 OR pr.updated_at > COALESCE(rel.read_pr_updated_at, 0))",
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
    //! Unit tests for the unread count source. The Tauri-bound write path is
    //! exercised at app run time; here we verify the SQL matches the unread
    //! predicate from ADR 0015 plus the open + unarchived gates.
    use super::*;
    use rusqlite::Connection;

    fn fresh_conn() -> Connection {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        conn
    }

    /// Seed an account / repo / PR / relation row with the supplied shape.
    /// `updated_at` is the PR's last-activity timestamp; `read_at` and
    /// `read_pr_updated_at` model the viewer's read watermark. A row is
    /// unread iff `read_at IS NULL OR updated_at > read_pr_updated_at`.
    #[allow(clippy::too_many_arguments)]
    fn seed_pr(
        conn: &Connection,
        pr_id: i64,
        state: &str,
        updated_at: i64,
        archived: bool,
        read_at: Option<i64>,
        read_pr_updated_at: Option<i64>,
    ) {
        conn.execute_batch(
            "INSERT OR IGNORE INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT OR IGNORE INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');",
        )
        .unwrap();
        let archived_sql = if archived {
            "strftime('%s','now')"
        } else {
            "NULL"
        };
        let read_at_sql = read_at.map_or("NULL".into(), |v| v.to_string());
        let read_pr_updated_at_sql = read_pr_updated_at.map_or("NULL".into(), |v| v.to_string());
        conn.execute_batch(&format!(
            "INSERT INTO pull_requests
                (id, repo_id, number, title, state, draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES ({pr_id}, 10, {pr_id}, 't', '{state}', 0, 'bob',
                        0, {updated_at}, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, last_seen_at,
                 read_at, read_pr_updated_at, archived_at)
                VALUES (1, {pr_id}, 0,
                        {read_at_sql}, {read_pr_updated_at_sql}, {archived_sql});"
        ))
        .unwrap();
    }

    #[test]
    fn count_global_unread_returns_zero_on_empty_table() {
        let conn = fresh_conn();
        assert_eq!(count_global_unread(&conn).unwrap(), 0);
    }

    #[test]
    fn count_global_unread_counts_never_opened_prs() {
        // `read_at IS NULL` means the viewer has never opened the row, so the
        // PR is unread regardless of `updated_at`.
        let conn = fresh_conn();
        seed_pr(&conn, 100, "open", 1000, false, None, None);
        assert_eq!(count_global_unread(&conn).unwrap(), 1);
    }

    #[test]
    fn count_global_unread_excludes_caught_up_prs() {
        // `read_at` set AND `updated_at <= read_pr_updated_at` -> caught up.
        let conn = fresh_conn();
        seed_pr(&conn, 100, "open", 500, false, Some(600), Some(500));
        assert_eq!(count_global_unread(&conn).unwrap(), 0);
    }

    #[test]
    fn count_global_unread_includes_re_unread_after_new_activity() {
        // A PR opened earlier ticks back to unread when sync surfaces fresh
        // activity (`updated_at > read_pr_updated_at`).
        let conn = fresh_conn();
        seed_pr(&conn, 100, "open", 700, false, Some(600), Some(500));
        assert_eq!(count_global_unread(&conn).unwrap(), 1);
    }

    #[test]
    fn count_global_unread_excludes_archived_rows() {
        // ADR 0018 decision 5: archived rows do not contribute to any active
        // count.
        let conn = fresh_conn();
        seed_pr(&conn, 100, "open", 1000, true, None, None);
        assert_eq!(count_global_unread(&conn).unwrap(), 0);
    }

    #[test]
    fn count_global_unread_excludes_closed_and_merged_prs() {
        // Post-M6 default: only open PRs feed the unread signal.
        let conn = fresh_conn();
        seed_pr(&conn, 100, "closed", 1000, false, None, None);
        seed_pr(&conn, 101, "merged", 1000, false, None, None);
        seed_pr(&conn, 102, "open", 1000, false, None, None);
        assert_eq!(count_global_unread(&conn).unwrap(), 1);
    }

    #[test]
    fn count_global_unread_distincts_across_accounts() {
        // A PR visible from two accounts and unread on either contributes one
        // - DISTINCT over `pr.id`, matching Slack's "this channel has new
        // messages" rather than "this channel has N new messages per workspace".
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
                        0, 1000, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, last_seen_at, read_at)
                VALUES (1, 100, 0, NULL);
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, last_seen_at, read_at)
                VALUES (2, 100, 0, NULL);",
        )
        .unwrap();
        assert_eq!(
            count_global_unread(&conn).unwrap(),
            1,
            "two accounts on the same PR count as one unread"
        );
    }
}
