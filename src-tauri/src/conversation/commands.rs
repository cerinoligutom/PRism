//! Tauri command surface for the conversation module.
//!
//! ADR 0029 moved conversation persistence (`review_comments`,
//! `issue_comments`) entirely under the sync worker. The commands here are
//! cache readers: synchronous DB queries that return whatever the most recent
//! sync cycle wrote. See `docs/contracts/conversation-depth.md` for the shape.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Runtime, State};
use thiserror::Error;

use crate::conversation::query;
use crate::conversation::types::{
    ConversationStats, HydratedConversation, PullRequestThread, TimelineEventRecord,
};
use crate::db::DbHandle;
use crate::notify::refresh_from_db as refresh_badge_from_db;

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
/// this command is a synchronous cache reader. It also runs the
/// auto-mark-on-open side effect (`read_at` flip + badge refresh) so the
/// drawer behaviour matches what the prior lazy hydrator did at this moment in
/// the open flow.
#[tauri::command]
pub fn load_pr_conversation<R: Runtime>(
    pull_request_id: i64,
    db: State<'_, DbHandle>,
    app_handle: AppHandle<R>,
) -> Result<HydratedConversation, ConversationCommandError> {
    let account_id = resolve_repo_owning_account(&db, pull_request_id)?;
    auto_mark_read(&db, pull_request_id, account_id);
    // ADR 0017 decision 3: the auto-mark-on-open flips `read_at` and zeroes
    // `mentioned_count_unread`, both of which can drop `needs_attention` to 0
    // for this row. Push the new global count to the dock so opening a drawer
    // immediately deflates the badge.
    refresh_badge_from_db(&app_handle, &db);
    hydrated_response(&db, pull_request_id, account_id)
}

/// Resolve the owning account of a PR's repo. The conversation surface uses
/// this to seed `auto_mark_read` and the involvement projection on
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

/// Best-effort auto-mark-on-open. Drives the same write path as
/// `triage::commands::mark_pr_read` but runs after the hydration
/// transaction commits so a failure can't unwind the cached payload.
/// Errors are logged and swallowed: a mark-read failure must never break
/// detail-surface hydration.
///
/// ADR 0016: in unified mode a PR can have relation rows under multiple
/// accounts. The auto-mark flips read state for every relation owner so the
/// merged dashboard row doesn't linger as unread under one of the in-scope
/// accounts after the open. The hydration `account_id` only drives the URL
/// host / client (the repo's owning account); the mark-read fan-out reads
/// the relation table directly.
fn auto_mark_read(db: &DbHandle, pull_request_id: i64, account_id: i64) {
    if mark_read_in_tx(db, pull_request_id, account_id).is_err() {
        // The `mark_read_in_tx` `internal()` helper already logged the
        // underlying failure; the outer site only needs to mark the run as
        // best-effort so the hydrated response still surfaces.
        tracing::warn!(
            pull_request_id,
            account_id,
            "auto-mark-on-open: best-effort failure",
        );
    }
}

fn mark_read_in_tx(
    db: &DbHandle,
    pull_request_id: i64,
    account_id: i64,
) -> Result<(), ConversationCommandError> {
    let mut conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    let tx = conn
        .transaction()
        .map_err(|e| internal(&format!("begin tx: {e}")))?;

    // The repo's owning account always gets a mark_read (UPSERTs the relation
    // row if missing - matches the existing "Team-view PR the viewer opens
    // for the first time" semantic). Every other relation owner gets a
    // per-account flip too.
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

    for owner in owners {
        // Per-account failures log + continue (ADR 0016: partial successes
        // must persist). The hydration transaction has already committed; the
        // mark-read fanout is best-effort from here.
        if let Err(e) = crate::triage::query::mark_read(&tx, owner, pull_request_id) {
            tracing::warn!(
                pull_request_id,
                account = owner,
                err = %e,
                "auto-mark-on-open mark_read failed",
            );
            continue;
        }
        // Auto-mark triggers (ADR 0017 decision 1) are intentionally not
        // dispatched from this code path. The drawer is currently open on
        // the PR, so a toast for "needs your attention" would point at a
        // surface the user is already viewing; the in-app badge already
        // tracks the rare 0 -> 1 flip the mark-read recompute can produce
        // (e.g. a fresh mention landed while the drawer was opening). A
        // future ADR can revisit if user feedback flags missed signals.
        match crate::triage::query::recompute_needs_attention(&tx, owner, pull_request_id, None) {
            Ok(_triggers) => {}
            Err(e) => tracing::warn!(
                pull_request_id,
                account = owner,
                err = %e,
                "auto-mark-on-open recompute failed",
            ),
        }
    }
    tx.commit()
        .map_err(|e| internal(&format!("commit tx: {e}")))?;
    Ok(())
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
