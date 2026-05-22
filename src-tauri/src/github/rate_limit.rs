//! Per-account rate-limit accounting.
//!
//! GitHub exposes the current budget via response headers
//! (`x-ratelimit-limit`, `x-ratelimit-remaining`, `x-ratelimit-used`,
//! `x-ratelimit-reset`). Each response also carries `x-ratelimit-resource`
//! identifying which sub-bucket the headers describe (`core`, `search`, or
//! `graphql` for github.com; Enterprise hosts may expose additional buckets
//! we still attribute to `core`). The dashboard sync hits all three: the
//! discovery phase queries Search, enrichment runs GraphQL, and timeline
//! pagination uses REST (core). Tracking each bucket separately lets the
//! worker's guard gate the phase whose budget is actually low instead of
//! parking the whole account on a generic "rate limited" state.
//!
//! A legacy top-level snapshot still surfaces the most recently observed
//! values so existing callers and the status bar's single budget label keep
//! working. New call sites read [`RateSnapshot::for_resource`].

use http::HeaderMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Sentinel meaning "no header observed yet".
const UNSET: i64 = -1;

/// GitHub's three documented rate-limit buckets. Any other value seen on
/// `x-ratelimit-resource` (e.g. `code_search`, `audit_log`) collapses into
/// [`Self::Core`] - the budget guard treats it the same as primary REST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateResource {
    Core,
    Search,
    Graphql,
}

impl RateResource {
    /// Parse the `x-ratelimit-resource` header value. Unknown values map to
    /// [`Self::Core`] so a future GitHub bucket name doesn't silently lose
    /// accounting.
    pub fn from_header(value: &str) -> Self {
        match value {
            "search" => Self::Search,
            "graphql" => Self::Graphql,
            _ => Self::Core,
        }
    }

    /// Wire-format name matching the `x-ratelimit-resource` header values.
    /// Used to thread the bucket identifier into emitted events without
    /// leaking the enum across the Rust / TypeScript boundary.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Search => "search",
            Self::Graphql => "graphql",
        }
    }
}

/// Per-bucket atomic store. Cheap to update on every response.
#[derive(Debug)]
struct ResourceBudget {
    limit: AtomicI64,
    remaining: AtomicI64,
    used: AtomicI64,
    reset_at_epoch: AtomicI64,
}

impl ResourceBudget {
    fn new() -> Self {
        Self {
            limit: AtomicI64::new(UNSET),
            remaining: AtomicI64::new(UNSET),
            used: AtomicI64::new(UNSET),
            reset_at_epoch: AtomicI64::new(UNSET),
        }
    }

