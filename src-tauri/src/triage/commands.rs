//! Tauri command surface for the triage module.
//!
//! Wave 2-A fills in the read-state writers: `mark_pr_read` resets the
//! mention counter and refreshes the read watermarks; `mark_pr_unread` clears
//! the read watermark while leaving the mention counter alone (only the sync
//! scanner ever bumps it). Both recompute `needs_attention` inside the same
//! transaction via [`crate::triage::query::recompute_needs_attention`].
//!
//! Wave 2-C adds `list_sidebar_attention_counts` - the per-view COUNT(*)
//! that drives the sidebar nav's `.has-attention` boost.
//!
//! Wave 2-D fills in `list_filter_chip_counts`. See
//! `docs/contracts/triage-ux.md` ("Tauri command surface") for the contract.

use tauri::State;

use crate::dashboard::DashboardView;
use crate::db::DbHandle;
use crate::triage::query;
use crate::triage::types::{FilterChipCounts, SidebarAttentionCounts};

/// Mark a PR as read.
///
/// `account_id = Some(id)` flips the read state for that single relation;
/// `account_id = None` (ADR 0016, unified mode) fans the flip out across every
/// existing relation owner for the PR. Each per-account write is independent:
/// a per-account failure during the fan-out logs and continues so partial
/// progress persists, matching ADR 0016's mark-read option 1.
///
/// In single-account mode the existing semantics hold: the relation row is
/// UPSERTed (so a PR the viewer reached without a prior discovery pass still
/// flips read). In multi-account mode the fan-out only writes to existing
/// relation rows - upserting against arbitrary accounts would manufacture
/// rows the sync cycle never validated.
///
/// The composite `needs_attention` flag is recomputed for each touched
/// `(account, PR)` pair inside the same transaction.
#[tauri::command]
pub fn mark_pr_read(
    pull_request_id: i64,
    account_id: Option<i64>,
    db: State<'_, DbHandle>,
) -> Result<(), String> {
    let mut conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    let tx = conn.transaction().map_err(|e| format!("begin tx: {e}"))?;
    match account_id {
        Some(id) => {
            query::mark_read(&tx, id, pull_request_id).map_err(|e| format!("mark read: {e}"))?;
            query::recompute_needs_attention(&tx, id, pull_request_id)
                .map_err(|e| format!("recompute needs_attention: {e}"))?;
        }
        None => {
            apply_to_all_relation_owners(&tx, pull_request_id, |tx, acct| {
                query::mark_read(tx, acct, pull_request_id)?;
                query::recompute_needs_attention(tx, acct, pull_request_id)
            })
            .map_err(|e| format!("mark read multi: {e}"))?;
        }
    }
    tx.commit().map_err(|e| format!("commit tx: {e}"))?;
    Ok(())
}

/// Flip a PR back to unread.
///
/// `account_id = Some(id)` clears the read watermark on that single relation;
/// `account_id = None` fans the clear out across every existing relation
/// owner. Per-account writes are independent so a partial failure doesn't roll
/// back successes (ADR 0016 mark-read option 1).
///
/// `mentioned_count_unread` is _not_ rewritten - the next sync cycle re-counts
/// comments past the existing `mention_scan_watermark_at` if any matched.
/// Recomputes `needs_attention` synchronously for each touched pair.
///
/// No-op when the relation row doesn't exist; the contract is "Team-view PRs
/// never get a row" and marking such a PR unread is not a meaningful
/// operation.
#[tauri::command]
pub fn mark_pr_unread(
    pull_request_id: i64,
    account_id: Option<i64>,
    db: State<'_, DbHandle>,
) -> Result<(), String> {
    let mut conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    let tx = conn.transaction().map_err(|e| format!("begin tx: {e}"))?;
    match account_id {
        Some(id) => {
            query::mark_unread(&tx, id, pull_request_id)
                .map_err(|e| format!("mark unread: {e}"))?;
            query::recompute_needs_attention(&tx, id, pull_request_id)
                .map_err(|e| format!("recompute needs_attention: {e}"))?;
        }
        None => {
            apply_to_all_relation_owners(&tx, pull_request_id, |tx, acct| {
                query::mark_unread(tx, acct, pull_request_id)?;
                query::recompute_needs_attention(tx, acct, pull_request_id)
            })
            .map_err(|e| format!("mark unread multi: {e}"))?;
        }
    }
    tx.commit().map_err(|e| format!("commit tx: {e}"))?;
    Ok(())
}

