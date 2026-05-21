//! Integration tests for the M3-B lazy hydrator's network + persistence path.
//!
//! Drives `prism_lib::conversation::commands::testing::fetch` against a
//! wiremock GraphQL server. The point is to assert that a single call writes
//! the comments + issue comments into SQLite and that the persistence path is
//! idempotent across repeated calls.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use prism_lib::conversation::commands::testing as commands_testing;
use prism_lib::db::{migrate, DbHandle};
use prism_lib::github::{AccountHandle, GitHubClient, InMemoryEtagStore, StaticTokenSource};
use rusqlite::Connection;
use url::Url;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PR_COMMENTS_FIXTURE: &str = include_str!("fixtures/pr_comments.json");

fn fresh_db() -> DbHandle {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate::run(&mut conn).unwrap();
    Arc::new(Mutex::new(conn))
}

fn seed_pr_with_thread(db: &DbHandle) {
    let conn = db.lock().unwrap();
    conn.execute_batch(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'alice', 'github.com', 'alice', 0);
        INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
            (10, 1, 'owner', 'repo', 'public');
        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, base_ref, head_ref,
             issue_comments_count)
            VALUES
            (100, 10, 42, 'pr/#42', 'open', 0, 'alice', 0, 0, 'main', 'feat', 1);

        -- Pre-existing thread (sync cycle wrote the header) keyed by the same
        -- node id the fixture's comments reference, so resolve_thread_id finds
        -- a row to attach the comments to. `reply_count = 1` mirrors what the
        -- cycle writes for a thread with `comments.totalCount = 2` (head + one
        -- reply) — the comment-type breakdown reads this column.
        INSERT INTO review_threads
            (id, pull_request_id, is_resolved, is_outdated, original_line,
             path, node_id, created_at, reply_count,
             head_comment_author_login, head_comment_body_text,
             head_comment_created_at, line)
            VALUES (1000, 100, 0, 0, 42, 'src/lib.rs', 'PRRT_fix1',
                    1000, 1, 'bob', 'should this be wrapped?', 1000, 42);

        -- One review with a body so the breakdown's `summary` count is positive.
        INSERT INTO reviews
            (id, pull_request_id, reviewer_login, state, submitted_at, body, node_id)
            VALUES (70001, 100, 'bob', 'APPROVED', 1900, 'lgtm', 'REV_1');
        "#,
    )
    .unwrap();
}

async fn client_against(server: &MockServer) -> GitHubClient {
    let base = Url::parse(&server.uri()).unwrap();
    GitHubClient::builder()
        .account(AccountHandle::new(1, "github.com", "alice"))
        .token_source(Arc::new(StaticTokenSource::new("ghp_test_pat")))
        .etag_store(Arc::new(InMemoryEtagStore::new()))
        .base_rest_url(base.join("/").unwrap())
        .base_graphql_url(base.join("/graphql").unwrap())
        .build()
        .unwrap()
}

#[tokio::test]
async fn hydrator_auto_marks_pr_read_after_persisting() {
    // M4-A acceptance: opening the drawer auto-marks the PR read for the
    // active account. The hook fires after the hydration transaction
    // commits and writes through the shared `triage::query::mark_read`
    // helper.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("PrComments"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(PR_COMMENTS_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let db = fresh_db();
    seed_pr_with_thread(&db);
    // Pre-seed a stale relations row so the auto-mark exercises the UPSERT
    // path's UPDATE branch (rather than the INSERT branch).
    db.lock()
        .unwrap()
        .execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, last_seen_at, mentioned_count_unread)
                VALUES (1, 100, 1, 0, 0, 0, 7)",
            [],
        )
        .unwrap();

    let client = client_against(&server).await;
    commands_testing::fetch(&db, &client, 100, "owner", "repo", 42, 1)
        .await
        .expect("hydrator");

    let conn = db.lock().unwrap();
    let (read_at, mentioned): (Option<i64>, i64) = conn
        .query_row(
            "SELECT read_at, mentioned_count_unread
               FROM pull_request_viewer_relations
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(read_at.is_some(), "auto-mark should set read_at");
    assert_eq!(mentioned, 0, "auto-mark should reset mention counter");
}

#[tokio::test]
async fn hydrator_auto_mark_is_idempotent_across_reopens() {
    // Repeated drawer opens must not break anything. The mention counter
    // stays at zero (the sync scanner is the only thing that bumps it).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(PR_COMMENTS_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let db = fresh_db();
    seed_pr_with_thread(&db);
    let client = client_against(&server).await;

    commands_testing::fetch(&db, &client, 100, "owner", "repo", 42, 1)
        .await
        .expect("first open");
    commands_testing::fetch(&db, &client, 100, "owner", "repo", 42, 1)
        .await
        .expect("second open");

    let conn = db.lock().unwrap();
    let (read_at, mentioned, rows): (Option<i64>, i64, i64) = conn
        .query_row(
            "SELECT (SELECT read_at FROM pull_request_viewer_relations
                      WHERE account_id = 1 AND pull_request_id = 100),
                    (SELECT mentioned_count_unread FROM pull_request_viewer_relations
                      WHERE account_id = 1 AND pull_request_id = 100),
                    (SELECT COUNT(*) FROM pull_request_viewer_relations
                      WHERE account_id = 1 AND pull_request_id = 100)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert!(read_at.is_some());
    assert_eq!(mentioned, 0);
    assert_eq!(rows, 1, "second open must not duplicate the relations row");
}

