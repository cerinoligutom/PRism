use super::*;

#[test]
fn rate_remaining_pct_handles_unobserved_budget() {
    assert_eq!(rate_remaining_pct(-1, -1), None);
    assert_eq!(rate_remaining_pct(0, 0), None);
}

#[test]
fn rate_remaining_pct_computes_correct_percentage() {
    assert_eq!(rate_remaining_pct(5000, 5000), Some(100));
    assert_eq!(rate_remaining_pct(999, 5000), Some(19));
    assert_eq!(rate_remaining_pct(1000, 5000), Some(20));
    assert_eq!(rate_remaining_pct(0, 5000), Some(0));
}

fn snap(limit: i64, remaining: i64) -> ResourceSnapshot {
    ResourceSnapshot {
        limit,
        remaining,
        used: (limit - remaining).max(0),
        reset_at: std::time::UNIX_EPOCH,
    }
}

#[test]
fn under_guard_fires_below_threshold() {
    // 19% < 20% guard - fires.
    assert!(under_guard(snap(5000, 999), 20));
    // 20% == guard - does not fire (threshold is "below").
    assert!(!under_guard(snap(5000, 1000), 20));
    // Unobserved - no skip.
    assert!(!under_guard(snap(-1, -1), 20));
}

#[test]
fn under_guard_keys_against_active_resource_bucket() {
    // The worker passes the snapshot for whichever resource the next
    // call will hit. A tight search bucket must trip the guard even if
    // core / graphql are still healthy - and vice versa.
    use crate::github::rate_limit::RateBudget;
    use http::{HeaderMap, HeaderName, HeaderValue};

    let b = RateBudget::new();
    let mut h = HeaderMap::new();
    h.insert(
        HeaderName::from_static("x-ratelimit-resource"),
        HeaderValue::from_static("search"),
    );
    h.insert(
        HeaderName::from_static("x-ratelimit-limit"),
        HeaderValue::from_static("30"),
    );
    h.insert(
        HeaderName::from_static("x-ratelimit-remaining"),
        HeaderValue::from_static("5"),
    );
    h.insert(
        HeaderName::from_static("x-ratelimit-used"),
        HeaderValue::from_static("25"),
    );
    h.insert(
        HeaderName::from_static("x-ratelimit-reset"),
        HeaderValue::from_static("9999999999"),
    );
    b.update_from_headers(&h);

    let mut h2 = HeaderMap::new();
    h2.insert(
        HeaderName::from_static("x-ratelimit-resource"),
        HeaderValue::from_static("core"),
    );
    h2.insert(
        HeaderName::from_static("x-ratelimit-limit"),
        HeaderValue::from_static("5000"),
    );
    h2.insert(
        HeaderName::from_static("x-ratelimit-remaining"),
        HeaderValue::from_static("4900"),
    );
    h2.insert(
        HeaderName::from_static("x-ratelimit-used"),
        HeaderValue::from_static("100"),
    );
    h2.insert(
        HeaderName::from_static("x-ratelimit-reset"),
        HeaderValue::from_static("9999999999"),
    );
    b.update_from_headers(&h2);

    let snapshot = b.snapshot();
    // search is at ~17% remaining; trips.
    assert!(under_guard(
        snapshot.for_bucket(RateResource::Search),
        RATE_BUDGET_GUARD_PCT,
    ));
    // core is at 98%; clean.
    assert!(!under_guard(
        snapshot.for_bucket(RateResource::Core),
        RATE_BUDGET_GUARD_PCT,
    ));
    // graphql is unobserved; clean (no skip on unknown).
    assert!(!under_guard(
        snapshot.for_bucket(RateResource::Graphql),
        RATE_BUDGET_GUARD_PCT,
    ));
}

// --- SyncRepoError auth routing (issue #236) ---
//
// A missing or empty keychain entry surfaces as `GitHubError::Auth(...)`
// from `attach_auth`, which the worker must route through the same
// `Unauthorized` path as a 401 so the reauth dialog opens. An OS-level
// keychain failure (`AuthError::Keychain`) stays on the generic-failure
// path so a transient libsecret blip doesn't trigger reauth.
//
// The mapping uses `SyncRepoError::from_err_for(err, resource)` (issue
// #235); auth routing is independent of the resource bucket, so the
// tests pass an arbitrary one.

use crate::auth::keychain::MockKeychain;
use crate::auth::token_source::KeychainTokenSource;
use crate::github::auth::{AccountHandle, AuthError, TokenSource};

#[test]
fn sync_repo_error_routes_auth_missing_to_unauthorized() {
    let err = GitHubError::Auth(AuthError::Missing(1));
    let mapped = SyncRepoError::from_err_for(err, RateResource::Core);
    assert!(matches!(mapped, SyncRepoError::Unauthorized));
}

#[test]
fn sync_repo_error_routes_auth_empty_to_unauthorized() {
    let err = GitHubError::Auth(AuthError::Empty(1));
    let mapped = SyncRepoError::from_err_for(err, RateResource::Core);
    assert!(matches!(mapped, SyncRepoError::Unauthorized));
}

#[test]
fn sync_repo_error_keeps_auth_keychain_on_generic_failure_path() {
    let err = GitHubError::Auth(AuthError::Keychain(
        crate::auth::keychain::KeychainError::BackendUnavailable {
            hint: "libsecret unavailable".into(),
        },
    ));
    let mapped = SyncRepoError::from_err_for(err, RateResource::Core);
    assert!(matches!(mapped, SyncRepoError::Other(_)));
}

#[test]
fn mock_keychain_none_chains_to_sync_repo_unauthorized() {
    // Reproduces the worker-level discovery flow: `KeychainTokenSource`
    // wrapping a `MockKeychain` that returns `Ok(None)` produces
    // `AuthError::Missing`, which `attach_auth` wraps in
    // `GitHubError::Auth(...)`. The `SyncRepoError` conversion must
    // funnel that into the `Unauthorized` arm.
    let src = KeychainTokenSource::new(MockKeychain::new());
    let handle = AccountHandle::new(1, "github.com", "me");

    let auth_err = src.token(&handle).expect_err("missing keychain entry");
    assert!(matches!(auth_err, AuthError::Missing(1)));

    let github_err = GitHubError::Auth(auth_err);
    let mapped = SyncRepoError::from_err_for(github_err, RateResource::Core);
    assert!(matches!(mapped, SyncRepoError::Unauthorized));
}
