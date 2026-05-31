//! Tauri command surface for the conversation module.
//!
//! ADR 0029 moved conversation persistence (`review_comments`,
//! `issue_comments`) entirely under the sync worker. The commands here are
//! cache readers: synchronous DB queries that return whatever the most recent
//! sync cycle wrote. See `docs/contracts/conversation-depth.md` for the shape.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};
use thiserror::Error;

use crate::conversation::query;
use crate::conversation::types::{
    ConversationStats, HydratedConversation, PullRequestThread, TimelineEventRecord,
};
use crate::db::DbHandle;
use crate::notify::refresh_from_db as refresh_badge_from_db;
use crate::sync::DASHBOARD_REFRESH_EVENT;

/// User-facing error shape for `conversation::*` commands. Internal failures
/// (lock poison, rusqlite errors) fold into a single opaque variant so
/// internals never leak to the renderer (CLAUDE.md security rule).
/// `NotFound` surfaces when the PR the renderer asked for has no resolvable
/// row - the conversation drawer needs a distinct signal to decide whether to
/// retry or fall through to an empty state.
#[derive(Debug, Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationCommandError {
    #[error("pull request or account not found")]
    NotFound,
    #[error("an unexpected error occurred")]
    Internal,
}

fn internal(message: &str) -> ConversationCommandError {
    tracing::error!(message, "conversation command internal error");
    ConversationCommandError::Internal
}

/// List per-thread state for a PR. Reads from the local cache only; no network
/// round-trip. The optional `account_id` resolves the `is_involved` marker.
#[tauri::command]
pub fn list_pr_threads(
    pull_request_id: i64,
    account_id: Option<i64>,
    db: State<'_, DbHandle>,
) -> Result<Vec<PullRequestThread>, ConversationCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    query::list_pr_threads(&conn, pull_request_id, account_id)
        .map_err(|e| internal(&format!("list_pr_threads: {e}")))
}

/// Compute conversation stats for a PR from the local cache.
#[tauri::command]
pub fn get_pr_conversation_stats(
    pull_request_id: i64,
    db: State<'_, DbHandle>,
) -> Result<ConversationStats, ConversationCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    query::get_conversation_stats(&conn, pull_request_id)
        .map_err(|e| internal(&format!("get_pr_conversation_stats: {e}")))
}

/// List the persisted timeline events for a PR. Reads from the local cache
/// only; no network round-trip. The events are populated by the sync worker
/// each cycle (wipe-and-rewrite) so the list always reflects the latest
/// upstream history at the granularity of the qualifying-event set defined in
/// ADR 0007.
#[tauri::command]
pub fn list_pr_timeline_events(
    pull_request_id: i64,
    db: State<'_, DbHandle>,
) -> Result<Vec<TimelineEventRecord>, ConversationCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    query::list_pr_timeline_events(&conn, pull_request_id)
        .map_err(|e| internal(&format!("list_pr_timeline_events: {e}")))
}

/// Load the cached conversation for a PR.
///
/// ADR 0029: sync owns `review_comments` / `issue_comments` persistence, so
/// this command is a synchronous cache reader. ADR 0031 replaces the old
/// PR-level auto-mark-read with per-unit "seen": on open it advances the seen
/// watermark for every visible review thread (keyed on its `node_id`) and the
/// PR's general comment stream, recomputes the `needs_attention` roll-up, and
/// refreshes the dock badge so opening the drawer settles the units the user
/// just saw without the lose-track problem of clearing everything PR-wide.
#[tauri::command]
pub fn load_pr_conversation<R: Runtime>(
    pull_request_id: i64,
    db: State<'_, DbHandle>,
    app_handle: AppHandle<R>,
) -> Result<HydratedConversation, ConversationCommandError> {
    let account_id = resolve_repo_owning_account(&db, pull_request_id)?;
    auto_mark_units_seen(&db, pull_request_id, account_id);
    // The per-unit seen marks can drop `needs_attention` to 0 for this row;
    // push the new global count to the dock so opening a drawer immediately
    // deflates the badge.
    refresh_badge_from_db(&app_handle, &db);
    hydrated_response(&db, pull_request_id, account_id)
}

