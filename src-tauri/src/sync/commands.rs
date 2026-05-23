//! Tauri commands for sync control.
//!
//! `get_sync_status` is a read-only snapshot of the worker's per-account
//! state. `refresh_now` nudges one (or every) account to run a cycle
//! immediately. Setting the poll interval is exposed for the Settings view.
//! `list_recent_activity` returns the rolling diagnostic buffer that backs
//! the status-bar activity panel (issue #122).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::github::AccountId;
use crate::sync::activity::{snapshot, ActivityBuffer, ActivityEvent};
use crate::sync::scheduler::{MAX_INTERVAL_SECS, MIN_INTERVAL_SECS};
use crate::sync::state::AccountSyncState;
use crate::sync::worker::WorkerHandle;

#[derive(Debug, Clone, Serialize)]
pub struct SyncStatusSnapshot {
    pub accounts: Vec<AccountSyncState>,
    pub interval_seconds: u64,
    pub min_interval_seconds: u64,
    pub max_interval_seconds: u64,
}

#[tauri::command]
pub fn get_sync_status(worker: State<'_, Arc<WorkerHandle>>) -> SyncStatusSnapshot {
    SyncStatusSnapshot {
        accounts: worker.state().snapshot_all(),
        interval_seconds: worker.config().interval_secs(),
        min_interval_seconds: MIN_INTERVAL_SECS,
        max_interval_seconds: MAX_INTERVAL_SECS,
    }
}

#[derive(Debug, Deserialize)]
pub struct RefreshNowInput {
    /// `None` refreshes every tracked account.
    pub account_id: Option<AccountId>,
}

#[derive(Debug, Serialize)]
pub struct RefreshNowResult {
    /// Number of accounts nudged. Zero means no matching account is being
    /// tracked yet (e.g. it was added between login and now).
    pub triggered: usize,
}

#[tauri::command]
pub fn refresh_now(
    worker: State<'_, Arc<WorkerHandle>>,
    input: RefreshNowInput,
) -> RefreshNowResult {
    let triggered = match input.account_id {
        Some(id) => {
            if worker.refresh_account(id) {
                1
            } else {
                0
            }
        }
        None => worker.refresh_all(),
    };
    RefreshNowResult { triggered }
}

#[derive(Debug, Deserialize)]
pub struct SetIntervalInput {
    pub seconds: u64,
}

#[derive(Debug, Serialize)]
pub struct SetIntervalResult {
    pub applied_seconds: u64,
}

/// Update the poll interval. Out-of-range values clamp to the nearest bound;
/// the result echoes the value actually applied. The clamped value is also
/// persisted to `app_settings.sync_interval_seconds` so the choice survives
/// an app restart. A persistence error is logged but doesn't fail the
/// command - the in-memory runtime change still applies.
#[tauri::command]
pub fn set_sync_interval(
    worker: State<'_, Arc<WorkerHandle>>,
    db: State<'_, crate::db::DbHandle>,
    input: SetIntervalInput,
) -> SetIntervalResult {
    worker.config().set_interval(input.seconds);
    let applied = worker.config().interval_secs();
    if let Err(err) = crate::sync::write_persisted_interval(&db, applied) {
        eprintln!("sync: persist interval failed: {err}");
    }
    SetIntervalResult {
        applied_seconds: applied,
    }
}

/// Default page size returned by `list_recent_activity` when the caller
/// doesn't pass an explicit `limit`. Matches the panel's render budget.
pub const DEFAULT_ACTIVITY_LIMIT: usize = 100;

#[derive(Debug, Default, Deserialize)]
pub struct ListRecentActivityInput {
    /// Cap on returned events. Defaults to `DEFAULT_ACTIVITY_LIMIT`.
    pub limit: Option<usize>,
    /// When set, only events with this `account_id` (or no account at all,
    /// when matching the variant's own scope) are returned.
    pub account_id: Option<AccountId>,
}

/// Most-recent-first slice of the diagnostic activity buffer (issue #122).
///
/// The buffer lives in process memory and is bounded to `activity::BUFFER_CAP`.
/// Callers typically hydrate on store init and then subscribe to the
/// `sync://activity` Tauri event for live deltas.
#[tauri::command]
pub fn list_recent_activity(
    input: Option<ListRecentActivityInput>,
    buffer: State<'_, ActivityBuffer>,
) -> Vec<ActivityEvent> {
    let input = input.unwrap_or_default();
    let limit = input.limit.unwrap_or(DEFAULT_ACTIVITY_LIMIT);
    snapshot(&buffer, limit, input.account_id)
}
