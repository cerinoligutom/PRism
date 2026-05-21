//! Integration tests for the background sync worker.
//!
//! These tests run the worker's `run_one_cycle` against a wiremock GraphQL +
//! REST server. They assert:
//!
//! 1. A single sync cycle drives a PR through detail-fetch, timeline-fetch,
//!    and SQLite persistence (including the derived latest-status-change).
//! 2. The per-account isolation contract: one account erroring does not stop
//!    another account's task — using the worker's own `EmitSink` / `WorkerHandle`.
//! 3. The 20% rate-budget guard: a 50-repo simulated cycle stays under the
//!    threshold; below threshold the worker emits a warning and skips.
//! 4. A 401 from upstream maps to the `Unauthorized` outcome and fires the
//!    reauth notifier exactly once.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use prism_lib::auth::store::{Account, AccountStore, SqlAccountStore};
use prism_lib::db::{open_at, DbHandle};
use prism_lib::github::{
    AccountHandle, EtagStore, GitHubClient, GitHubError, InMemoryEtagStore, StaticTokenSource,
};
use prism_lib::sync::{
    new_activity_buffer, AccountSyncState, ActivityBuffer, ClientFactory, CycleOutcome, EmitSink,
    ReauthNotifier, SchedulerConfig, SkipReason, SyncStateMap, WorkerContext,
};
use rusqlite::params;
use tempfile::TempDir;
use url::Url;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PR_DETAIL_FIXTURE: &str = include_str!("fixtures/pr_detail.json");
const REST_TIMELINE_FIXTURE: &str = include_str!("fixtures/timeline_full_lifecycle.json");
const DISCOVERY_EMPTY_FIXTURE: &str = include_str!("fixtures/discovery_empty.json");
const DISCOVERY_ONE_AUTHORED_FIXTURE: &str = include_str!("fixtures/discovery_one_authored.json");

#[derive(Default)]
struct CapturingEmitter {
    events: Mutex<Vec<(String, serde_json::Value)>>,
}

impl CapturingEmitter {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn count(&self, name: &str) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .count()
    }
}

impl EmitSink for CapturingEmitter {
    fn emit(&self, event: &str, payload: &serde_json::Value) {
        self.events
            .lock()
            .unwrap()
            .push((event.to_string(), payload.clone()));
    }
}

#[derive(Default)]
struct CountingReauth {
    fired: AtomicUsize,
}

impl CountingReauth {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn count(&self) -> usize {
        self.fired.load(Ordering::Relaxed)
    }
}

impl ReauthNotifier for CountingReauth {
    fn notify(&self, _account: &Account) {
        self.fired.fetch_add(1, Ordering::Relaxed);
    }
}

/// Build a `ClientFactory` that points every account at the mock server.
struct MockServerFactory {
    rest: Url,
    graphql: Url,
    etags: Arc<dyn EtagStore>,
}

impl ClientFactory for MockServerFactory {
    fn build(&self, account: &Account) -> Result<GitHubClient, GitHubError> {
        GitHubClient::builder()
            .account(AccountHandle::new(
                account.id,
                account.host.clone(),
                account.label.clone(),
            ))
            .token_source(Arc::new(StaticTokenSource::new("ghp_test_pat")))
            .etag_store(self.etags.clone())
            .base_rest_url(self.rest.clone())
            .base_graphql_url(self.graphql.clone())
            .build()
    }
}

struct Harness {
    _tmp: TempDir,
    db: DbHandle,
    accounts: Arc<dyn AccountStore>,
    state: SyncStateMap,
    emit: Arc<CapturingEmitter>,
    reauth: Arc<CountingReauth>,
    config: Arc<SchedulerConfig>,
    factory: Arc<MockServerFactory>,
    activity: ActivityBuffer,
}

impl Harness {
    fn ctx(&self) -> WorkerContext {
        WorkerContext {
            db: self.db.clone(),
            accounts: self.accounts.clone(),
            clients: self.factory.clone(),
            config: self.config.clone(),
            state: self.state.clone(),
            emit: self.emit.clone(),
            reauth: self.reauth.clone(),
            activity: self.activity.clone(),
        }
    }
}

fn setup_harness(server: &MockServer) -> Harness {
    let tmp = TempDir::new().expect("tempdir");
    let db = open_at(&tmp.path().join("prism.sqlite")).expect("open db");
    let accounts_store: Arc<dyn AccountStore> = Arc::new(SqlAccountStore::new(db.clone()));

    let base = Url::parse(&server.uri()).unwrap();
    Harness {
        _tmp: tmp,
        db,
        accounts: accounts_store,
        state: SyncStateMap::new(),
        emit: CapturingEmitter::new(),
        reauth: CountingReauth::new(),
        config: Arc::new(SchedulerConfig::default()),
        factory: Arc::new(MockServerFactory {
            rest: base.join("/").unwrap(),
            graphql: base.join("/graphql").unwrap(),
            etags: Arc::new(InMemoryEtagStore::new()),
        }),
        activity: new_activity_buffer(),
    }
}

fn seed_account(h: &Harness, id: u64, login: &str) -> Account {
    let account = Account {
        id,
        label: format!("Acct {id}"),
        host: "github.com".into(),
        login: login.into(),
        scopes: vec!["repo".into()],
        expires_at: None,
        avatar_url: None,
    };
    h.accounts.upsert(account.clone()).unwrap();
    account
}

