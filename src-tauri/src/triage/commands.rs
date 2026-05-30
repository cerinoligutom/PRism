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
//!
//! M6 wave 1 adds `mark_pr_archived` / `mark_pr_unarchived` (ADR 0018). Both
//! commands fire [`DASHBOARD_REFRESH_EVENT`] on success so the frontend can
//! reload the affected views without waiting for the next sync tick.
//!
//! Issue #336 adds `mark_view_read` - the "Mark all read" command that
//! bulk-flips read state on every relation row matching the active view +
//! chip filter. Same per-row semantics as `mark_pr_read`, applied to the row
//! set the dashboard list would project.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};
use thiserror::Error;

use crate::dashboard::DashboardView;
use crate::db::DbHandle;
use crate::notify::{
    format_trigger, refresh_from_db as refresh_badge_from_db, NotificationSinkHandle,
    NotificationTrigger,
};
use crate::sync::DASHBOARD_REFRESH_EVENT;
use crate::triage::query;
use crate::triage::types::{ChipKey, FilterChipCounts, SidebarAttentionCounts};

/// User-facing error shape for `triage::*` commands. Internal failures (lock
/// poison, rusqlite errors mid-transaction) fold into a single opaque variant
/// so internals never leak to the renderer (CLAUDE.md security rule).
#[derive(Debug, Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriageCommandError {
    #[error("an unexpected error occurred")]
    Internal,
}

