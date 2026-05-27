//! Tauri event payloads emitted by the sync worker.
//!
//! Event names are stable strings so the frontend can subscribe without
//! reaching into Rust types. Payload structs are `Serialize` only —
//! deserialisation happens in TypeScript.

use serde::Serialize;

use crate::github::AccountId;
use crate::sync::state::AccountSyncState;

/// Emitted whenever an account's phase, last-synced timestamp, or next-sync
/// timer changes. The full state is sent each time so the frontend store
/// stays consistent without needing to merge partial updates.
pub const SYNC_STATUS_EVENT: &str = "sync://status";

/// Emitted after any write that should cause the dashboard / conversation
/// surfaces to re-read from SQLite: end of a successful sync cycle (ADR 0029),
/// and after each successful triage command (mark-read, archive,
/// unarchive). The frontend stores listen on this single event; the payload
/// is empty because the surfaces re-query their own scope.
pub const DASHBOARD_REFRESH_EVENT: &str = "dashboard://refresh";

/// Emitted alongside `sync://status` when the cycle failed transiently. The
/// payload mirrors the status event but carries a short human-readable
/// message field. Kept separate from status so a toast layer can hook just
/// the errors without filtering every status tick.
pub const SYNC_ERROR_EVENT: &str = "sync://error";

/// Emitted when an account's rate budget falls below the 20% guard. The
/// worker stops scheduling cycles for that account until the budget recovers.
pub const SYNC_RATE_LIMIT_EVENT: &str = "sync://rate-limit-warning";

/// Emitted on every push to the in-memory activity buffer. Payload is one
/// `ActivityEvent` (see `sync::activity`). Additive alongside the existing
/// status / error / rate-limit events; the activity panel subscribes here
/// for live updates and uses `list_recent_activity` to hydrate on startup.
pub const SYNC_ACTIVITY_EVENT: &str = "sync://activity";

/// Full status payload. Mirror of [`AccountSyncState`] so subscribers in the
/// frontend get one shape they can store directly.
#[derive(Debug, Clone, Serialize)]
pub struct SyncStatusPayload {
    pub account_id: AccountId,
    #[serde(flatten)]
    pub state: AccountSyncState,
}

impl SyncStatusPayload {
    pub fn new(state: AccountSyncState) -> Self {
        Self {
            account_id: state.account_id,
            state,
        }
    }
}

/// Error payload. `message` is short and safe to render directly. Internal
/// detail (network errors, GraphQL fault paths) is logged, not surfaced here.
#[derive(Debug, Clone, Serialize)]
pub struct SyncErrorPayload {
    pub account_id: AccountId,
    pub message: String,
}

/// Rate-limit warning payload. `rate_remaining_pct` is the budget remaining
/// as 0-100; the guard fires when this falls below 20.
///
/// `resource` carries the GitHub bucket whose budget tripped the guard
/// (`core`, `search`, or `graphql`). The status-bar surface uses it to render
/// "search budget low" instead of the generic "rate limited" - the issue's
/// motivating case is a single sub-budget bottoming out while the others
/// still have headroom.
#[derive(Debug, Clone, Serialize)]
pub struct SyncRateLimitPayload {
    pub account_id: AccountId,
    pub rate_remaining_pct: u8,
    pub limit: Option<i64>,
    pub reset_in_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}