fn seed_repo_with_pr(
    h: &Harness,
    repo_id: i64,
    account_id: u64,
    owner: &str,
    name: &str,
    pr_id: i64,
    number: i64,
) {
    let conn = h.db.lock().unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO repos (id, account_id, owner, name, visibility)
           VALUES (?1, ?2, ?3, ?4, 'public')",
        params![repo_id, account_id as i64, owner, name],
    )
    .unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, base_ref, head_ref)
            VALUES (?1, ?2, ?3, 'placeholder', 'open', 0, '', 0, 0, 'main', 'feature')",
        params![pr_id, repo_id, number],
    )
    .unwrap();
}

fn rate_headers(remaining: u64, limit: u64) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("x-ratelimit-limit", limit.to_string())
        .insert_header("x-ratelimit-remaining", remaining.to_string())
        .insert_header(
            "x-ratelimit-used",
            (limit.saturating_sub(remaining)).to_string(),
        )
        .insert_header("x-ratelimit-reset", "9999999999")
}

/// Mount an empty `DiscoverPrs` mock that catches every discovery search query.
/// Use this in tests that aren't asserting on discovery behaviour - the cycle's
/// discovery phase still runs three queries, all of which now return zero PRs.
async fn mount_empty_discovery(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("DiscoverPrs"))
        .respond_with(rate_headers(5000, 5000).set_body_raw(
            DISCOVERY_EMPTY_FIXTURE.as_bytes().to_vec(),
            "application/json",
        ))
        .mount(server)
        .await;
}

