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

/// Rate-limit warning payload. `pct` is the budget remaining as 0-100; the
/// guard fires when this falls below 20.
#[derive(Debug, Clone, Serialize)]
pub struct SyncRateLimitPayload {
    pub account_id: AccountId,
    pub pct: u8,
    pub limit: Option<i64>,
    pub reset_in_seconds: Option<u64>,
}