/// Explicitly mark one review thread seen for one account (ADR 0031). Backs a
/// later frontend "Mark seen" affordance: advances the per-thread seen
/// watermark (keyed on `node_id`), recomputes the roll-up, refreshes the
/// badge, and emits [`DASHBOARD_REFRESH_EVENT`] so the dashboard rows,
/// conversation drawer, and inbox chip reconcile without waiting for the next
/// sync tick. Re-arms on the next other-authored reply past the watermark.
#[tauri::command]
pub fn mark_thread_seen<R: Runtime>(
    pull_request_id: i64,
    account_id: i64,
    thread_node_id: String,
    db: State<'_, DbHandle>,
    app_handle: AppHandle<R>,
) -> Result<(), ConversationCommandError> {
    {
        let mut conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
        let tx = conn
            .transaction()
            .map_err(|e| internal(&format!("begin tx: {e}")))?;
        crate::triage::units::advance_thread_seen(&tx, account_id, &thread_node_id, unix_now())
            .map_err(|e| internal(&format!("advance_thread_seen: {e}")))?;
        crate::triage::query::recompute_needs_attention(&tx, account_id, pull_request_id)
            .map_err(|e| internal(&format!("recompute needs_attention: {e}")))?;
        tx.commit()
            .map_err(|e| internal(&format!("commit tx: {e}")))?;
    }
    emit_dashboard_refresh(&app_handle);
    refresh_badge_from_db(&app_handle, &db);
    Ok(())
}

/// Explicitly mark a PR's general comment stream seen for one account (ADR
/// 0031). Companion to [`mark_thread_seen`] for the general-stream unit. Emits
/// [`DASHBOARD_REFRESH_EVENT`] on commit for the same surface reconcile.
#[tauri::command]
pub fn mark_general_stream_seen<R: Runtime>(
    pull_request_id: i64,
    account_id: i64,
    db: State<'_, DbHandle>,
    app_handle: AppHandle<R>,
) -> Result<(), ConversationCommandError> {
    {
        let mut conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
        let tx = conn
            .transaction()
            .map_err(|e| internal(&format!("begin tx: {e}")))?;
        crate::triage::units::advance_general_stream_seen(
            &tx,
            account_id,
            pull_request_id,
            unix_now(),
        )
        .map_err(|e| internal(&format!("advance_general_stream_seen: {e}")))?;
        crate::triage::query::recompute_needs_attention(&tx, account_id, pull_request_id)
            .map_err(|e| internal(&format!("recompute needs_attention: {e}")))?;
        tx.commit()
            .map_err(|e| internal(&format!("commit tx: {e}")))?;
    }
    emit_dashboard_refresh(&app_handle);
    refresh_badge_from_db(&app_handle, &db);
    Ok(())
}

/// Fire-and-forget refresh signal. A failed emit logs and continues - the
/// command's write already succeeded, and the frontend can recover via the
/// next sync-cycle reload. Mirrors `triage::commands::emit_dashboard_refresh`.
fn emit_dashboard_refresh<R: Runtime>(app: &AppHandle<R>) {
    if let Err(err) = app.emit(DASHBOARD_REFRESH_EVENT, ()) {
        tracing::warn!(event = DASHBOARD_REFRESH_EVENT, %err, "failed to emit refresh event");
    }
}