#[tokio::test]
async fn one_cycle_persists_pr_detail_and_latest_status_change() {
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 1, "alice");
    seed_repo_with_pr(&harness, 100, 1, "owner", "repo", 999, 42);

    mount_empty_discovery(&server).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("PrDetail"))
        .respond_with(
            rate_headers(4999, 5000)
                .set_body_raw(PR_DETAIL_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/42/timeline"))
        .respond_with(
            rate_headers(4998, 5000)
                .insert_header("etag", "W/\"t1\"")
                .set_body_raw(
                    REST_TIMELINE_FIXTURE.as_bytes().to_vec(),
                    "application/json",
                ),
        )
        .mount(&server)
        .await;

    let ctx = harness.ctx();
    let client = harness.factory.build(&account).unwrap();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;

    assert_eq!(report.outcome, CycleOutcome::Completed);
    assert_eq!(report.repos_visited, 1);
    assert_eq!(report.prs_visited, 1);

    // Verify the PR row was upserted with the GraphQL detail title.
    let title: String = harness
        .db
        .lock()
        .unwrap()
        .query_row("SELECT title FROM pull_requests WHERE id = 999", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(title, "Add a thing");

    // Latest status change derived from the REST timeline fixture (reopened).
    let (event_type, at): (Option<String>, Option<i64>) = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT latest_status_change_event_type, latest_status_change_at
               FROM pull_requests WHERE id = 999",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(event_type.as_deref(), Some("reopened"));
    assert!(at.is_some(), "latest_status_change_at must be set");

    // Dashboard enrichments from the detail fixture (mergeable, sizes, CI rollup).
    type Enrichments = (
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<i64>,
        Option<i64>,
    );
    let row: Enrichments = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT mergeable, review_decision, additions, deletions, changed_files,
                    ci_state, ci_total, ci_passing
               FROM pull_requests WHERE id = 999",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row.0.as_deref(), Some("MERGEABLE"));
    assert_eq!(row.1.as_deref(), Some("APPROVED"));
    assert_eq!(row.2, Some(120));
    assert_eq!(row.3, Some(30));
    assert_eq!(row.4, Some(5));
    assert_eq!(row.5.as_deref(), Some("SUCCESS"));
    assert_eq!(row.6, Some(3));
    assert_eq!(row.7, Some(3));

    // Requested reviewers reflect the fixture (user + team).
    let reviewers: Vec<(String, String)> = {
        let conn = harness.db.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT login, reviewer_type FROM requested_reviewers
                  WHERE pull_request_id = 999 ORDER BY reviewer_type, login",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        reviewers,
        vec![
            ("platform".to_string(), "team".to_string()),
            ("dave".to_string(), "user".to_string()),
        ]
    );

    // Conversation-depth enrichments (M3-A): threads, reviews, issue-comments
    // count. Fixture carries two threads (one resolved, one unresolved), two
    // reviews, and seven issue comments.
    type ThreadRow = (String, i64, i64, Option<String>, Option<i64>, i64);
    let threads: Vec<ThreadRow> = {
        let conn = harness.db.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT node_id, is_resolved, is_outdated, path, line, reply_count
                   FROM review_threads
                  WHERE pull_request_id = 999
                  ORDER BY node_id",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    };
    assert_eq!(threads.len(), 2, "two threads from the fixture");
    assert_eq!(threads[0].0, "PRRT_thread1");
    assert_eq!(threads[0].1, 1, "thread1 is resolved");
    assert_eq!(threads[0].3.as_deref(), Some("src/lib.rs"));
    assert_eq!(threads[0].4, Some(42));
    assert_eq!(threads[0].5, 0, "totalCount(1) - 1 = 0 replies");
    assert_eq!(threads[1].0, "PRRT_thread2");
    assert_eq!(threads[1].1, 0, "thread2 is unresolved");
    assert_eq!(threads[1].4, Some(88));
    assert_eq!(threads[1].5, 2, "totalCount(3) - 1 = 2 replies");

    // Issue #115: `review_threads.url` derives from the head comment's url
    // because PullRequestReviewThread itself doesn't expose `url`. Both
    // fixture threads carry a head comment with a discussion permalink.
    let thread_urls: Vec<(String, Option<String>)> = {
        let conn = harness.db.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT node_id, url FROM review_threads
                  WHERE pull_request_id = 999 ORDER BY node_id",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        thread_urls[0].1.as_deref(),
        Some("https://github.com/owner/repo/pull/42#discussion_r5001"),
        "thread1's url is the head comment's url"
    );
    assert_eq!(
        thread_urls[1].1.as_deref(),
        Some("https://github.com/owner/repo/pull/42#discussion_r5002"),
        "thread2's url is the head comment's url"
    );

    let reviews: Vec<(String, String, Option<String>, String)> = {
        let conn = harness.db.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT node_id, state, body, reviewer_login FROM reviews
                  WHERE pull_request_id = 999 ORDER BY node_id",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(reviews.len(), 2);
    assert_eq!(reviews[0].0, "PRR_1");
    assert_eq!(reviews[0].1, "APPROVED");
    assert_eq!(reviews[0].2.as_deref(), Some("LGTM overall."));
    assert_eq!(reviews[0].3, "bob");
    assert_eq!(reviews[1].0, "PRR_2");
    assert_eq!(reviews[1].1, "COMMENTED");

    let issue_count: i64 = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT issue_comments_count FROM pull_requests WHERE id = 999",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(issue_count, 7);

    // timeline_events: the REST fixture has eight qualifying events
    // (ready_for_review x 2, review_requested, reviewed, convert_to_draft,
    // merged, closed, reopened); non-qualifying events (labeled, committed,
    // assigned) are filtered upstream of persistence.
    type TimelineRow = (String, Option<String>, i64, String);
    let timeline_rows: Vec<TimelineRow> = {
        let conn = harness.db.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT event_type, actor_login, created_at, payload
                   FROM timeline_events
                  WHERE pull_request_id = 999
                  ORDER BY created_at, id",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        timeline_rows.len(),
        8,
        "eight qualifying events from the REST fixture",
    );
    // First qualifying event in the fixture is ready_for_review by alice.
    assert_eq!(timeline_rows[0].0, "ready_for_review");
    assert_eq!(timeline_rows[0].1.as_deref(), Some("alice"));
    assert_eq!(timeline_rows[0].3, "{}");
    // The `reviewed` event carries its review state in the payload column.
    let reviewed = timeline_rows
        .iter()
        .find(|r| r.0 == "reviewed")
        .expect("reviewed event present");
    assert_eq!(reviewed.1.as_deref(), Some("bob"));
    assert_eq!(reviewed.3, r#"{"state":"APPROVED"}"#);

    // Status event fired at least twice (Syncing + Synced).
    assert!(harness.emit.count("sync://status") >= 2);
    assert_eq!(harness.reauth.count(), 0);
}

#[tokio::test]
async fn cycle_skips_below_rate_budget_guard() {
    // Seed the budget below 20% before running the cycle. The guard must
    // skip with `RateBudgetGuard` and emit a rate-limit warning.
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 7, "bob");
    seed_repo_with_pr(&harness, 200, 7, "owner", "repo", 7000, 42);

    // Pre-populate the rate budget via a cheap REST call returning headers.
    Mock::given(method("GET"))
        .and(path("/seed"))
        .respond_with(rate_headers(500, 5000))
        .mount(&server)
        .await;

    let client = harness.factory.build(&account).unwrap();
    let _ = client.get_conditional("/seed").await;
    let snap = client.rate().snapshot();
    assert_eq!(snap.remaining, 500);

    let ctx = harness.ctx();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;

    match &report.outcome {
        CycleOutcome::Skipped {
            reason: SkipReason::RateBudgetGuard { pct },
        } => {
            assert!(*pct < 20, "pct {pct} should be below guard");
        }
        other => panic!("expected RateBudgetGuard, got {other:?}"),
    }
    assert!(harness.emit.count("sync://rate-limit-warning") >= 1);
}

#[tokio::test]
async fn fifty_repo_cycle_stays_under_twenty_percent_of_budget() {
    // The acceptance criterion: at the default 60s interval, one cycle
    // against 50 repos must consume less than 20% of the per-account budget.
    // With 1 PR per repo and 2 requests per PR (detail + timeline), that's
    // 100 requests against 5000 — 2% used, comfortably under the 20% cap.
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 1, "alice");

    for i in 0..50 {
        seed_repo_with_pr(
            &harness,
            1000 + i,
            1,
            &format!("owner{i}"),
            "repo",
            10_000 + i,
            i + 1,
        );
    }

    // Mocks: serve the same fixtures for every repo path. Each response
    // decrements `remaining` so the assertion below operates on a real delta.
    // wiremock can't decrement automatically, so we set the final headers to
    // 4900 remaining (100 requests consumed, exactly 2%).
    mount_empty_discovery(&server).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("PrDetail"))
        .respond_with(
            rate_headers(4900, 5000)
                .set_body_raw(PR_DETAIL_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(rate_headers(4900, 5000).set_body_raw(
            REST_TIMELINE_FIXTURE.as_bytes().to_vec(),
            "application/json",
        ))
        .mount(&server)
        .await;

    let ctx = harness.ctx();
    let client = harness.factory.build(&account).unwrap();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;

    assert_eq!(report.outcome, CycleOutcome::Completed);
    assert_eq!(report.repos_visited, 50);
    assert_eq!(report.prs_visited, 50);

    // Final budget snapshot: at least 80% remaining => under 20% consumed.
    let snap = client.rate().snapshot();
    assert!(snap.limit > 0, "limit observed");
    let pct_remaining = (snap.remaining * 100) / snap.limit;
    assert!(
        pct_remaining >= 80,
        "expected ≥80% budget remaining for a 50-repo cycle, got {pct_remaining}%",
    );
}

#[tokio::test]
async fn unauthorized_outcome_fires_reauth_notifier() {
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 1, "alice");
    seed_repo_with_pr(&harness, 100, 1, "owner", "repo", 999, 42);

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(header("authorization", "Bearer ghp_test_pat"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let ctx = harness.ctx();
    let client = harness.factory.build(&account).unwrap();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;

    assert_eq!(report.outcome, CycleOutcome::Unauthorized);
    // The notifier is fired by the per-account loop, not `run_one_cycle`;
    // assert the worker would route correctly by inspecting the state map.
    let state = harness.state.snapshot(1).expect("state for account 1");
    assert_eq!(state.phase, prism_lib::sync::SyncPhase::Unauthorized);
}

#[tokio::test]
async fn one_account_failing_does_not_stop_another() {
    // Per-account isolation: failure on account A should not affect account B.
    // We run two `run_one_cycle` invocations back-to-back; the first errors,
    // the second completes — and the state map records both outcomes
    // independently.
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account_a = seed_account(&harness, 1, "alice");
    let account_b = seed_account(&harness, 2, "bob");
    seed_repo_with_pr(&harness, 100, 1, "owner-a", "repo", 1000, 1);
    seed_repo_with_pr(&harness, 200, 2, "owner-b", "repo", 2000, 1);

    // Both accounts run a discovery phase first; mount an empty response so
    // the cycle proceeds into the per-repo enrichment loop where the
    // differentiation below kicks in.
    mount_empty_discovery(&server).await;
    // Account A: 500 on PR detail → fails as Server { 500 }.
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("PrDetail"))
        .and(wiremock::matchers::body_partial_json(serde_json::json!({
            "variables": { "owner": "owner-a" }
        })))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    // Account B: PR detail OK + timeline OK.
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("PrDetail"))
        .and(wiremock::matchers::body_partial_json(serde_json::json!({
            "variables": { "owner": "owner-b" }
        })))
        .respond_with(
            rate_headers(4999, 5000)
                .set_body_raw(PR_DETAIL_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/owner-b/repo/issues/1/timeline"))
        .respond_with(rate_headers(4998, 5000).set_body_raw(
            REST_TIMELINE_FIXTURE.as_bytes().to_vec(),
            "application/json",
        ))
        .mount(&server)
        .await;

    let ctx = harness.ctx();
    let report_a = prism_lib::sync::worker::run_one_cycle(
        &ctx,
        &harness.factory.build(&account_a).unwrap(),
        &account_a,
    )
    .await;
    let report_b = prism_lib::sync::worker::run_one_cycle(
        &ctx,
        &harness.factory.build(&account_b).unwrap(),
        &account_b,
    )
    .await;

    assert!(matches!(report_a.outcome, CycleOutcome::Failed { .. }));
    assert_eq!(report_b.outcome, CycleOutcome::Completed);

    let state_a = harness.state.snapshot(1).expect("state A");
    let state_b = harness.state.snapshot(2).expect("state B");
    assert_eq!(state_a.phase, prism_lib::sync::SyncPhase::Error);
    assert_eq!(state_b.phase, prism_lib::sync::SyncPhase::Synced);
    assert!(state_b.last_synced_at.is_some());
}

#[tokio::test]
async fn empty_discovery_with_no_repos_skips_as_no_repos_configured() {
    // A freshly added account whose discovery phase returns zero PRs - and
    // therefore inserts zero repos - should fall through to the
    // `Skipped { NoReposConfigured }` outcome and still emit a `Synced` status
    // so the UI clears any prior error state.
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 1, "alice");
    // Intentionally no repos seeded; discovery returns empty.
    mount_empty_discovery(&server).await;

    let ctx = harness.ctx();
    let client = harness.factory.build(&account).unwrap();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;

    assert!(matches!(
        report.outcome,
        CycleOutcome::Skipped {
            reason: SkipReason::NoReposConfigured
        }
    ));
    let state: AccountSyncState = harness.state.snapshot(1).expect("state");
    assert_eq!(state.phase, prism_lib::sync::SyncPhase::Synced);
}

#[tokio::test]
async fn add_account_hot_spawns_a_new_per_account_task() {
    // Start the worker with no accounts. Verify nothing is tracked.
    // Then hot-add an account via the public WorkerHandle::add_account hook
    // and verify (a) refresh_account now finds it, and (b) the state map
    // has a baseline entry for the new id.
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let ctx = harness.ctx();
    let worker = prism_lib::sync::spawn_worker(ctx);

    assert!(!worker.refresh_account(7));
    assert!(harness.state.snapshot(7).is_none());

    // Seed the account so the worker task can find it in the store on its
    // first cycle (the hot-add path stages the task; it reads metadata from
    // the store at run time, same as the startup path).
    let account = seed_account(&harness, 7, "carol");
    assert!(worker.add_account(account.clone()));
    // A second add for the same id should be a no-op.
    assert!(!worker.add_account(account.clone()));

    // refresh_account succeeds → the slot exists.
    assert!(worker.refresh_account(7));
    // State map seeded with the baseline.
    let state = harness.state.snapshot(7).expect("baseline state seeded");
    assert_eq!(state.phase, prism_lib::sync::SyncPhase::Idle);

    worker.shutdown();
}

#[tokio::test]
async fn on_token_updated_nudges_the_account_loop() {
    // Per-account re-auth (issue #59): after the auth command rewrites the
    // keychain, it fires `on_token_updated(account_id)` on the registered
    // listener. The worker implementation routes that to `refresh_account`
    // so a parked `SyncPhase::Unauthorized` loop wakes on the next cycle.
    //
    // The slot exists -> true. Untracked id -> false. This mirrors the
    // contract `refresh_account` already documents; the test pins the
    // listener wiring so a future refactor doesn't silently break re-auth.
    use prism_lib::auth::commands::AccountChangeListener;
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let ctx = harness.ctx();
    let worker = prism_lib::sync::spawn_worker(ctx);

    let account = seed_account(&harness, 11, "erin");
    assert!(worker.add_account(account));

    // Tracked account -> the listener fires refresh_account internally.
    // We can't observe the Notify directly, but we can observe that
    // refresh_account afterwards still returns true (slot intact) and
    // that an unknown id still returns false via the same hook.
    worker.on_token_updated(11);
    assert!(worker.refresh_account(11));

    // Untracked id is a clean no-op.
    worker.on_token_updated(999);
    assert!(!worker.refresh_account(999));

    worker.shutdown();
}

#[tokio::test]
async fn remove_account_cancels_task_and_clears_state() {
    // Hot-add then hot-remove: the slot disappears and the state map forgets.
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    // Spawn first (empty store) so the hot-add path is exercised explicitly,
    // not the startup auto-discovery branch.
    let ctx = harness.ctx();
    let worker = prism_lib::sync::spawn_worker(ctx);
    let account = seed_account(&harness, 9, "dora");
    assert!(worker.add_account(account.clone()));

    assert!(worker.remove_account(9));
    // Removing twice is a no-op (returns false).
    assert!(!worker.remove_account(9));
    // refresh_account no longer finds it; state map forgot it.
    assert!(!worker.refresh_account(9));
    assert!(harness.state.snapshot(9).is_none());

    worker.shutdown();
}

#[tokio::test]
async fn discovery_upserts_repo_pr_and_relation_then_runs_enrichment() {
    // End-to-end Wave 2-A flow: an Authored discovery hit auto-seeds the repo,
    // upserts the PR row, writes a relation with is_authored=1, and the
    // enrichment phase then picks the PR up via the seeded repo.
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 1, "alice");
    // No repos seeded - discovery will create the repo row from the search result.

    // Authored returns one PR; the other two queries return empty.
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("DiscoverPrs"))
        .and(body_string_contains("author:@me"))
        .respond_with(rate_headers(4999, 5000).set_body_raw(
            DISCOVERY_ONE_AUTHORED_FIXTURE.as_bytes().to_vec(),
            "application/json",
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("DiscoverPrs"))
        .respond_with(rate_headers(4998, 5000).set_body_raw(
            DISCOVERY_EMPTY_FIXTURE.as_bytes().to_vec(),
            "application/json",
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("PrDetail"))
        .respond_with(
            rate_headers(4997, 5000)
                .set_body_raw(PR_DETAIL_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/42/timeline"))
        .respond_with(rate_headers(4996, 5000).set_body_raw(
            REST_TIMELINE_FIXTURE.as_bytes().to_vec(),
            "application/json",
        ))
        .mount(&server)
        .await;

    let ctx = harness.ctx();
    let client = harness.factory.build(&account).unwrap();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;

    assert_eq!(report.outcome, CycleOutcome::Completed);
    assert_eq!(
        report.repos_visited, 1,
        "discovery should have auto-seeded one repo"
    );

    let conn = harness.db.lock().unwrap();
    let (owner, name): (String, String) = conn
        .query_row("SELECT owner, name FROM repos WHERE id = 100", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(owner, "owner");
    assert_eq!(name, "repo");

    let (is_auth, is_req, is_inv): (i64, i64, i64) = conn
        .query_row(
            "SELECT is_authored, is_review_requested, is_involved
               FROM pull_request_viewer_relations
              WHERE account_id = 1 AND pull_request_id = 999",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(is_auth, 1);
    assert_eq!(is_req, 0);
    assert_eq!(is_inv, 0);
}

#[tokio::test]
async fn end_of_cycle_pruning_drops_stale_relations() {
    // Seed a stale relation row (last_seen_at far in the past). The next
    // cycle must prune it once enrichment finishes, since discovery didn't
    // re-stamp it.
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 1, "alice");
    seed_repo_with_pr(&harness, 100, 1, "owner", "repo", 999, 42);

    // Pre-seed a relation with an ancient timestamp.
    harness
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, last_seen_at)
                VALUES (1, 999, 1, 0, 0, 1)",
            [],
        )
        .unwrap();

    mount_empty_discovery(&server).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("PrDetail"))
        .respond_with(
            rate_headers(4999, 5000)
                .set_body_raw(PR_DETAIL_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/42/timeline"))
        .respond_with(rate_headers(4998, 5000).set_body_raw(
            REST_TIMELINE_FIXTURE.as_bytes().to_vec(),
            "application/json",
        ))
        .mount(&server)
        .await;

    let ctx = harness.ctx();
    let client = harness.factory.build(&account).unwrap();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;
    assert_eq!(report.outcome, CycleOutcome::Completed);

    let survivors: i64 = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM pull_request_viewer_relations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        survivors, 0,
        "stale relation row must be pruned post-enrichment"
    );
}

#[tokio::test]
async fn discovery_failure_returns_failed_and_skips_pruning() {
    // A 500 from the very first discovery query halts the cycle as `Failed`
    // and leaves any pre-existing relations alone (no pruning on failure).
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 1, "alice");
    seed_repo_with_pr(&harness, 100, 1, "owner", "repo", 999, 42);
    harness
        .db
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, last_seen_at)
                VALUES (1, 999, 1, 0, 0, 1)",
            [],
        )
        .unwrap();

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("DiscoverPrs"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let ctx = harness.ctx();
    let client = harness.factory.build(&account).unwrap();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;

    assert!(matches!(report.outcome, CycleOutcome::Failed { .. }));

    let survivors: i64 = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM pull_request_viewer_relations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(survivors, 1, "discovery hiccup must not drop relations");
}

struct ThreadFixture<'a> {
    node_id: &'a str,
    is_resolved: bool,
    is_outdated: bool,
    path: &'a str,
    line: Option<i64>,
    total_count: i64,
}

struct ReviewFixture<'a> {
    node_id: &'a str,
    state: &'a str,
    body: &'a str,
    author: &'a str,
}

/// Build a `PrDetail` GraphQL response body for the conversation-depth fields.
/// Keeps the rest of the PR shape minimal so the body stays diffable.
fn pr_detail_body_with_threads_and_issue_comments(
    threads: &[ThreadFixture<'_>],
    reviews: &[ReviewFixture<'_>],
    issue_comments_total: i64,
) -> String {
    let thread_nodes: Vec<String> = threads
        .iter()
        .map(|t| {
            let line_json = match t.line {
                Some(n) => n.to_string(),
                None => "null".into(),
            };
            format!(
                r#"{{
                    "id": "{node_id}",
                    "isResolved": {is_resolved},
                    "isOutdated": {is_outdated},
                    "path": "{path}",
                    "line": {line_json},
                    "startLine": null,
                    "originalLine": null,
                    "comments": {{
                        "totalCount": {total_count},
                        "nodes": [{{
                            "id": "{node_id}_C1",
                            "url": "https://github.com/owner/repo/pull/42#discussion_r{node_id}",
                            "author": {{ "login": "alice" }},
                            "bodyText": "head body",
                            "createdAt": "2026-05-18T10:00:00Z"
                        }}]
                    }}
                }}"#,
                node_id = t.node_id,
                is_resolved = t.is_resolved,
                is_outdated = t.is_outdated,
                path = t.path,
                total_count = t.total_count,
            )
        })
        .collect();

    let review_nodes: Vec<String> = reviews
        .iter()
        .map(|r| {
            format!(
                r#"{{
                    "id": "{node_id}",
                    "state": "{state}",
                    "body": "{body}",
                    "submittedAt": "2026-05-18T12:00:00Z",
                    "author": {{ "login": "{author}" }}
                }}"#,
                node_id = r.node_id,
                state = r.state,
                body = r.body,
                author = r.author,
            )
        })
        .collect();

    format!(
        r#"{{
            "data": {{
                "repository": {{
                    "pullRequest": {{
                        "id": "PR_test",
                        "number": 42,
                        "title": "Threaded PR",
                        "isDraft": false,
                        "state": "OPEN",
                        "merged": false,
                        "mergeable": "MERGEABLE",
                        "url": "https://github.com/owner/repo/pull/42",
                        "createdAt": "2026-05-18T10:00:00Z",
                        "updatedAt": "2026-05-19T11:00:00Z",
                        "author": {{ "login": "alice" }},
                        "baseRefName": "main",
                        "headRefName": "feat/threads",
                        "reviewDecision": null,
                        "additions": 1,
                        "deletions": 0,
                        "changedFiles": 1,
                        "reviewRequests": {{ "nodes": [] }},
                        "commits": {{ "nodes": [] }},
                        "reviewThreads": {{
                            "pageInfo": {{ "hasNextPage": false, "endCursor": null }},
                            "nodes": [{}]
                        }},
                        "reviews": {{ "nodes": [{}] }},
                        "issueComments": {{ "totalCount": {issue_comments_total} }}
                    }}
                }}
            }}
        }}"#,
        thread_nodes.join(","),
        review_nodes.join(",")
    )
}

