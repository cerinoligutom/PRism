//! Tauri commands for sync control.
//!
//! `get_sync_status` is a read-only snapshot of the worker's per-account
//! state. `refresh_now` nudges one (or every) account to run a cycle
//! immediately. Setting the poll interval is exposed for the Settings view.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::github::AccountId;
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
pub fn refresh_now(worker: State<'_, Arc<WorkerHandle>>, input: RefreshNowInput) -> RefreshNowResult {
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
/// the result echoes the value actually applied.
#[tauri::command]
pub fn set_sync_interval(
    worker: State<'_, Arc<WorkerHandle>>,
    input: SetIntervalInput,
) -> SetIntervalResult {
    worker.config().set_interval(input.seconds);
    SetIntervalResult {
        applied_seconds: worker.config().interval_secs(),
    }
}
