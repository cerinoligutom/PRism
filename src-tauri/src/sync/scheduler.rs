//! Sync polling scheduler configuration.
//!
//! Default interval and clamping live here (ADR 0004: 300s default, range
//! 30s-10min). The worker reads `SchedulerConfig` once per cycle so settings
//! changes pick up on the next tick without restarting the task.
//!
//! The current interval is persisted on the `app_settings` singleton so the
//! user's chosen cadence survives an app restart. Helpers below read/write
//! the column; the worker init in `lib.rs` reads on startup, the
//! `set_sync_interval` command writes on every change.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::db::{lock_db, DbHandle};

pub const DEFAULT_INTERVAL_SECS: u64 = 300;
pub const MIN_INTERVAL_SECS: u64 = 30;
pub const MAX_INTERVAL_SECS: u64 = 600;

/// Fraction of the rate budget that must remain before a cycle is allowed to
/// run. Mirrors PRD §8.2 / ADR 0004 ("under 20% of 5000/hr per account").
pub const RATE_BUDGET_GUARD_PCT: u8 = 20;

/// Atomic poll-interval container so the worker reads the latest value without
/// locking. The setter clamps to `[MIN_INTERVAL_SECS, MAX_INTERVAL_SECS]`.
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
            interval_secs: AtomicU64::new(clamp_interval(interval_secs)),
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

    /// Replace the interval. Out-of-range inputs clamp to the nearest bound.
    pub fn set_interval(&self, secs: u64) {
        self.interval_secs
            .store(clamp_interval(secs), Ordering::Relaxed);
    }
}

fn clamp_interval(secs: u64) -> u64 {
    secs.clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS)
}

/// Read the persisted poll interval from `app_settings`. Returns `None` if
/// the column read fails (DB locked, migration mid-flight, transient I/O);
/// the caller should fall back to `DEFAULT_INTERVAL_SECS`. The value is
/// re-clamped on read since the column default could in theory drift from
/// the [`MIN_INTERVAL_SECS`, `MAX_INTERVAL_SECS`] range across migrations.
pub fn read_persisted_interval(db: &DbHandle) -> Option<u64> {
    let conn = match lock_db(db) {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("sync: read persisted interval — db lock failed: {err}");
            return None;
        }
    };
    match conn.query_row(
        "SELECT sync_interval_seconds FROM app_settings WHERE id = 1",
        [],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(secs) if secs > 0 => Some(clamp_interval(secs as u64)),
        Ok(_) => None,
        Err(err) => {
            eprintln!("sync: read persisted interval — query failed: {err}");
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
    let clamped = clamp_interval(secs) as i64;
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
}