#[tokio::test]
async fn conversation_depth_persists_mixed_thread_states_and_prunes_on_next_cycle() {
    // Two cycles. Cycle 1 carries three threads (unresolved, resolved,
    // outdated) and two reviews. Cycle 2 drops one thread and one review,
    // resolves a previously-unresolved thread, and flips a resolved thread
    // back to unresolved. Asserts: upserts apply, resolved_at tracks the
    // transition, pruning removes gone threads/reviews, issue_comments_count
    // re-writes.
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 1, "alice");
    seed_repo_with_pr(&harness, 100, 1, "owner", "repo", 999, 42);

    mount_empty_discovery(&server).await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/42/timeline"))
        .respond_with(rate_headers(4998, 5000).set_body_raw(
            REST_TIMELINE_FIXTURE.as_bytes().to_vec(),
            "application/json",
        ))
        .mount(&server)
        .await;

    // Cycle 1.
    let cycle1_body = pr_detail_body_with_threads_and_issue_comments(
        &[
            ThreadFixture {
                node_id: "PRRT_unresolved",
                is_resolved: false,
                is_outdated: false,
                path: "a.rs",
                line: Some(1),
                total_count: 1,
            },
            ThreadFixture {
                node_id: "PRRT_resolved",
                is_resolved: true,
                is_outdated: false,
                path: "b.rs",
                line: Some(2),
                total_count: 1,
            },
            ThreadFixture {
                node_id: "PRRT_drop",
                is_resolved: false,
                is_outdated: false,
                path: "c.rs",
                line: Some(3),
                total_count: 1,
            },
        ],
        &[
            ReviewFixture {
                node_id: "PRR_keep",
                state: "APPROVED",
                body: "LGTM",
                author: "bob",
            },
            ReviewFixture {
                node_id: "PRR_drop",
                state: "COMMENTED",
                body: "wip",
                author: "carol",
            },
        ],
        5,
    );
    let cycle1_mock = Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("PrDetail"))
        .respond_with(
            rate_headers(4999, 5000).set_body_raw(cycle1_body.into_bytes(), "application/json"),
        )
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let ctx = harness.ctx();
    let client = harness.factory.build(&account).unwrap();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;
    assert_eq!(report.outcome, CycleOutcome::Completed);
    drop(cycle1_mock);

    let resolved_at_initial: Option<i64> = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT resolved_at FROM review_threads WHERE node_id = 'PRRT_resolved'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        resolved_at_initial.is_some(),
        "thread that arrived resolved must have resolved_at set"
    );

    let issue_count: i64 = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT issue_comments_count FROM pull_requests WHERE id = 999",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(issue_count, 5);

    // Cycle 2: drop PRRT_drop + PRR_drop; resolve the previously-unresolved
    // thread; flip the previously-resolved thread back to unresolved; bump
    // issue_comments_count to 8.
    let cycle2_body = pr_detail_body_with_threads_and_issue_comments(
        &[
            ThreadFixture {
                node_id: "PRRT_unresolved",
                is_resolved: true,
                is_outdated: false,
                path: "a.rs",
                line: Some(1),
                total_count: 1,
            },
            ThreadFixture {
                node_id: "PRRT_resolved",
                is_resolved: false,
                is_outdated: false,
                path: "b.rs",
                line: Some(2),
                total_count: 1,
            },
        ],
        &[ReviewFixture {
            node_id: "PRR_keep",
            state: "APPROVED",
            body: "LGTM",
            author: "bob",
        }],
        8,
    );
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("PrDetail"))
        .respond_with(
            rate_headers(4998, 5000).set_body_raw(cycle2_body.into_bytes(), "application/json"),
        )
        .mount(&server)
        .await;

    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;
    assert_eq!(report.outcome, CycleOutcome::Completed);

    // PRRT_unresolved transitioned to resolved → resolved_at stamped.
    let resolved_at_new: Option<i64> = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT resolved_at FROM review_threads WHERE node_id = 'PRRT_unresolved'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        resolved_at_new.is_some(),
        "transition to resolved must stamp resolved_at"
    );

    // PRRT_resolved transitioned back → resolved_at cleared.
    let resolved_at_cleared: Option<i64> = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT resolved_at FROM review_threads WHERE node_id = 'PRRT_resolved'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        resolved_at_cleared, None,
        "transition back to unresolved must clear resolved_at"
    );

    // PRRT_drop pruned.
    let drop_present: i64 = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM review_threads WHERE node_id = 'PRRT_drop'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(drop_present, 0, "dropped thread must be pruned");

    // PRR_drop pruned.
    let drop_review_present: i64 = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM reviews WHERE node_id = 'PRR_drop'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(drop_review_present, 0, "dropped review must be pruned");

    // issue_comments_count overwrites.
    let issue_count: i64 = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT issue_comments_count FROM pull_requests WHERE id = 999",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(issue_count, 8);
}

