//! Shared sync state.
//!
//! The worker writes `AccountSyncState` rows; the `get_sync_status` Tauri
//! command reads them. The state map is `Arc<Mutex<_>>` so commands clone the
//! handle cheaply and the worker holds the lock for the duration of a single
//! field update only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::github::AccountId;

/// Phase of an account's sync loop. Drives the dot colour in the status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncPhase {
    /// Worker has registered the account but no cycle has started yet.
    Idle,
    /// A cycle is in flight.
    Syncing,
    /// The last cycle finished cleanly.
    Synced,
    /// The last cycle returned a transient error.
    Error,
    /// We hit a 401 and stopped polling this account until the UI re-auths.
    Unauthorized,
    /// The rate-limit budget is below the 20% guard threshold.
    RateLimited,
}

/// Per-account sync state. Cloneable for the wire (Tauri event payload).
#[derive(Debug, Clone, Serialize)]
pub struct AccountSyncState {
    pub account_id: AccountId,
    pub phase: SyncPhase,
    /// RFC-3339 timestamp of the last successful cycle, or `None` until the
    /// first one completes.
    pub last_synced_at: Option<String>,
    /// Seconds until the next scheduled cycle. `None` while a cycle is in
    /// flight or when polling is suspended (e.g. `Unauthorized`).
    pub next_sync_in_seconds: Option<u64>,
    /// Short, user-facing message attached to an error / rate-limit state.
    /// Internal error detail is logged, not surfaced here.
    pub message: Option<String>,
    /// Most-recent observed rate-budget percentage remaining (0-100). `None`
    /// until the first response is observed.
    pub rate_remaining_pct: Option<u8>,
    /// Most-recent observed `x-ratelimit-limit` value, mirrored so the UI can
    /// render "X% / 5000/hr" without a second command.
    pub rate_limit: Option<i64>,
}

impl AccountSyncState {
    pub fn new(account_id: AccountId) -> Self {
        Self {
            account_id,
            phase: SyncPhase::Idle,
            last_synced_at: None,
            next_sync_in_seconds: None,
            message: None,
            rate_remaining_pct: None,
            rate_limit: None,
        }
    }
}

/// Thread-safe map of `AccountId` -> `AccountSyncState`. Shared between the
/// worker (writer) and the Tauri commands (readers).
#[derive(Clone, Default)]
pub struct SyncStateMap {
    inner: Arc<Mutex<HashMap<AccountId, AccountSyncState>>>,
}

impl SyncStateMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot every account's state. Order is unspecified — callers sort
    /// client-side if they care.
    pub fn snapshot_all(&self) -> Vec<AccountSyncState> {
        match self.inner.lock() {
            Ok(guard) => guard.values().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn snapshot(&self, account_id: AccountId) -> Option<AccountSyncState> {
        self.inner.lock().ok()?.get(&account_id).cloned()
    }

    /// Apply a closure to the account's state, inserting an `Idle` baseline
    /// row if none exists yet. Returns the new state for emission.
    pub fn update<F: FnOnce(&mut AccountSyncState)>(
        &self,
        account_id: AccountId,
        update: F,
    ) -> AccountSyncState {
        let mut guard = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::error!(
                %account_id,
                "sync state map: recovered from poisoned mutex during update"
            );
            poisoned.into_inner()
        });
        let entry = guard
            .entry(account_id)
            .or_insert_with(|| AccountSyncState::new(account_id));
        update(entry);
        entry.clone()
    }

    /// Remove an account from the map (e.g. after `remove_account`).
    pub fn forget(&self, account_id: AccountId) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(&account_id);
        }
    }
}

/// Convert a `SystemTime` to an RFC-3339 string for serialisation.
///
/// We accept whatever string the underlying clock produces and tolerate
/// pre-epoch sentinels by returning `None`.
pub fn format_rfc3339(t: SystemTime) -> Option<String> {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    let secs = t.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    let odt = OffsetDateTime::from_unix_timestamp(secs.as_secs() as i64).ok()?;
    odt.format(&Rfc3339).ok()
}

/// Round a `Duration` to a whole-second count, never less than 1.
pub fn seconds_floor(d: Duration) -> u64 {
    d.as_secs().max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_inserts_baseline_then_mutates() {
        let map = SyncStateMap::new();
        let state = map.update(7, |s| {
            s.phase = SyncPhase::Syncing;
        });
        assert_eq!(state.account_id, 7);
        assert_eq!(state.phase, SyncPhase::Syncing);
    }

    #[test]
    fn snapshot_returns_none_for_unknown_account() {
        let map = SyncStateMap::new();
        assert!(map.snapshot(99).is_none());
    }

    #[test]
    fn forget_removes_state() {
        let map = SyncStateMap::new();
        map.update(1, |s| s.phase = SyncPhase::Synced);
        assert!(map.snapshot(1).is_some());
        map.forget(1);
        assert!(map.snapshot(1).is_none());
    }

    #[test]
    fn seconds_floor_clamps_below_one_second() {
        assert_eq!(seconds_floor(Duration::from_millis(0)), 1);
        assert_eq!(seconds_floor(Duration::from_millis(500)), 1);
        assert_eq!(seconds_floor(Duration::from_secs(7)), 7);
    }

    #[test]
    fn update_recovers_from_poisoned_mutex() {
        let map = SyncStateMap::new();
        map.update(7, |s| s.phase = SyncPhase::Synced);

        // Poison the mutex by panicking in a thread that holds the lock.
        // Silence the panic hook so the test output stays readable; the
        // join() return value confirms the panic still propagated.
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let inner = Arc::clone(&map.inner);
        let join = std::thread::spawn(move || {
            let _guard = inner.lock().unwrap();
            panic!("intentional poison for test");
        })
        .join();
        std::panic::set_hook(prev_hook);

        assert!(join.is_err(), "thread should have panicked");
        assert!(map.inner.is_poisoned(), "mutex should be poisoned");

        // The fix: update() recovers the poisoned guard, leaves prior data
        // intact, and applies the closure normally.
        let state = map.update(7, |s| s.phase = SyncPhase::Syncing);
        assert_eq!(state.account_id, 7);
        assert_eq!(state.phase, SyncPhase::Syncing);
    }
}
