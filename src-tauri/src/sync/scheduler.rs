//! Sync polling scheduler configuration.
//!
//! Default interval and clamping live here (ADR 0004: 60s default, range
//! 30s-10min). The worker reads `SchedulerConfig` once per cycle so settings
//! changes pick up on the next tick without restarting the task.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_INTERVAL_SECS: u64 = 60;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_interval_is_60_seconds() {
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
