//! Tauri command surface for the conversation module.

use tauri::State;

use crate::conversation::types::{ConversationStats, HydratedConversation, PullRequestThread};
use crate::db::DbHandle;

/// List per-thread state for a PR. Reads from the local cache only; no
/// network round-trip. Returns the latest sync-cycle snapshot.
///
/// Wave 2-B implements the body. The command is registered now so the
/// TypeScript bindings see it during M3 frontend work.
#[tauri::command]
pub fn list_pr_threads(
    pull_request_id: i64,
    _db: State<'_, DbHandle>,
) -> Result<Vec<PullRequestThread>, String> {
    let _ = pull_request_id;
    Err("list_pr_threads not implemented (M3-B)".into())
}

/// Compute conversation stats for a PR from the local cache.
///
/// Wave 2-B implements the body.
#[tauri::command]
pub fn get_pr_conversation_stats(
    pull_request_id: i64,
    _db: State<'_, DbHandle>,
) -> Result<ConversationStats, String> {
    let _ = pull_request_id;
    Err("get_pr_conversation_stats not implemented (M3-B)".into())
}

/// Lazy hydration: fetch full thread replies + issue-comment bodies from
/// GitHub, persist them, return the hydrated DTO. Called when the drawer /
/// route mounts.
///
/// Idempotent — subsequent calls within the same cache window re-render
/// from SQLite without a new network round-trip when the underlying
/// `pull_requests.updated_at` is unchanged.
///
/// Wave 2-B implements the body. The signature may grow additional state
/// parameters (`ClientFactory`, `AccountStore`) when the GitHub round-trip
/// is wired; the typed contract documented in
/// `docs/contracts/conversation-depth.md` is the source of truth.
#[tauri::command]
pub async fn fetch_pr_conversation(
    pull_request_id: i64,
    _db: State<'_, DbHandle>,
) -> Result<HydratedConversation, String> {
    let _ = pull_request_id;
    Err("fetch_pr_conversation not implemented (M3-B)".into())
}