/// Iterate every account_id that has a relation row for `pull_request_id` and
/// invoke `op` once per account. Per-account failures are logged and skipped
/// (ADR 0016: "partial failures must not roll back successful per-account
/// writes"). Returns `Ok(())` even if every per-account write fails - the
/// outer transaction commits successful rows and surfaces nothing to the
/// frontend. The next sync cycle reconciles.
fn apply_to_all_relation_owners<F>(
    tx: &rusqlite::Transaction<'_>,
    pull_request_id: i64,
    mut op: F,
) -> Result<(), rusqlite::Error>
where
    F: FnMut(&rusqlite::Transaction<'_>, i64) -> Result<(), rusqlite::Error>,
{
    let mut stmt = tx.prepare(
        "SELECT account_id FROM pull_request_viewer_relations
          WHERE pull_request_id = ?1
          ORDER BY account_id",
    )?;
    let account_ids: Vec<i64> = stmt
        .query_map([pull_request_id], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    for account_id in account_ids {
        if let Err(err) = op(tx, account_id) {
            eprintln!(
                "per-account triage write failed (pr={pull_request_id}, \
                 account={account_id}): {err}"
            );
        }
    }
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
    db: State<'_, DbHandle>,
) -> Result<FilterChipCounts, String> {
    let conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    query::list_filter_chip_counts(&conn, view, account_id)
        .map_err(|e| format!("list_filter_chip_counts: {e}"))
}

/// Count PRs flagged `needs_attention = 1` for the active account, bucketed
/// by the four dashboard views. The sidebar nav uses these to boost the
/// count chip with the existing `.has-attention` class when any matching PR
/// is outstanding. Re-fetched on view change and on sync completion events.
///
/// Synchronous because the underlying query is a single `SELECT` over the
/// partial index `idx_pr_viewer_relations_attention` - no network round-trip.
#[tauri::command]
pub fn list_sidebar_attention_counts(
    account_id: i64,
    db: State<'_, DbHandle>,
) -> Result<SidebarAttentionCounts, String> {
    let conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    query::count_sidebar_attention(&conn, account_id)
        .map_err(|e| format!("list_sidebar_attention_counts: {e}"))
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
    /// recompute; defaults flip none. `unresolved_involved_threads` is the
    /// number of unresolved threads with a viewer-authored comment to seed -
    /// each one drives ADR-0016's query-time involvement test.
    fn seed(
        db: &DbHandle,
        author_login: &str,
        pr_updated_at: i64,
        unresolved_involved_threads: i64,
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
                 created_at, updated_at, base_ref, head_ref, review_decision)
                VALUES (100, 10, 1, 't', 'open', 0, '{author_login}',
                        0, {pr_updated_at}, 'main', 'feat',
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
        for i in 0..unresolved_involved_threads {
            let thread_id = 5000 + i;
            let comment_id = 6000 + i;
            conn.execute_batch(&format!(
                "INSERT INTO review_threads
                    (id, pull_request_id, is_resolved, is_outdated, node_id)
                    VALUES ({thread_id}, 100, 0, 0, 'RT_seed_{i}');
                 INSERT INTO review_comments
                    (id, review_thread_id, author_login, body, created_at)
                    VALUES ({comment_id}, {thread_id}, 'alice', 'note', 1);"
            ))
            .unwrap();
        }
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
        // Resolve every seeded thread so the recompute's signal-1 EXISTS
        // misses and the only-thread-driven attention can clear.
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "UPDATE review_threads SET is_resolved = 1 WHERE pull_request_id = 100",
                [],
            )
            .unwrap();
        }
        invoke_mark_pr_unread(&db, 100, 1).unwrap();
        let (_, _, _, after) = read_triage(&db);
        assert_eq!(after, 0, "no signals left after thread clears");
    }
}
