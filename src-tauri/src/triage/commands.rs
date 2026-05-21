//! Tauri command surface for the triage module.
//!
//! Wave 2-A fills in the read-state writers: `mark_pr_read` resets the
//! mention counter and refreshes the read watermarks; `mark_pr_unread` clears
//! the read watermark while leaving the mention counter alone (only the sync
//! scanner ever bumps it). Both recompute `needs_attention` inside the same
//! transaction via [`crate::triage::query::recompute_needs_attention`].
//!
//! Wave 2-D will fill in `list_filter_chip_counts`. See
//! `docs/contracts/triage-ux.md` ("Tauri command surface") for the contract.

use tauri::State;

use crate::dashboard::DashboardView;
use crate::db::DbHandle;
use crate::triage::query;
use crate::triage::types::FilterChipCounts;

/// Mark a PR as read for the given account. Sets
/// `pull_request_viewer_relations.read_at` to the current Unix timestamp,
/// captures `pull_requests.updated_at` into `read_pr_updated_at`, resets
/// `mentioned_count_unread` to zero, and pushes
/// `mention_scan_watermark_at` to the current timestamp so future sync
/// cycles only count comments newer than the open.
///
/// The composite `needs_attention` flag is recomputed against the new
/// state inside the same transaction so the next dashboard read reflects
/// the open.
///
/// Idempotent: re-marking an already-read PR is a no-op apart from
/// refreshing the timestamps. UPSERTs the relation row so the call works
/// for PRs the viewer has reached the detail surface for without a prior
/// discovery pass writing a row.
#[tauri::command]
pub fn mark_pr_read(
    pull_request_id: i64,
    account_id: i64,
    db: State<'_, DbHandle>,
) -> Result<(), String> {
    let mut conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    let tx = conn.transaction().map_err(|e| format!("begin tx: {e}"))?;
    query::mark_read(&tx, account_id, pull_request_id).map_err(|e| format!("mark read: {e}"))?;
    query::recompute_needs_attention(&tx, account_id, pull_request_id)
        .map_err(|e| format!("recompute needs_attention: {e}"))?;
    tx.commit().map_err(|e| format!("commit tx: {e}"))?;
    Ok(())
}

/// Flip a PR back to unread for the given account. Clears
/// `read_at` and `read_pr_updated_at` so the derived `unread` projection
/// returns true. `mentioned_count_unread` is _not_ rewritten - the next
/// sync cycle re-counts comments past the existing
/// `mention_scan_watermark_at` if any matched.
///
/// Recomputes `needs_attention` synchronously so the dashboard reflects
/// the flip without waiting for the next sync cycle.
///
/// No-op when the relation row doesn't exist; the contract is "Team-view
/// PRs never get a row" and marking such a PR unread is not a meaningful
/// operation.
#[tauri::command]
pub fn mark_pr_unread(
    pull_request_id: i64,
    account_id: i64,
    db: State<'_, DbHandle>,
) -> Result<(), String> {
    let mut conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    let tx = conn.transaction().map_err(|e| format!("begin tx: {e}"))?;
    query::mark_unread(&tx, account_id, pull_request_id)
        .map_err(|e| format!("mark unread: {e}"))?;
    query::recompute_needs_attention(&tx, account_id, pull_request_id)
        .map_err(|e| format!("recompute needs_attention: {e}"))?;
    tx.commit().map_err(|e| format!("commit tx: {e}"))?;
    Ok(())
}