fn internal(message: &str) -> TriageCommandError {
    tracing::error!(message, "triage command internal error");
    TriageCommandError::Internal
}

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
pub fn mark_pr_read<R: Runtime>(
    pull_request_id: i64,
    account_id: Option<i64>,
    db: State<'_, DbHandle>,
    notify_sink: State<'_, NotificationSinkHandle>,
    app_handle: AppHandle<R>,
) -> Result<(), TriageCommandError> {
    let mut conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    let tx = conn
        .transaction()
        .map_err(|e| internal(&format!("begin tx: {e}")))?;
    let mut triggers: Vec<NotificationTrigger> = Vec::new();
    match account_id {
        Some(id) => {
            query::mark_read(&tx, id, pull_request_id)
                .map_err(|e| internal(&format!("mark read: {e}")))?;
            // mark_read zeroed the counter, so any Mention transition is a
            // clear (5 -> 0), not an increase. Pass `None` to disable Mention
            // detection - clearing reads doesn't surface as a notification
            // (ADR 0017 decision 1).
            let new = query::recompute_needs_attention(&tx, id, pull_request_id, None)
                .map_err(|e| internal(&format!("recompute needs_attention: {e}")))?;
            triggers.extend(new);
        }
        None => {
            apply_to_all_relation_owners(&tx, pull_request_id, |tx, acct| {
                query::mark_read(tx, acct, pull_request_id)?;
                let new = query::recompute_needs_attention(tx, acct, pull_request_id, None)?;
                triggers.extend(new);
                Ok(())
            })
            .map_err(|e| internal(&format!("mark read multi: {e}")))?;
        }
    }
    tx.commit()
        .map_err(|e| internal(&format!("commit tx: {e}")))?;
    dispatch_triggers(&conn, notify_sink.inner(), &triggers);
    drop(conn);
    refresh_badge_from_db(&app_handle, &db);
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
pub fn mark_pr_unread<R: Runtime>(
    pull_request_id: i64,
    account_id: Option<i64>,
    db: State<'_, DbHandle>,
    notify_sink: State<'_, NotificationSinkHandle>,
    app_handle: AppHandle<R>,
) -> Result<(), TriageCommandError> {
    let mut conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    let tx = conn
        .transaction()
        .map_err(|e| internal(&format!("begin tx: {e}")))?;
    let mut triggers: Vec<NotificationTrigger> = Vec::new();
    match account_id {
        Some(id) => {
            query::mark_unread(&tx, id, pull_request_id)
                .map_err(|e| internal(&format!("mark unread: {e}")))?;
            // mark_unread leaves `mentioned_count_unread` untouched, so the
            // pre-call counter equals the post-UPDATE value. No Mention
            // transition is possible from this path.
            let new = query::recompute_needs_attention(&tx, id, pull_request_id, None)
                .map_err(|e| internal(&format!("recompute needs_attention: {e}")))?;
            triggers.extend(new);
        }
        None => {
            apply_to_all_relation_owners(&tx, pull_request_id, |tx, acct| {
                query::mark_unread(tx, acct, pull_request_id)?;
                let new = query::recompute_needs_attention(tx, acct, pull_request_id, None)?;
                triggers.extend(new);
                Ok(())
            })
            .map_err(|e| internal(&format!("mark unread multi: {e}")))?;
        }
    }
    tx.commit()
        .map_err(|e| internal(&format!("commit tx: {e}")))?;
    dispatch_triggers(&conn, notify_sink.inner(), &triggers);
    drop(conn);
    refresh_badge_from_db(&app_handle, &db);
    Ok(())
}

/// Manual archive write for one `(account_id, pull_request_id)` pair.
/// ADR 0018 keeps manual + auto archive on the same `archived_at` column;
/// this command is the manual writer the row overflow menu invokes. The
/// frontend supplies a single `account_id` per call - in unified scope it
/// fans out across every relation owner the viewer holds (one invoke per
/// account), mirroring the mark-read fan-out from ADR 0016.
///
/// UPSERTs the relation row so an account whose viewer hasn't opened the
/// drawer can still archive the PR. Wraps the write in a transaction even
/// though the underlying UPSERT is a single statement so the future addition
/// of a recompute / cascade follow-up doesn't break the atomicity contract
/// established by `mark_pr_read`. Emits [`DASHBOARD_REFRESH_EVENT`] on
/// success so the frontend reloads without waiting for the next sync tick.
#[tauri::command]
pub fn mark_pr_archived<R: Runtime>(
    pull_request_id: i64,
    account_id: i64,
    db: State<'_, DbHandle>,
    app_handle: AppHandle<R>,
) -> Result<(), TriageCommandError> {
    let mut conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    let tx = conn
        .transaction()
        .map_err(|e| internal(&format!("begin tx: {e}")))?;
    query::mark_archived(&tx, account_id, pull_request_id)
        .map_err(|e| internal(&format!("mark archived: {e}")))?;
    tx.commit()
        .map_err(|e| internal(&format!("commit tx: {e}")))?;
    drop(conn);
    emit_dashboard_refresh(&app_handle);
    refresh_badge_from_db(&app_handle, &db);
    Ok(())
}

/// Batched archive write for a set of PRs against one `account_id`. Backs
/// the dashboard's bulk multi-select archive flow (#331). Mirrors
/// [`mark_pr_archived`]'s per-account semantics so the frontend keeps its
/// fan-out shape: one invoke per account, each batching the subset of PR
/// ids that account holds a relation to. Empty `pull_request_ids` is a
/// no-op.
///
/// Wraps the write in a transaction to keep parity with the single-pair
/// command; the underlying `query::mark_prs_archived` is one prepared
/// `INSERT ... ON CONFLICT` so a future cascade addition doesn't break the
/// atomicity contract. Emits [`DASHBOARD_REFRESH_EVENT`] on success so the
/// frontend reloads without waiting for the next sync tick.
#[tauri::command]
pub fn mark_prs_archived<R: Runtime>(
    pull_request_ids: Vec<i64>,
    account_id: i64,
    db: State<'_, DbHandle>,
    app_handle: AppHandle<R>,
) -> Result<(), TriageCommandError> {
    if pull_request_ids.is_empty() {
        return Ok(());
    }
    let mut conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    let tx = conn
        .transaction()
        .map_err(|e| internal(&format!("begin tx: {e}")))?;
    query::mark_prs_archived(&tx, account_id, &pull_request_ids)
        .map_err(|e| internal(&format!("mark prs archived: {e}")))?;
    tx.commit()
        .map_err(|e| internal(&format!("commit tx: {e}")))?;
    drop(conn);
    emit_dashboard_refresh(&app_handle);
    refresh_badge_from_db(&app_handle, &db);
    Ok(())
}

/// Reverse of [`mark_pr_archived`]: clear `archived_at` so the PR
/// reappears in the default views. UPSERTs the row the same way so an
/// Archive-view unarchive against a PR the viewer never opened works
/// without a sync round-trip first. Per ADR 0018 the same column
/// services both manual and auto-archive paths, so the unarchive write
/// is symmetric.
#[tauri::command]
pub fn mark_pr_unarchived<R: Runtime>(
    pull_request_id: i64,
    account_id: i64,
    db: State<'_, DbHandle>,
    app_handle: AppHandle<R>,
) -> Result<(), TriageCommandError> {
    let mut conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    let tx = conn
        .transaction()
        .map_err(|e| internal(&format!("begin tx: {e}")))?;
    query::mark_unarchived(&tx, account_id, pull_request_id)
        .map_err(|e| internal(&format!("mark unarchived: {e}")))?;
    tx.commit()
        .map_err(|e| internal(&format!("commit tx: {e}")))?;
    drop(conn);
    emit_dashboard_refresh(&app_handle);
    refresh_badge_from_db(&app_handle, &db);
    Ok(())
}

/// Mark every PR in the active view as read.
///
/// Issue #336: bulk read-flip for every relation row matching the dashboard
/// view + active chip filter. The write set is the same one the dashboard list
/// projects, so a user clicking "Mark all read" sees every visible row's dot
/// settle in one round-trip.
///
/// `account_id = Some(id)` flips only the active account's relation rows.
/// `account_id = None` (ADR 0016 unified mode) flips every relation row for
/// every PR the view + chips admit. Tracked-view PRs the active account has
/// never opened (no relation row) are not touched - bulk mark-read doesn't
/// UPSERT, since flipping read on a PR the user has never engaged with would
/// create relation rows the sync cycle hasn't validated.
///
/// Returns the number of distinct PRs whose read state the call touched.
/// Mirrors the user-visible "N marked" report the frontend renders.
///
/// Recomputes `needs_attention` for the same row set inside the same
/// transaction. Per ADR 0017 decision 1, read clears don't surface as OS
/// notifications, so this command emits no notification triggers.
#[tauri::command]
pub fn mark_view_read<R: Runtime>(
    view: DashboardView,
    account_id: Option<i64>,
    chips: Vec<ChipKey>,
    db: State<'_, DbHandle>,
    app_handle: AppHandle<R>,
) -> Result<i64, TriageCommandError> {
    let mut conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    let tx = conn
        .transaction()
        .map_err(|e| internal(&format!("begin tx: {e}")))?;
    let count = query::mark_view_read(&tx, view, account_id, &chips)
        .map_err(|e| internal(&format!("mark view read: {e}")))?;
    tx.commit()
        .map_err(|e| internal(&format!("commit tx: {e}")))?;
    drop(conn);
    refresh_badge_from_db(&app_handle, &db);
    Ok(count)
}

/// Fire-and-forget refresh signal. A failed emit logs and continues - the
/// command's write already succeeded, and the frontend can recover via the
/// next sync-cycle reload.
fn emit_dashboard_refresh<R: Runtime>(app: &AppHandle<R>) {
    if let Err(err) = app.emit(DASHBOARD_REFRESH_EVENT, ()) {
        tracing::warn!(event = DASHBOARD_REFRESH_EVENT, %err, "failed to emit refresh event");
    }
}

/// Format any triggers produced by the recompute pair and hand them to the
/// notification sink one at a time. Runs after the enclosing transaction
/// commits so a plugin failure can't roll back the read-state write, and the
/// connection is borrowed for the formatter lookup only - the sink owns
/// gating + permission state via its own internal locks (ADR 0017 decision
/// 5). A formatter miss (PR row vanished between the commit and the
/// dispatch) is logged and skipped; the in-app badge picks up the slack.
fn dispatch_triggers(
    conn: &rusqlite::Connection,
    sink: &NotificationSinkHandle,
    triggers: &[NotificationTrigger],
) {
    for trigger in triggers {
        match format_trigger(conn, trigger) {
            Some(notification) => sink.dispatch(&notification),
            None => tracing::debug!(
                account_id = trigger.account_id,
                pull_request_id = trigger.pull_request_id,
                "notify: skipping dispatch, PR row missing",
            ),
        }
    }
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
            tracing::warn!(
                pull_request_id,
                account_id,
                %err,
                "per-account triage write failed",
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
///
/// `account_id = Some(id)` keeps the single-account behaviour byte-identical
/// to before ADR 0016. `account_id = None` (the unified default) fans the
/// counts across every tracked account and dedupes by PR id so a PR matched
/// via two accounts contributes one to each chip it triggers - mirroring the
/// dashboard query's union-mode `GROUP BY pr.id` row shape.
#[tauri::command]
pub fn list_filter_chip_counts(
    view: DashboardView,
    account_id: Option<i64>,
    db: State<'_, DbHandle>,
) -> Result<FilterChipCounts, TriageCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    query::list_filter_chip_counts(&conn, view, account_id)
        .map_err(|e| internal(&format!("list_filter_chip_counts: {e}")))
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
) -> Result<SidebarAttentionCounts, TriageCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    query::count_sidebar_attention(&conn, account_id)
        .map_err(|e| internal(&format!("list_sidebar_attention_counts: {e}")))
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
    /// every test below. `author_login` controls which role obligations fire on
    /// recompute; defaults flip none. `threads_needing_me` is the number of
    /// unresolved threads to seed that need the viewer under the ADR 0031
    /// roll-up: each carries a viewer comment at t=1 (involvement) plus a later
    /// other-authored reply at t=2 (past the engagement watermark), so the (A)
    /// branch fires.
    fn seed(
        db: &DbHandle,
        author_login: &str,
        pr_updated_at: i64,
        threads_needing_me: i64,
        review_decision: Option<&str>,
    ) {
        let conn = db.lock().unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref, review_decision)
                VALUES (100, 10, 1, 't', 'open', 0, '{author_login}',
                        0, {pr_updated_at}, 'main', 'feat',
                        {review_decision_sql});
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at)
                VALUES (1, 100, 0, 0, 0, 0);",
            review_decision_sql = match review_decision {
                Some(s) => format!("'{s}'"),
                None => "NULL".to_string(),
            }
        ))
        .unwrap();
        for i in 0..threads_needing_me {
            let thread_id = 5000 + i;
            let own_comment_id = 6000 + i;
            let other_comment_id = 6500 + i;
            conn.execute_batch(&format!(
                "INSERT INTO review_threads
                    (id, pull_request_id, is_resolved, is_outdated, node_id)
                    VALUES ({thread_id}, 100, 0, 0, 'RT_seed_{i}');
                 INSERT INTO review_comments
                    (id, review_thread_id, author_login, body, created_at) VALUES
                    ({own_comment_id},   {thread_id}, 'alice', 'note',  1),
                    ({other_comment_id}, {thread_id}, 'bob',   'reply', 2);"
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
        query::recompute_needs_attention(&tx, account, pr, None)
            .map_err(|e| format!("recompute needs_attention: {e}"))?;
        tx.commit().map_err(|e| format!("commit tx: {e}"))?;
        Ok(())
    }

    /// Mirrors [`super::mark_pr_unread`].
    fn invoke_mark_pr_unread(db: &DbHandle, pr: i64, account: i64) -> Result<(), String> {
        let mut conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
        let tx = conn.transaction().map_err(|e| format!("begin tx: {e}"))?;
        query::mark_unread(&tx, account, pr).map_err(|e| format!("mark unread: {e}"))?;
        query::recompute_needs_attention(&tx, account, pr, None)
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
        // Two threads with a fresh other-authored reply keep the roll-up's (A)
        // branch firing even after the read flip - the legacy mention counter
        // is zeroed, but the conversation watermark still says the threads need
        // me (ADR 0031).
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
                    (id, repo_id, number, title, state, is_draft, author_login,
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
        // After mark_pr_read the (A) branch keeps needs_attention = 1 (the
        // threads still carry a fresh other-authored reply).
        let (_, _, _, before) = read_triage(&db);
        assert_eq!(before, 1);
        // Delete the other-authored replies so no comment past my watermark
        // remains; the units settle and the recompute clears the roll-up.
        // (Resolving alone would NOT clear it - a resolved thread with a fresh
        // reply still nags under ADR 0031.)
        {
            let conn = db.lock().unwrap();
            conn.execute("DELETE FROM review_comments WHERE author_login = 'bob'", [])
                .unwrap();
        }
        invoke_mark_pr_unread(&db, 100, 1).unwrap();
        let (_, _, _, after) = read_triage(&db);
        assert_eq!(after, 0, "no unit needs me after the other replies clear");
    }

    // ===== archive (M6 wave 1) =====

    /// Mirrors the body of [`super::mark_pr_archived`] minus the AppHandle
    /// emit. The Tauri runtime can't be booted from a unit test - the
    /// emit-path lives in a separate helper that's verified by integration
    /// tests against a real `AppHandle`. The DB write is what matters here.
    fn invoke_mark_pr_archived(db: &DbHandle, pr: i64, account: i64) -> Result<(), String> {
        let mut conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
        let tx = conn.transaction().map_err(|e| format!("begin tx: {e}"))?;
        query::mark_archived(&tx, account, pr).map_err(|e| format!("mark archived: {e}"))?;
        tx.commit().map_err(|e| format!("commit tx: {e}"))?;
        Ok(())
    }

    fn invoke_mark_pr_unarchived(db: &DbHandle, pr: i64, account: i64) -> Result<(), String> {
        let mut conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
        let tx = conn.transaction().map_err(|e| format!("begin tx: {e}"))?;
        query::mark_unarchived(&tx, account, pr).map_err(|e| format!("mark unarchived: {e}"))?;
        tx.commit().map_err(|e| format!("commit tx: {e}"))?;
        Ok(())
    }

    fn read_archived_at(db: &DbHandle) -> Option<i64> {
        let conn = db.lock().unwrap();
        conn.query_row(
            "SELECT archived_at FROM pull_request_viewer_relations
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    }

    #[test]
    fn mark_pr_archived_via_command_sets_archived_at() {
        let db = fresh_db();
        seed(&db, "bob", 1_700_000_000, 0, None);
        invoke_mark_pr_archived(&db, 100, 1).unwrap();
        assert!(read_archived_at(&db).is_some());
    }

    #[test]
    fn mark_pr_archived_via_command_preserves_read_state_and_attention() {
        // Set up a row with active read-state and needs_attention, then
        // archive. The archive write must leave those alone.
        let db = fresh_db();
        seed(&db, "alice", 1_700_000_000, 2, None);
        invoke_mark_pr_read(&db, 100, 1).unwrap();
        // After mark_pr_read, the relation has read_at set, mentions = 0,
        // and the (A) branch keeps needs_attention = 1 (the seeded threads
        // still carry a fresh other-authored reply).
        let (read_at_before, _, _, attention_before) = read_triage(&db);
        assert!(read_at_before.is_some());
        assert_eq!(attention_before, 1);

        invoke_mark_pr_archived(&db, 100, 1).unwrap();

        let (read_at_after, _, mentions_after, attention_after) = read_triage(&db);
        assert_eq!(
            read_at_after, read_at_before,
            "archive write must not touch read_at"
        );
        assert_eq!(mentions_after, 0);
        assert_eq!(
            attention_after, attention_before,
            "archive write must not touch needs_attention"
        );
        assert!(read_archived_at(&db).is_some());
    }

    #[test]
    fn mark_pr_unarchived_via_command_clears_archived_at() {
        let db = fresh_db();
        seed(&db, "bob", 1_700_000_000, 0, None);
        invoke_mark_pr_archived(&db, 100, 1).unwrap();
        assert!(read_archived_at(&db).is_some());

        invoke_mark_pr_unarchived(&db, 100, 1).unwrap();
        assert_eq!(read_archived_at(&db), None);
    }

    #[test]
    fn mark_pr_archived_via_command_upserts_when_relation_missing() {
        // Seed account + PR but no relation row, mirroring the "Team-view"
        // flow where the user reaches a PR before discovery created the row.
        let db = fresh_db();
        {
            let conn = db.lock().unwrap();
            conn.execute_batch(
                "INSERT INTO accounts (id, label, host, login, created_at)
                    VALUES (1, 'a', 'github.com', 'alice', 0);
                 INSERT INTO repos (id, account_id, owner, name, visibility)
                    VALUES (10, 1, 'owner', 'repo', 'public');
                 INSERT INTO pull_requests
                    (id, repo_id, number, title, state, is_draft, author_login,
                     created_at, updated_at, base_ref, head_ref)
                    VALUES (100, 10, 1, 't', 'open', 0, 'bob',
                            0, 1_700_000_000, 'main', 'feat');",
            )
            .unwrap();
        }
        invoke_mark_pr_archived(&db, 100, 1).unwrap();
        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn internal_variant_serialises_without_leaking_inner_message() {
        // CLAUDE.md security rule: internal failure detail must never reach
        // the renderer. The `Internal` variant carries no payload so the
        // serialised JSON only ever exposes its kind tag.
        let err = internal("rusqlite: table 'pull_requests' has no column named secret");
        let serialised = serde_json::to_string(&err).expect("serialise");
        assert_eq!(serialised, r#"{"kind":"internal"}"#);
        assert!(!serialised.contains("rusqlite"));
        assert!(!serialised.contains("secret"));
    }
}