    fn snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot {
            limit: self.limit.load(Ordering::Relaxed),
            remaining: self.remaining.load(Ordering::Relaxed),
            used: self.used.load(Ordering::Relaxed),
            reset_at: epoch_to_system_time(self.reset_at_epoch.load(Ordering::Relaxed)),
        }
    }

    fn store_from_headers(&self, headers: &HeaderMap) {
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

/// Atomic snapshot store. Updated on every response, snapshotted by the sync
/// worker before scheduling a fetch. Carries both a legacy "last observed"
/// view (preserved so the status bar's single budget label keeps working) and
/// three per-resource sub-buckets routed by the `x-ratelimit-resource` header.
#[derive(Debug)]
pub struct RateBudget {
    core: ResourceBudget,
    search: ResourceBudget,
    graphql: ResourceBudget,
}

impl Default for RateBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl RateBudget {
    pub fn new() -> Self {
        Self {
            core: ResourceBudget::new(),
            search: ResourceBudget::new(),
            graphql: ResourceBudget::new(),
        }
    }

    /// Read the current values. The top-level fields mirror the most recently
    /// observed bucket so single-budget consumers (e.g. the status bar)
    /// continue to surface a sensible value; per-resource callers use
    /// [`RateSnapshot::for_resource`].
    pub fn snapshot(&self) -> RateSnapshot {
        let core = self.core.snapshot();
        let search = self.search.snapshot();
        let graphql = self.graphql.snapshot();
        // The "global" view picks the most constrained observed bucket -
        // the status bar uses this for the single budget label, and "most
        // constrained" matches the user's mental model of "the worst one".
        let top = pick_most_constrained(&core, &search, &graphql).unwrap_or(ResourceSnapshot {
            limit: UNSET,
            remaining: UNSET,
            used: UNSET,
            reset_at: UNIX_EPOCH,
        });
        RateSnapshot {
            limit: top.limit,
            remaining: top.remaining,
            used: top.used,
            reset_at: top.reset_at,
            core,
            search,
            graphql,
        }
    }

    /// Update from the response headers. The `x-ratelimit-resource` header
    /// picks the bucket; absent or unknown values fall back to `core` (REST's
    /// default). Missing or unparseable numeric headers leave existing values
    /// untouched on the targeted bucket.
    pub fn update_from_headers(&self, headers: &HeaderMap) {
        let resource = headers
            .get("x-ratelimit-resource")
            .and_then(|v| v.to_str().ok())
            .map(RateResource::from_header)
            .unwrap_or(RateResource::Core);
        let bucket = match resource {
            RateResource::Core => &self.core,
            RateResource::Search => &self.search,
            RateResource::Graphql => &self.graphql,
        };
        bucket.store_from_headers(headers);
    }
}

/// Pick the bucket with the lowest `remaining` percentage. Returns `None` only
/// when every bucket is unobserved. Used to keep the status bar's single
/// budget label honest in the face of three independent sub-budgets.
fn pick_most_constrained(
    core: &ResourceSnapshot,
    search: &ResourceSnapshot,
    graphql: &ResourceSnapshot,
) -> Option<ResourceSnapshot> {
    let candidates = [core, search, graphql];
    candidates
        .into_iter()
        .filter(|s| s.is_observed())
        .min_by_key(|s| remaining_ppm(s.remaining, s.limit))
        .copied()
}

/// Parts-per-million remaining; used for ranking buckets without floating-point
/// arithmetic. Saturates at the max when `limit` is non-positive so unobserved
/// buckets never win the "most constrained" comparison.
fn remaining_ppm(remaining: i64, limit: i64) -> i64 {
    if limit <= 0 {
        return i64::MAX;
    }
    let pct = remaining.saturating_mul(1_000_000) / limit;
    pct.max(0)
}

/// Point-in-time copy. Top-level fields mirror the most-constrained observed
/// bucket; per-resource fields carry each sub-budget independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateSnapshot {
    pub limit: i64,
    pub remaining: i64,
    pub used: i64,
    pub reset_at: SystemTime,
    pub core: ResourceSnapshot,
    pub search: ResourceSnapshot,
    pub graphql: ResourceSnapshot,
}

impl RateSnapshot {
    pub fn is_observed(&self) -> bool {
        self.remaining != UNSET
    }

    /// Duration until reset, or `None` if reset is in the past or unknown.
    pub fn time_until_reset(&self) -> Option<Duration> {
        self.reset_at.duration_since(SystemTime::now()).ok()
    }

    /// Read the sub-bucket for a named resource. The string is matched the
    /// same way [`RateResource::from_header`] handles inbound header values.
    pub fn for_resource(&self, resource: &str) -> ResourceSnapshot {
        match RateResource::from_header(resource) {
            RateResource::Core => self.core,
            RateResource::Search => self.search,
            RateResource::Graphql => self.graphql,
        }
    }

    /// Same as [`for_resource`] but takes the typed enum directly.
    pub fn for_bucket(&self, resource: RateResource) -> ResourceSnapshot {
        match resource {
            RateResource::Core => self.core,
            RateResource::Search => self.search,
            RateResource::Graphql => self.graphql,
        }
    }
}

/// Per-resource view. Same numeric shape as the top-level snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceSnapshot {
    pub limit: i64,
    pub remaining: i64,
    pub used: i64,
    pub reset_at: SystemTime,
}