/// Resolve the owning account of a PR's repo. The conversation surface uses
/// this to seed `auto_mark_units_seen` and the involvement projection on
/// `list_pr_threads`.
fn resolve_repo_owning_account(
    db: &DbHandle,
    pull_request_id: i64,
) -> Result<i64, ConversationCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    conn.query_row(
        "SELECT r.account_id
           FROM pull_requests pr
           JOIN repos r ON r.id = pr.repo_id
          WHERE pr.id = ?1",
        params![pull_request_id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map_err(|e| internal(&format!("resolve pr: {e}")))?
    .ok_or(ConversationCommandError::NotFound)
}

/// Best-effort per-unit "mark seen" on open (ADR 0031). Runs after the
/// hydration read so a failure can't unwind the cached payload. Errors are
/// logged and swallowed: a seen-write failure must never break detail-surface
/// hydration.
///
/// For every relation owner (ADR 0016: a PR can have relation rows under
/// multiple accounts in unified mode), advance the seen watermark for each
/// visible review thread (keyed on its `node_id`) and the PR's general comment
/// stream, then recompute the `needs_attention` roll-up. Marking a unit seen
/// re-arms it: a later other-authored comment past the watermark re-lights it.
fn auto_mark_units_seen(db: &DbHandle, pull_request_id: i64, account_id: i64) {
    if mark_units_seen_in_tx(db, pull_request_id, account_id).is_err() {
        // The `mark_units_seen_in_tx` `internal()` helper already logged the
        // underlying failure; the outer site only needs to mark the run as
        // best-effort so the hydrated response still surfaces.
        tracing::warn!(
            pull_request_id,
            account_id,
            "auto-mark-on-open: best-effort failure",
        );
    }
}

fn mark_units_seen_in_tx(
    db: &DbHandle,
    pull_request_id: i64,
    account_id: i64,
) -> Result<(), ConversationCommandError> {
    let mut conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    let tx = conn
        .transaction()
        .map_err(|e| internal(&format!("begin tx: {e}")))?;

    // Every visible thread's node_id. The conversation surface marks exactly
    // the units it loaded; a thread row with no node_id (legacy / pre-M3) has
    // no per-thread watermark to advance and is skipped.
    let node_ids: Vec<String> = {
        let mut stmt = tx
            .prepare(
                "SELECT node_id FROM review_threads
                  WHERE pull_request_id = ?1 AND node_id IS NOT NULL",
            )
            .map_err(|e| internal(&format!("prepare thread node_ids: {e}")))?;
        let rows = stmt
            .query_map([pull_request_id], |row| row.get::<_, String>(0))
            .map_err(|e| internal(&format!("query thread node_ids: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| internal(&format!("read thread node_id: {e}")))?);
        }
        out
    };

    // The repo's owning account always participates (UPSERTs nothing here -
    // the general-stream watermark advance is a no-op on a missing relation
    // row, matching the Team-view PR semantic). Every other relation owner
    // gets the same per-unit seen marks too.
    let mut owners: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    owners.insert(account_id);
    {
        let mut stmt = tx
            .prepare(
                "SELECT account_id FROM pull_request_viewer_relations
                  WHERE pull_request_id = ?1",
            )
            .map_err(|e| internal(&format!("prepare relation owners: {e}")))?;
        let rows = stmt
            .query_map([pull_request_id], |row| row.get::<_, i64>(0))
            .map_err(|e| internal(&format!("query relation owners: {e}")))?;
        for row in rows {
            let id = row.map_err(|e| internal(&format!("read relation owner: {e}")))?;
            owners.insert(id);
        }
    }

    let now = unix_now();
    for owner in owners {
        // Per-account failures log + continue (ADR 0016: partial successes
        // must persist). The seen-mark fanout is best-effort.
        for node_id in &node_ids {
            if let Err(e) = crate::triage::units::advance_thread_seen(&tx, owner, node_id, now) {
                tracing::warn!(
                    pull_request_id,
                    account = owner,
                    err = %e,
                    "auto-mark-on-open advance_thread_seen failed",
                );
            }
        }
        if let Err(e) =
            crate::triage::units::advance_general_stream_seen(&tx, owner, pull_request_id, now)
        {
            tracing::warn!(
                pull_request_id,
                account = owner,
                err = %e,
                "auto-mark-on-open advance_general_stream_seen failed",
            );
        }
        // Opening the PR also clears the PR-level "unread" axis (the bold-title
        // signal): advance the read watermark so the dashboard's unread
        // derivation reads false. This is distinct from the per-unit seen marks
        // above - the dot tracks "needs me", the bold title tracks "opened".
        if let Err(e) =
            crate::triage::units::advance_read_watermark(&tx, owner, pull_request_id, now)
        {
            tracing::warn!(
                pull_request_id,
                account = owner,
                err = %e,
                "auto-mark-on-open advance_read_watermark failed",
            );
        }
        // Recompute the roll-up so the row, badge, and sidebar reflect the
        // newly-seen units in the same transaction. ADR 0031: this path does
        // not dispatch toasts (the user is looking at the drawer); all
        // dispatch lives on the sync re-arm path.
        if let Err(e) = crate::triage::query::recompute_needs_attention(&tx, owner, pull_request_id)
        {
            tracing::warn!(
                pull_request_id,
                account = owner,
                err = %e,
                "auto-mark-on-open recompute failed",
            );
        }
    }
    tx.commit()
        .map_err(|e| internal(&format!("commit tx: {e}")))?;
    Ok(())
}

/// Current Unix epoch seconds. Used for the per-unit seen watermark on open.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn hydrated_response(
    db: &DbHandle,
    pull_request_id: i64,
    account_id: i64,
) -> Result<HydratedConversation, ConversationCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    build_hydrated(&conn, pull_request_id, Some(account_id))
        .map_err(|e| internal(&format!("hydrate response: {e}")))
}

/// Read the persisted state for a PR back into a `HydratedConversation`. Pulled
/// out so the hydrator and any future cache-only reader share the same shape.
fn build_hydrated(
    conn: &Connection,
    pull_request_id: i64,
    account_id: Option<i64>,
) -> Result<HydratedConversation, rusqlite::Error> {
    let threads = query::list_pr_threads(conn, pull_request_id, account_id)?;
    let thread_comments = query::list_thread_comments(conn, pull_request_id)?;
    let issue_comments = query::list_issue_comments(conn, pull_request_id)?;
    let reviews = query::list_reviews(conn, pull_request_id)?;
    let stats = query::get_conversation_stats(conn, pull_request_id)?;
    Ok(HydratedConversation {
        pull_request_id,
        threads,
        thread_comments,
        issue_comments,
        reviews,
        stats,
    })
}

/// Test-only helpers. Exposed to integration tests under `tests/` so they can
/// read the hydrated DTO without booting Tauri state.
#[doc(hidden)]
pub mod testing {
    use super::*;

    /// Rebuild a `HydratedConversation` from a connection, matching what the
    /// live command returns.
    pub fn build_hydrated(
        conn: &Connection,
        pull_request_id: i64,
        account_id: Option<i64>,
    ) -> Result<HydratedConversation, String> {
        super::build_hydrated(conn, pull_request_id, account_id).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    fn seed_two_unit_pr() -> DbHandle {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'gh', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 0, 'me', 0, 0, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at)
                VALUES (1, 100, 0);
             INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, node_id)
                VALUES (5001, 100, 0, 0, 'RT_one'),
                       (5002, 100, 0, 0, 'RT_two');
             INSERT INTO review_comments
                (id, review_thread_id, author_login, body, created_at) VALUES
                (6001, 5001, 'me',  'mine', 5),
                (6002, 5001, 'bob', 'r1',   20),
                (6003, 5002, 'me',  'mine', 5),
                (6004, 5002, 'bob', 'r2',   20);
             INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
                VALUES (7001, 100, 'bob', 'general', 20);",
        )
        .unwrap();
        Arc::new(Mutex::new(conn))
    }

    fn read_thread_seen(db: &DbHandle, node_id: &str) -> Option<i64> {
        db.lock()
            .unwrap()
            .query_row(
                "SELECT seen_at FROM thread_read_state
                  WHERE account_id = 1 AND review_thread_node_id = ?1",
                params![node_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .unwrap()
    }

    fn read_general_seen(db: &DbHandle) -> Option<i64> {
        db.lock()
            .unwrap()
            .query_row(
                "SELECT general_stream_seen_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |r| r.get::<_, Option<i64>>(0),
            )
            .unwrap()
    }

    fn read_pr_level_read_at(db: &DbHandle) -> Option<i64> {
        db.lock()
            .unwrap()
            .query_row(
                "SELECT read_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |r| r.get::<_, Option<i64>>(0),
            )
            .unwrap()
    }

    #[test]
    fn auto_mark_on_open_marks_every_visible_unit_seen() {
        // Opening a conversation advances the seen watermark for every visible
        // thread (keyed on node_id) and the general stream, recomputes the
        // roll-up to 0, and advances the PR-level read watermark so the unread
        // (bold-title) axis clears on open too (ADR 0031's two-axis encoding:
        // the dot tracks "needs me", the bold title tracks "opened").
        let db = seed_two_unit_pr();
        // Both threads + general are lit before open.
        {
            let conn = db.lock().unwrap();
            crate::triage::query::recompute_needs_attention(&conn, 1, 100).unwrap();
        }
        assert_eq!(
            db.lock()
                .unwrap()
                .query_row(
                    "SELECT needs_attention FROM pull_request_viewer_relations
                      WHERE account_id = 1 AND pull_request_id = 100",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1,
        );

        mark_units_seen_in_tx(&db, 100, 1).unwrap();

        assert!(
            read_thread_seen(&db, "RT_one").is_some(),
            "thread 1 marked seen"
        );
        assert!(
            read_thread_seen(&db, "RT_two").is_some(),
            "thread 2 marked seen"
        );
        assert!(
            read_general_seen(&db).is_some(),
            "general stream marked seen"
        );
        assert!(
            read_pr_level_read_at(&db).is_some(),
            "opening advances the PR read watermark so the unread bold-title axis clears on open"
        );
        assert_eq!(
            db.lock()
                .unwrap()
                .query_row(
                    "SELECT needs_attention FROM pull_request_viewer_relations
                      WHERE account_id = 1 AND pull_request_id = 100",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0,
            "every visible unit seen drops the roll-up to 0",
        );
    }

    #[test]
    fn auto_mark_on_open_is_max_only_does_not_regress() {
        // Marking seen twice (the second with an earlier clock) must not move
        // a watermark backwards.
        let db = seed_two_unit_pr();
        mark_units_seen_in_tx(&db, 100, 1).unwrap();
        let first = read_thread_seen(&db, "RT_one").unwrap();
        // Force a stale re-mark.
        {
            let conn = db.lock().unwrap();
            crate::triage::units::advance_thread_seen(&conn, 1, "RT_one", 1).unwrap();
        }
        assert_eq!(
            read_thread_seen(&db, "RT_one").unwrap(),
            first,
            "MAX-only: a stale re-mark must not regress the watermark"
        );
    }

    #[test]
    fn internal_variant_serialises_without_leaking_inner_message() {
        // CLAUDE.md security rule: internal failure detail must never reach
        // the renderer. The `Internal` variant carries no payload so the
        // serialised JSON only ever exposes its kind tag.
        let err = internal("graphql: { errors: [{ message: 'secret token revoked' }] }");
        let serialised = serde_json::to_string(&err).expect("serialise");
        assert_eq!(serialised, r#"{"kind":"internal"}"#);
        assert!(!serialised.contains("graphql"));
        assert!(!serialised.contains("secret"));
    }

    #[test]
    fn not_found_variant_serialises_to_kind_only() {
        let err = ConversationCommandError::NotFound;
        let serialised = serde_json::to_string(&err).expect("serialise");
        assert_eq!(serialised, r#"{"kind":"not_found"}"#);
    }
}