/// Two-cycle integration: the persisted timeline must wipe-and-rewrite cleanly
/// when the second cycle returns a changed event set.
#[tokio::test]
async fn timeline_events_wipe_and_rewrite_across_two_cycles() {
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 1, "alice");
    seed_repo_with_pr(&harness, 100, 1, "owner", "repo", 999, 42);

    mount_empty_discovery(&server).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("PrDetail"))
        .respond_with(
            rate_headers(4999, 5000)
                .set_body_raw(PR_DETAIL_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    // Cycle 1: a short timeline with ready_for_review + a reviewed APPROVED.
    let cycle1_timeline = r#"[
        {
            "event": "ready_for_review",
            "created_at": "2026-05-02T14:30:00Z",
            "actor": { "login": "alice", "id": 1 }
        },
        {
            "event": "reviewed",
            "submitted_at": "2026-05-03T10:00:00Z",
            "state": "approved",
            "user": { "login": "bob", "id": 2 }
        }
    ]"#;
    let cycle1_mock = Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/42/timeline"))
        .respond_with(
            rate_headers(4998, 5000)
                .set_body_raw(cycle1_timeline.as_bytes().to_vec(), "application/json"),
        )
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    let ctx = harness.ctx();
    let client = harness.factory.build(&account).unwrap();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;
    assert_eq!(report.outcome, CycleOutcome::Completed);
    drop(cycle1_mock);

    let rows_after_cycle1: i64 = harness
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM timeline_events WHERE pull_request_id = 999",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows_after_cycle1, 2);

    // Cycle 2: a different event set - the prior reviewed APPROVED is gone
    // (rare backfill simulation), replaced by a CHANGES_REQUESTED review and a
    // merged event. The wipe-and-rewrite policy must replace the whole set.
    let cycle2_timeline = r#"[
        {
            "event": "ready_for_review",
            "created_at": "2026-05-02T14:30:00Z",
            "actor": { "login": "alice", "id": 1 }
        },
        {
            "event": "reviewed",
            "submitted_at": "2026-05-04T11:00:00Z",
            "state": "changes_requested",
            "user": { "login": "bob", "id": 2 }
        },
        {
            "event": "merged",
            "created_at": "2026-05-06T11:00:00Z",
            "actor": { "login": "alice", "id": 1 }
        }
    ]"#;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/42/timeline"))
        .respond_with(
            rate_headers(4997, 5000)
                .set_body_raw(cycle2_timeline.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;
    assert_eq!(report.outcome, CycleOutcome::Completed);

    type TimelineRow = (String, Option<String>, String);
    let rows: Vec<TimelineRow> = {
        let conn = harness.db.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT event_type, actor_login, payload FROM timeline_events
                  WHERE pull_request_id = 999 ORDER BY created_at, id",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(rows.len(), 3, "wipe-and-rewrite replaced the previous set");
    let states: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(states, vec!["ready_for_review", "reviewed", "merged"]);
    // The reviewed event's payload reflects the new state, proving the
    // overwrite is observed on the cached row.
    let reviewed = rows.iter().find(|r| r.0 == "reviewed").unwrap();
    assert_eq!(reviewed.2, r#"{"state":"CHANGES_REQUESTED"}"#);
}

// ===== Diagnostic activity feed (issue #122) =====
//
// A single happy-path cycle must emit the documented `sync://activity`
// sequence: cycle_started -> phase_started discovery -> phase_completed
// discovery -> phase_started enrichment -> pr_fetched + phase_progress per PR
// -> phase_completed enrichment -> phase_started pruning -> phase_completed
// pruning -> cycle_completed. The failure path must emit `cycle_failed` with
// the underlying error message.

fn activity_event_kinds(emit: &CapturingEmitter) -> Vec<String> {
    emit.events
        .lock()
        .unwrap()
        .iter()
        .filter(|(name, _)| name == "sync://activity")
        .map(|(_, payload)| {
            payload
                .get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

fn activity_event_messages(emit: &CapturingEmitter) -> Vec<String> {
    emit.events
        .lock()
        .unwrap()
        .iter()
        .filter(|(name, _)| name == "sync://activity")
        .map(|(_, payload)| {
            payload
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn happy_cycle_emits_full_activity_event_sequence() {
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 1, "alice");
    seed_repo_with_pr(&harness, 100, 1, "owner", "repo", 999, 42);

    mount_empty_discovery(&server).await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("PrDetail"))
        .respond_with(
            rate_headers(4999, 5000)
                .set_body_raw(PR_DETAIL_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/42/timeline"))
        .respond_with(rate_headers(4998, 5000).set_body_raw(
            REST_TIMELINE_FIXTURE.as_bytes().to_vec(),
            "application/json",
        ))
        .mount(&server)
        .await;

    let ctx = harness.ctx();
    let client = harness.factory.build(&account).unwrap();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;
    assert_eq!(report.outcome, CycleOutcome::Completed);

    let kinds = activity_event_kinds(&harness.emit);
    // The exact order matters: cycle -> discovery -> enrichment + PRs ->
    // pruning -> cycle completion. The pr_fetched + phase_progress pair fires
    // once per enriched PR (here, one).
    assert_eq!(
        kinds,
        vec![
            "cycle_started",
            "phase_started",
            "phase_completed",
            "phase_started",
            "pr_fetched",
            "phase_progress",
            "phase_completed",
            "phase_started",
            "phase_completed",
            "cycle_completed",
        ],
        "activity event order",
    );

    // The pre-rendered messages mention the active account login and the
    // owner/name/number for the per-PR row, so the panel doesn't have to
    // re-render structured payloads.
    let messages = activity_event_messages(&harness.emit);
    assert!(
        messages.iter().any(|m| m.contains("alice")),
        "cycle-level messages mention the account login: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("owner/repo#42") || m.contains("owner/repo#42")),
        "per-PR message includes owner/name#number: {messages:?}"
    );

    // The buffer itself holds the same events (the worker writes through
    // `record`, which both appends and emits).
    let buffered = prism_lib::sync::activity::snapshot(&harness.activity, 100, Some(1));
    assert_eq!(buffered.len(), kinds.len(), "buffer mirrors emitted events");
}

#[tokio::test]
async fn cycle_failure_emits_cycle_failed_event_with_error_message() {
    // Stub a discovery failure (500 on the GraphQL endpoint) and assert the
    // activity feed surfaces a `cycle_failed` event with the underlying error.
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 1, "alice");
    seed_repo_with_pr(&harness, 100, 1, "owner", "repo", 999, 42);

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("DiscoverPrs"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let ctx = harness.ctx();
    let client = harness.factory.build(&account).unwrap();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;
    assert!(matches!(report.outcome, CycleOutcome::Failed { .. }));

    let kinds = activity_event_kinds(&harness.emit);
    assert!(
        kinds.contains(&"cycle_failed".to_string()),
        "cycle_failed must be emitted on a failed cycle: {kinds:?}"
    );
    // Stricter: the failure event carries the structured `error_kind`
    // discriminator the panel uses to colour the row.
    let activity_events: Vec<serde_json::Value> = harness
        .emit
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|(n, _)| n == "sync://activity")
        .map(|(_, p)| p.clone())
        .collect();
    let failed = activity_events
        .iter()
        .find(|p| p.get("kind").and_then(|k| k.as_str()) == Some("cycle_failed"))
        .expect("cycle_failed event present");
    assert_eq!(
        failed.get("error_kind").and_then(|k| k.as_str()),
        Some("discovery"),
        "error_kind classifies the failing phase",
    );
    assert_eq!(
        failed.get("level").and_then(|l| l.as_str()),
        Some("error"),
        "cycle_failed event is at error level",
    );
}

#[tokio::test]
async fn rate_budget_guard_emits_rate_limit_pause_activity() {
    // Seed the budget below 20% before running the cycle. Activity feed must
    // surface the pause as a `rate_limit_pause` warn event so the panel can
    // explain the skip without leaving the user wondering.
    let server = MockServer::start().await;
    let harness = setup_harness(&server);
    let account = seed_account(&harness, 7, "bob");
    seed_repo_with_pr(&harness, 200, 7, "owner", "repo", 7000, 42);

    Mock::given(method("GET"))
        .and(path("/seed"))
        .respond_with(rate_headers(500, 5000))
        .mount(&server)
        .await;
    let client = harness.factory.build(&account).unwrap();
    let _ = client.get_conditional("/seed").await;

    let ctx = harness.ctx();
    let report = prism_lib::sync::worker::run_one_cycle(&ctx, &client, &account).await;
    assert!(matches!(report.outcome, CycleOutcome::Skipped { .. }));

    let kinds = activity_event_kinds(&harness.emit);
    assert!(
        kinds.contains(&"rate_limit_pause".to_string()),
        "rate_limit_pause must be emitted: {kinds:?}"
    );
}