/// Count how many PRs in the current view would match each filter chip
/// _independently_ of the other chips. The chips compose as AND at the
/// list level, but the counts are per-chip so the user sees what would
/// match if they toggled a single chip alone.
///
/// The view scope still applies (chips never cross view boundaries) so the
/// caller passes the active `DashboardView` + `account_id`. Returns
/// `FilterChipCounts` with one i64 per chip - see the type doc for the
/// per-chip predicate definitions.
#[tauri::command]
pub fn list_filter_chip_counts(
    view: DashboardView,
    account_id: i64,
    _db: State<'_, DbHandle>,
) -> Result<FilterChipCounts, String> {
    let _ = (view, account_id);
    unimplemented!("M4-D");
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    fn fresh_db() -> DbHandle {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        Arc::new(Mutex::new(conn))
    }

    /// Seeds a baseline (account, repo, PR, relation row) fixture used by
    /// every test below. `author_login` controls which signals fire on
    /// recompute; defaults flip none.
    fn seed(
        db: &DbHandle,
        author_login: &str,
        pr_updated_at: i64,
        threads_unresolved_involved: i64,
        review_decision: Option<&str>,
    ) {
        let conn = db.lock().unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, draft, author_login,
                 created_at, updated_at, base_ref, head_ref,
                 threads_unresolved_involved, review_decision)
                VALUES (100, 10, 1, 't', 'open', 0, '{author_login}',
                        0, {pr_updated_at}, 'main', 'feat',
                        {threads_unresolved_involved},
                        {review_decision_sql});
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, last_seen_at)
                VALUES (1, 100, 0, 0, 0, 0);",
            review_decision_sql = match review_decision {
                Some(s) => format!("'{s}'"),
                None => "NULL".to_string(),
            }
        ))
        .unwrap();
    }

    /// Helper: read the four triage columns for the test fixture's row.
    fn read_triage(db: &DbHandle) -> (Option<i64>, Option<i64>, i64, i64) {
        let conn = db.lock().unwrap();
        conn.query_row(
            "SELECT read_at, read_pr_updated_at,
                    mentioned_count_unread, needs_attention
               FROM pull_request_viewer_relations
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .unwrap()
    }

    /// Drive the same write path as the Tauri command without booting the
    /// state container. Mirrors the body of [`super::mark_pr_read`].
    fn invoke_mark_pr_read(db: &DbHandle, pr: i64, account: i64) -> Result<(), String> {
        let mut conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
        let tx = conn.transaction().map_err(|e| format!("begin tx: {e}"))?;
        query::mark_read(&tx, account, pr).map_err(|e| format!("mark read: {e}"))?;
        query::recompute_needs_attention(&tx, account, pr)
            .map_err(|e| format!("recompute needs_attention: {e}"))?;
        tx.commit().map_err(|e| format!("commit tx: {e}"))?;
        Ok(())
    }

    /// Mirrors [`super::mark_pr_unread`].
    fn invoke_mark_pr_unread(db: &DbHandle, pr: i64, account: i64) -> Result<(), String> {
        let mut conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
        let tx = conn.transaction().map_err(|e| format!("begin tx: {e}"))?;
        query::mark_unread(&tx, account, pr).map_err(|e| format!("mark unread: {e}"))?;
        query::recompute_needs_attention(&tx, account, pr)
            .map_err(|e| format!("recompute needs_attention: {e}"))?;
        tx.commit().map_err(|e| format!("commit tx: {e}"))?;
        Ok(())
    }

    #[test]
    fn mark_pr_read_sets_read_watermark_and_captures_updated_at() {
        let db = fresh_db();
        seed(&db, "bob", 1_700_000_000, 0, None);
        invoke_mark_pr_read(&db, 100, 1).unwrap();
        let (read_at, read_pr_updated_at, mentioned, _) = read_triage(&db);
        assert!(read_at.is_some(), "read_at should be set");
        assert_eq!(
            read_pr_updated_at,
            Some(1_700_000_000),
            "read_pr_updated_at snapshots pr.updated_at"
        );
        assert_eq!(mentioned, 0, "mentioned_count_unread reset to zero");
    }

    #[test]
    fn mark_pr_read_resets_mention_counter() {
        let db = fresh_db();
        seed(&db, "bob", 1_700_000_000, 0, None);
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "UPDATE pull_request_viewer_relations
                    SET mentioned_count_unread = 5
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
            )
            .unwrap();
        }
        invoke_mark_pr_read(&db, 100, 1).unwrap();
        let (_, _, mentioned, _) = read_triage(&db);
        assert_eq!(mentioned, 0);
    }

    #[test]
    fn mark_pr_read_recomputes_needs_attention_against_remaining_signals() {
        let db = fresh_db();
        // Author == viewer + unresolved involved threads => signal 1 fires
        // even after the read flip (mentions are zeroed, but threads remain).
        seed(&db, "alice", 1_700_000_000, 2, None);
        invoke_mark_pr_read(&db, 100, 1).unwrap();
        let (_, _, _, needs_attention) = read_triage(&db);
        assert_eq!(needs_attention, 1);
    }

    #[test]
    fn mark_pr_read_clears_attention_when_only_signal_was_mention() {
        let db = fresh_db();
        seed(&db, "bob", 1_700_000_000, 0, None);
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "UPDATE pull_request_viewer_relations
                    SET mentioned_count_unread = 3, needs_attention = 1
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
            )
            .unwrap();
        }
        invoke_mark_pr_read(&db, 100, 1).unwrap();
        let (_, _, mentioned, needs_attention) = read_triage(&db);
        assert_eq!(mentioned, 0);
        assert_eq!(needs_attention, 0, "read flip drops the only signal");
    }

    #[test]
    fn mark_pr_read_upserts_relation_row_when_missing() {
        let db = fresh_db();
        // Seed account + PR but no relation row.
        {
            let conn = db.lock().unwrap();
            conn.execute_batch(
                "INSERT INTO accounts (id, label, host, login, created_at)
                    VALUES (1, 'a', 'github.com', 'alice', 0);
                 INSERT INTO repos (id, account_id, owner, name, visibility)
                    VALUES (10, 1, 'owner', 'repo', 'public');
                 INSERT INTO pull_requests
                    (id, repo_id, number, title, state, draft, author_login,
                     created_at, updated_at, base_ref, head_ref)
                    VALUES (100, 10, 1, 't', 'open', 0, 'bob',
                            0, 1_700_000_000, 'main', 'feat');",
            )
            .unwrap();
        }
        invoke_mark_pr_read(&db, 100, 1).unwrap();
        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "row created by the auto-mark hook");
    }

    #[test]
    fn mark_pr_unread_clears_read_watermark() {
        let db = fresh_db();
        seed(&db, "bob", 1_700_000_000, 0, None);
        invoke_mark_pr_read(&db, 100, 1).unwrap();
        invoke_mark_pr_unread(&db, 100, 1).unwrap();
        let (read_at, read_pr_updated_at, _, _) = read_triage(&db);
        assert!(read_at.is_none());
        assert!(read_pr_updated_at.is_none());
    }

    #[test]
    fn mark_pr_unread_preserves_mention_counter() {
        let db = fresh_db();
        seed(&db, "bob", 1_700_000_000, 0, None);
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "UPDATE pull_request_viewer_relations
                    SET mentioned_count_unread = 4
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
            )
            .unwrap();
        }
        invoke_mark_pr_unread(&db, 100, 1).unwrap();
        let (_, _, mentioned, _) = read_triage(&db);
        assert_eq!(
            mentioned, 4,
            "unread flip never touches the mention counter"
        );
    }

    #[test]
    fn mark_pr_unread_recomputes_needs_attention() {
        let db = fresh_db();
        seed(&db, "alice", 1_700_000_000, 2, None);
        invoke_mark_pr_read(&db, 100, 1).unwrap();
        // After mark_pr_read, signal 1 keeps needs_attention = 1.
        let (_, _, _, before) = read_triage(&db);
        assert_eq!(before, 1);
        // Flip the threads counter to zero so the recompute can clear.
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "UPDATE pull_requests SET threads_unresolved_involved = 0
                  WHERE id = 100",
                [],
            )
            .unwrap();
        }
        invoke_mark_pr_unread(&db, 100, 1).unwrap();
        let (_, _, _, after) = read_triage(&db);
        assert_eq!(after, 0, "no signals left after thread clears");
    }
}