impl ResourceSnapshot {
    pub fn is_observed(&self) -> bool {
        self.remaining != UNSET
    }

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
        assert!(!s.core.is_observed());
        assert!(!s.search.is_observed());
        assert!(!s.graphql.is_observed());
    }

    #[test]
    fn update_without_resource_header_targets_core() {
        // Default REST responses don't always carry `x-ratelimit-resource`;
        // those updates must land in the `core` bucket so the existing
        // single-bucket behaviour is preserved.
        let b = RateBudget::new();
        b.update_from_headers(&headers(&[
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-remaining", "4321"),
            ("x-ratelimit-used", "679"),
            ("x-ratelimit-reset", "9999999999"),
        ]));
        let s = b.snapshot();
        assert!(s.core.is_observed());
        assert_eq!(s.core.remaining, 4321);
        assert!(!s.search.is_observed());
        assert!(!s.graphql.is_observed());
    }

    #[test]
    fn update_routes_by_x_ratelimit_resource_header() {
        let b = RateBudget::new();
        b.update_from_headers(&headers(&[
            ("x-ratelimit-resource", "search"),
            ("x-ratelimit-limit", "30"),
            ("x-ratelimit-remaining", "5"),
            ("x-ratelimit-used", "25"),
            ("x-ratelimit-reset", "9999999999"),
        ]));
        b.update_from_headers(&headers(&[
            ("x-ratelimit-resource", "graphql"),
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-remaining", "4000"),
            ("x-ratelimit-used", "1000"),
            ("x-ratelimit-reset", "9999999999"),
        ]));
        b.update_from_headers(&headers(&[
            ("x-ratelimit-resource", "core"),
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-remaining", "4900"),
            ("x-ratelimit-used", "100"),
            ("x-ratelimit-reset", "9999999999"),
        ]));

        let s = b.snapshot();
        assert_eq!(s.for_resource("search").remaining, 5);
        assert_eq!(s.for_resource("search").limit, 30);
        assert_eq!(s.for_resource("graphql").remaining, 4000);
        assert_eq!(s.for_resource("graphql").limit, 5000);
        assert_eq!(s.for_resource("core").remaining, 4900);
        assert_eq!(s.for_resource("core").limit, 5000);
    }

    #[test]
    fn unknown_resource_header_falls_back_to_core() {
        // GitHub may add new bucket names; we accept rather than drop the
        // accounting and treat them as core.
        let b = RateBudget::new();
        b.update_from_headers(&headers(&[
            ("x-ratelimit-resource", "code_search"),
            ("x-ratelimit-limit", "10"),
            ("x-ratelimit-remaining", "9"),
            ("x-ratelimit-used", "1"),
            ("x-ratelimit-reset", "9999999999"),
        ]));
        let s = b.snapshot();
        assert!(s.core.is_observed());
        assert_eq!(s.core.remaining, 9);
    }

    #[test]
    fn top_level_snapshot_reflects_most_constrained_bucket() {
        // search bucket at 5/30 (17%) is more constrained than graphql at
        // 4000/5000 (80%) - the top-level snapshot mirrors the worst one
        // so the status bar's single budget label still surfaces something
        // useful when only one sub-budget is tight.
        let b = RateBudget::new();
        b.update_from_headers(&headers(&[
            ("x-ratelimit-resource", "search"),
            ("x-ratelimit-limit", "30"),
            ("x-ratelimit-remaining", "5"),
            ("x-ratelimit-used", "25"),
            ("x-ratelimit-reset", "9999999999"),
        ]));
        b.update_from_headers(&headers(&[
            ("x-ratelimit-resource", "graphql"),
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-remaining", "4000"),
            ("x-ratelimit-used", "1000"),
            ("x-ratelimit-reset", "9999999999"),
        ]));
        let s = b.snapshot();
        assert_eq!(s.limit, 30);
        assert_eq!(s.remaining, 5);
    }

    #[test]
    fn missing_headers_do_not_overwrite_targeted_bucket() {
        let b = RateBudget::new();
        b.update_from_headers(&headers(&[
            ("x-ratelimit-resource", "core"),
            ("x-ratelimit-remaining", "100"),
        ]));
        b.update_from_headers(&headers(&[
            ("x-ratelimit-resource", "core"),
            ("x-ratelimit-limit", "5000"),
        ]));
        let s = b.snapshot();
        assert_eq!(s.core.remaining, 100);
        assert_eq!(s.core.limit, 5000);
    }

    #[test]
    fn rate_resource_from_header_recognises_canonical_values() {
        assert_eq!(RateResource::from_header("core"), RateResource::Core);
        assert_eq!(RateResource::from_header("search"), RateResource::Search);
        assert_eq!(RateResource::from_header("graphql"), RateResource::Graphql);
        assert_eq!(RateResource::from_header(""), RateResource::Core);
        assert_eq!(RateResource::from_header("audit_log"), RateResource::Core);
    }
}