#[tokio::test]
async fn hydrator_round_trip_persists_comments_and_issue_comments() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_string_contains("PrComments"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-ratelimit-limit", "5000")
                .insert_header("x-ratelimit-remaining", "4999")
                .insert_header("x-ratelimit-used", "1")
                .set_body_raw(PR_COMMENTS_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let db = fresh_db();
    seed_pr_with_thread(&db);
    let client = client_against(&server).await;

    let hydrated = commands_testing::fetch(&db, &client, 100, "owner", "repo", 42, 1)
        .await
        .expect("hydrator");

    assert_eq!(hydrated.pull_request_id, 100);
    // Two comments persisted under thread 1000.
    assert_eq!(hydrated.thread_comments.len(), 2);
    assert!(hydrated.thread_comments.iter().all(|c| c.thread_id == 1000));
    // Issue #115: every review comment in the fixture carries a `url`; the
    // hydrator must persist it through to the DTO.
    assert_eq!(
        hydrated.thread_comments[0].url.as_deref(),
        Some("https://github.com/owner/repo/pull/42#discussion_r4001"),
    );
    assert_eq!(
        hydrated.thread_comments[1].url.as_deref(),
        Some("https://github.com/owner/repo/pull/42#discussion_r4002"),
    );
    // One issue comment persisted.
    assert_eq!(hydrated.issue_comments.len(), 1);
    assert_eq!(hydrated.issue_comments[0].author_login, "carol");
    assert_eq!(
        hydrated.issue_comments[0].url.as_deref(),
        Some("https://github.com/owner/repo/pull/42#issuecomment-8001"),
        "issue comment url persisted (issue #115)",
    );
    // Review still surfaces in the hydrated response.
    assert_eq!(hydrated.reviews.len(), 1);
    assert_eq!(hydrated.reviews[0].body.as_deref(), Some("lgtm"));
    // The stats reflect the persisted comments (review count = 2).
    assert_eq!(hydrated.stats.comment_breakdown.review, 2);
    assert_eq!(hydrated.stats.comment_breakdown.summary, 1);
    assert_eq!(hydrated.stats.comment_breakdown.issue, 1, "rollup column");
}

/// Counts how many `POST /graphql` requests the mock observes. A second
/// hydrator call against the same DB after disabling the mount yields an error
/// only if the network is hit; instead we assert that the persistence layer is
/// the source of truth for cached re-renders (the frontend's conversation
/// store de-duplicates concurrent mounts; the backend stays stateless).
#[tokio::test]
async fn hydrator_repeated_call_writes_idempotently() {
    let counter = Arc::new(AtomicUsize::new(0));
    let server = MockServer::start().await;
    let payload_counter = counter.clone();
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(move |_: &wiremock::Request| {
            payload_counter.fetch_add(1, Ordering::Relaxed);
            ResponseTemplate::new(200)
                .set_body_raw(PR_COMMENTS_FIXTURE.as_bytes().to_vec(), "application/json")
        })
        .mount(&server)
        .await;

    let db = fresh_db();
    seed_pr_with_thread(&db);
    let client = client_against(&server).await;

    commands_testing::fetch(&db, &client, 100, "owner", "repo", 42, 1)
        .await
        .expect("first call");
    commands_testing::fetch(&db, &client, 100, "owner", "repo", 42, 1)
        .await
        .expect("second call");

    // Persistence is idempotent — re-running the same payload must not
    // duplicate rows.
    let conn = db.lock().unwrap();
    let comments: i64 = conn
        .query_row("SELECT COUNT(*) FROM review_comments", [], |r| r.get(0))
        .unwrap();
    let issues: i64 = conn
        .query_row("SELECT COUNT(*) FROM issue_comments", [], |r| r.get(0))
        .unwrap();
    assert_eq!(comments, 2, "review_comments must dedupe by node_id");
    assert_eq!(issues, 1, "issue_comments must dedupe by node_id");

    // The hydrator fires the network call on every invocation by design
    // (frontend caches; backend doesn't). Two calls -> two requests.
    assert_eq!(counter.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn hydrator_skips_unknown_threads_without_aborting() {
    // The seed leaves thread node id 'PRRT_fix1' on the DB. Mock returns one
    // known thread + one unknown thread. The unknown one should be silently
    // skipped and the known one's comments written.
    let fixture = r#"
    {
        "data": {
            "repository": {
                "pullRequest": {
                    "reviewThreads": {
                        "pageInfo": { "hasNextPage": false, "endCursor": null },
                        "nodes": [
                            {
                                "id": "PRRT_fix1",
                                "comments": {
                                    "pageInfo": { "hasNextPage": false, "endCursor": null },
                                    "nodes": [
                                        {
                                            "id": "PRRC_known",
                                            "databaseId": 1,
                                            "author": { "login": "alice" },
                                            "body": "kept",
                                            "bodyText": "kept",
                                            "createdAt": "2026-05-19T10:00:00Z"
                                        }
                                    ]
                                }
                            },
                            {
                                "id": "PRRT_phantom",
                                "comments": {
                                    "pageInfo": { "hasNextPage": false, "endCursor": null },
                                    "nodes": [
                                        {
                                            "id": "PRRC_lost",
                                            "databaseId": 2,
                                            "author": { "login": "bob" },
                                            "body": "dropped",
                                            "bodyText": "dropped",
                                            "createdAt": "2026-05-19T10:01:00Z"
                                        }
                                    ]
                                }
                            }
                        ]
                    },
                    "issueComments": {
                        "pageInfo": { "hasNextPage": false, "endCursor": null },
                        "nodes": []
                    }
                }
            }
        }
    }
    "#;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let db = fresh_db();
    seed_pr_with_thread(&db);
    let client = client_against(&server).await;

    commands_testing::fetch(&db, &client, 100, "owner", "repo", 42, 1)
        .await
        .expect("hydrator");

    let conn = db.lock().unwrap();
    let kept: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM review_comments WHERE node_id = 'PRRC_known'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let lost: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM review_comments WHERE node_id = 'PRRC_lost'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1);
    assert_eq!(lost, 0);
}
