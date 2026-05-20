//! Per-account rate-limit accounting.
//!
//! GitHub exposes the current budget via response headers
//! (`x-ratelimit-limit`, `x-ratelimit-remaining`, `x-ratelimit-used`,
//! `x-ratelimit-reset`). GraphQL and REST share the same budget per account on
//! `github.com`; Enterprise hosts may configure their own cap and the headers
//! are the source of truth either way.

use http::HeaderMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Sentinel meaning "no header observed yet".
const UNSET: i64 = -1;

/// Atomic snapshot store. Updated on every response, snapshotted by the sync
/// worker before scheduling a fetch.
#[derive(Debug)]
pub struct RateBudget {
    limit: AtomicI64,
    remaining: AtomicI64,
    used: AtomicI64,
    reset_at_epoch: AtomicI64,
}

impl Default for RateBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl RateBudget {
    pub fn new() -> Self {
        Self {
            limit: AtomicI64::new(UNSET),
            remaining: AtomicI64::new(UNSET),
            used: AtomicI64::new(UNSET),
            reset_at_epoch: AtomicI64::new(UNSET),
        }
    }

    /// Read the current values.
    pub fn snapshot(&self) -> RateSnapshot {
        RateSnapshot {
            limit: self.limit.load(Ordering::Relaxed),
            remaining: self.remaining.load(Ordering::Relaxed),
            used: self.used.load(Ordering::Relaxed),
            reset_at: epoch_to_system_time(self.reset_at_epoch.load(Ordering::Relaxed)),
        }
    }

    /// Update from the response headers. Missing or unparseable headers leave
    /// existing values untouched.
    pub fn update_from_headers(&self, headers: &HeaderMap) {
        if let Some(v) = parse_i64(headers, "x-ratelimit-limit") {
            self.limit.store(v, Ordering::Relaxed);
        }
        if let Some(v) = parse_i64(headers, "x-ratelimit-remaining") {
            self.remaining.store(v, Ordering::Relaxed);
        }
        if let Some(v) = parse_i64(headers, "x-ratelimit-used") {
            self.used.store(v, Ordering::Relaxed);
        }
        if let Some(v) = parse_i64(headers, "x-ratelimit-reset") {
            self.reset_at_epoch.store(v, Ordering::Relaxed);
        }
    }
}

/// Point-in-time copy. `limit`, `remaining`, `used` are `-1` until the first
/// response is observed; the worker treats those as "unknown".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateSnapshot {
    pub limit: i64,
    pub remaining: i64,
    pub used: i64,
    pub reset_at: SystemTime,
}

impl RateSnapshot {
    pub fn is_observed(&self) -> bool {
        self.remaining != UNSET
    }

    /// Duration until reset, or `None` if reset is in the past or unknown.
    pub fn time_until_reset(&self) -> Option<Duration> {
        self.reset_at.duration_since(SystemTime::now()).ok()
    }
}

fn parse_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
}

fn epoch_to_system_time(epoch: i64) -> SystemTime {
    if epoch <= 0 {
        UNIX_EPOCH
    } else {
        UNIX_EPOCH + Duration::from_secs(epoch as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{HeaderName, HeaderValue};

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                HeaderName::from_static(k),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn snapshot_is_unobserved_before_update() {
        let b = RateBudget::new();
        let s = b.snapshot();
        assert!(!s.is_observed());
        assert_eq!(s.remaining, -1);
    }

    #[test]
    fn update_from_headers_populates_fields() {
        let b = RateBudget::new();
        b.update_from_headers(&headers(&[
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-remaining", "4321"),
            ("x-ratelimit-used", "679"),
            ("x-ratelimit-reset", "9999999999"),
        ]));
        let s = b.snapshot();
        assert!(s.is_observed());
        assert_eq!(s.limit, 5000);
        assert_eq!(s.remaining, 4321);
        assert_eq!(s.used, 679);
        assert_eq!(s.reset_at, UNIX_EPOCH + Duration::from_secs(9_999_999_999));
    }

    #[test]
    fn missing_headers_do_not_overwrite() {
        let b = RateBudget::new();
        b.update_from_headers(&headers(&[("x-ratelimit-remaining", "100")]));
        b.update_from_headers(&headers(&[("x-ratelimit-limit", "5000")]));
        let s = b.snapshot();
        assert_eq!(s.remaining, 100);
        assert_eq!(s.limit, 5000);
    }
}
