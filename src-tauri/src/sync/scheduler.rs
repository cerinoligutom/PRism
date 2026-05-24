//! Sync polling scheduler configuration.
//!
//! Default interval and clamping live here. The worker reads `SchedulerConfig`
//! once per cycle so settings changes pick up on the next tick without
//! restarting the task.
//!
//! The current interval is persisted on the `app_settings` singleton so the
//! user's chosen cadence survives an app restart. Helpers below read/write
//! the column; the worker init in `lib.rs` reads on startup, the
//! `set_sync_interval` command writes on every change.
//!
//! `0` is a sentinel for Manual mode (issue #358): the per-account loop parks
//! until an explicit refresh nudge arrives. It mirrors `auto_archive_days = 0`
//! as the "off" value and bypasses the `[MIN, MAX]` clamp.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::db::{lock_db, DbHandle};

pub const DEFAULT_INTERVAL_SECS: u64 = 300;
pub const MIN_INTERVAL_SECS: u64 = 30;
pub const MAX_INTERVAL_SECS: u64 = 3600;

/// Manual-mode sentinel. Persisted as `sync_interval_seconds = 0` and parks
/// the per-account loop until an explicit refresh nudge fires.
pub const MANUAL_INTERVAL_SECS: u64 = 0;

/// Fraction of the rate budget that must remain before a cycle is allowed to
/// run. Mirrors PRD §8.2 / ADR 0004 ("under 20% of 5000/hr per account").
pub const RATE_BUDGET_GUARD_PCT: u8 = 20;

/// Atomic poll-interval container so the worker reads the latest value without
/// locking. The setter clamps positive values to `[MIN_INTERVAL_SECS,
/// MAX_INTERVAL_SECS]`; `0` passes through as the Manual sentinel.
#[derive(Debug)]
pub struct SchedulerConfig {
    interval_secs: AtomicU64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self::new(DEFAULT_INTERVAL_SECS)
    }
}

impl SchedulerConfig {
    pub fn new(interval_secs: u64) -> Self {
        Self {
            interval_secs: AtomicU64::new(clamp_interval_secs(interval_secs)),
        }
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.interval_secs.load(Ordering::Relaxed))
    }

    pub fn interval_secs(&self) -> u64 {
        self.interval_secs.load(Ordering::Relaxed)
    }

    /// `true` when the scheduler is in Manual mode (interval == 0). The
    /// per-account loop parks instead of ticking in this state.
    pub fn is_manual(&self) -> bool {
        self.interval_secs() == MANUAL_INTERVAL_SECS
    }

    /// Replace the interval. Positive values clamp to the nearest bound; `0`
    /// passes through and parks the per-account loop until an explicit
    /// refresh nudge fires.
    pub fn set_interval(&self, secs: u64) {
        self.interval_secs
            .store(clamp_interval_secs(secs), Ordering::Relaxed);
    }
}

/// Clamp a poll interval into a runnable value. Positive inputs clamp to
/// `[MIN_INTERVAL_SECS, MAX_INTERVAL_SECS]`; `0` is the Manual sentinel and
/// passes through unchanged.
pub fn clamp_interval_secs(secs: u64) -> u64 {
    if secs == MANUAL_INTERVAL_SECS {
        return MANUAL_INTERVAL_SECS;
    }
    secs.clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS)
}

/// Read the persisted poll interval from `app_settings`. Returns `None` if
/// the column read fails (DB locked, migration mid-flight, transient I/O) or
/// the value is negative (legacy "missing" marker); the caller should fall
/// back to `DEFAULT_INTERVAL_SECS`. `0` returns `Some(0)` — the Manual
/// sentinel. Positive values are re-clamped to `[MIN_INTERVAL_SECS,
/// MAX_INTERVAL_SECS]` in case the persisted value predates a range bump.
pub fn read_persisted_interval(db: &DbHandle) -> Option<u64> {
    let conn = match lock_db(db) {
        Ok(conn) => conn,
        Err(err) => {
            tracing::error!(%err, "sync: read persisted interval - db lock failed");
            return None;
        }
    };
    match conn.query_row(
        "SELECT sync_interval_seconds FROM app_settings WHERE id = 1",
        [],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(secs) if secs >= 0 => Some(clamp_interval_secs(secs as u64)),
        Ok(_) => None,
        Err(err) => {
            tracing::warn!(%err, "sync: read persisted interval - query failed");
            None
        }
    }
}

/// Write the (clamped) poll interval back to `app_settings`. Called from
/// the `set_sync_interval` Tauri command after the in-memory atomic has
/// been updated, so a write failure doesn't desync the runtime — the
/// runtime change still applies; only the persisted value is missed.
pub fn write_persisted_interval(db: &DbHandle, secs: u64) -> Result<(), rusqlite::Error> {
    let conn = lock_db(db)?;
    let clamped = clamp_interval_secs(secs) as i64;
    conn.execute(
        "UPDATE app_settings SET sync_interval_seconds = ?1, updated_at = strftime('%s', 'now') WHERE id = 1",
        [clamped],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_interval_matches_constant() {
        let cfg = SchedulerConfig::default();
        assert_eq!(cfg.interval_secs(), DEFAULT_INTERVAL_SECS);
    }

    #[test]
    fn interval_below_minimum_clamps_up() {
        let cfg = SchedulerConfig::new(5);
        assert_eq!(cfg.interval_secs(), MIN_INTERVAL_SECS);
    }

    #[test]
    fn interval_above_maximum_clamps_down() {
        let cfg = SchedulerConfig::new(9_999);
        assert_eq!(cfg.interval_secs(), MAX_INTERVAL_SECS);
    }

    #[test]
    fn set_interval_replaces_clamped_value() {
        let cfg = SchedulerConfig::default();
        cfg.set_interval(45);
        assert_eq!(cfg.interval_secs(), 45);
        cfg.set_interval(10_000);
        assert_eq!(cfg.interval_secs(), MAX_INTERVAL_SECS);
    }

    #[test]
    fn manual_sentinel_passes_through_clamp() {
        assert_eq!(clamp_interval_secs(0), 0);
    }

    #[test]
    fn clamp_floors_below_minimum() {
        assert_eq!(clamp_interval_secs(1), MIN_INTERVAL_SECS);
        assert_eq!(
            clamp_interval_secs(MIN_INTERVAL_SECS - 1),
            MIN_INTERVAL_SECS
        );
    }

    #[test]
    fn clamp_ceilings_above_maximum() {
        assert_eq!(clamp_interval_secs(7_200), MAX_INTERVAL_SECS);
        assert_eq!(clamp_interval_secs(u64::MAX), MAX_INTERVAL_SECS);
    }

    #[test]
    fn clamp_passes_through_in_range_values() {
        assert_eq!(clamp_interval_secs(60), 60);
        assert_eq!(clamp_interval_secs(1_800), 1_800);
        assert_eq!(clamp_interval_secs(3_600), 3_600);
    }

    #[test]
    fn is_manual_flips_on_zero() {
        let cfg = SchedulerConfig::default();
        assert!(!cfg.is_manual());
        cfg.set_interval(0);
        assert!(cfg.is_manual());
        cfg.set_interval(60);
        assert!(!cfg.is_manual());
    }
}
