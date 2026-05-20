//! Integration tests for the shared GitHub client.
//!
//! The wiremock server stands in for `api.github.com`. We point the client at
//! `mock.uri()` via the builder's URL overrides; nothing in production code
//! hardcodes the host.

use std::sync::Arc;

use prism_lib::github::graphql::PrCoord;
use prism_lib::github::{
    list_pr_timeline, AccountHandle, Conditional, EtagStore, GitHubClient, GitHubError,
    InMemoryEtagStore, ListTimeline, RepoCoord, StaticTokenSource,
};
use prism_lib::sync::{latest_status_change, QualifyingEvent};
use serde_json::json;
use time::macros::datetime;
use url::Url;
use wiremock::matchers::{header, header_exists, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PR_DETAIL_FIXTURE: &str = include_str!("fixtures/pr_detail.json");
const PR_TIMELINE_PAGE_1: &str = include_str!("fixtures/pr_timeline_page1.json");
const PR_TIMELINE_PAGE_2: &str = include_str!("fixtures/pr_timeline_page2.json");
const GRAPHQL_ERRORS: &str = include_str!("fixtures/graphql_errors.json");
const REST_TIMELINE_FIXTURE: &str = include_str!("fixtures/timeline_full_lifecycle.json");

async fn client_against(server: &MockServer) -> GitHubClient {
    let base = Url::parse(&server.uri()).unwrap();
    let rest = base.join("/").unwrap();
    let graphql = base.join("/graphql").unwrap();
    GitHubClient::builder()
        .account(AccountHandle::new(1, "github.com", "tester"))
        .token_source(Arc::new(StaticTokenSource::new("ghp_test_pat")))
        .etag_store(Arc::new(InMemoryEtagStore::new()))
        .base_rest_url(rest)
        .base_graphql_url(graphql)
        .build()
        .unwrap()
}

#[tokio::test]
async fn graphql_pr_detail_returns_resolved_threads() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(header("authorization", "Bearer ghp_test_pat"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-ratelimit-limit", "5000")
                .insert_header("x-ratelimit-remaining", "4999")
                .insert_header("x-ratelimit-used", "1")
                .insert_header("x-ratelimit-reset", "9999999999")
                .set_body_raw(PR_DETAIL_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let client = client_against(&server).await;
    let pr = client
        .pr_detail(PrCoord {
            owner: "owner",
            name: "repo",
            number: 42,
        })
        .await
        .unwrap()
        .expect("pull request");

    assert_eq!(pr.number, 42);
    assert_eq!(pr.review_threads.nodes.len(), 2);
    assert!(pr.review_threads.nodes[0].is_resolved);
    assert!(!pr.review_threads.nodes[1].is_resolved);

    let snapshot = client.rate().snapshot();
    assert_eq!(snapshot.remaining, 4999);
    assert_eq!(snapshot.limit, 5000);
}

#[tokio::test]
async fn graphql_response_with_errors_surfaces_as_graphql_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(GRAPHQL_ERRORS.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let client = client_against(&server).await;
    let err = client
        .pr_detail(PrCoord {
            owner: "owner",
            name: "repo",
            number: 1,
        })
        .await
        .unwrap_err();

    match err {
        GitHubError::Graphql(errors) => {
            assert_eq!(errors.len(), 1);
            assert!(errors[0].message.contains("doesn't exist"));
        }
        other => panic!("expected Graphql error, got {other:?}"),
    }
}

#[tokio::test]
async fn timeline_pagination_walks_until_no_next_page() {
    let server = MockServer::start().await;
    // First call: returns hasNextPage = true and a cursor.
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(wiremock::matchers::body_partial_json(json!({
            "variables": { "after": null }
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(PR_TIMELINE_PAGE_1.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;
    // Second call: cursor present, returns hasNextPage = false.
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(wiremock::matchers::body_partial_json(json!({
            "variables": { "after": "Y2lyY2xlMQ==" }
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(PR_TIMELINE_PAGE_2.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let client = client_against(&server).await;
    let events = client
        .pr_timeline_all(
            PrCoord {
                owner: "owner",
                name: "repo",
                number: 42,
            },
            10,
        )
        .await
        .unwrap();

    assert_eq!(events.len(), 4);
}

#[tokio::test]
async fn unauthorized_response_maps_to_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let client = client_against(&server).await;
    let err = client
        .pr_detail(PrCoord {
            owner: "owner",
            name: "repo",
            number: 1,
        })
        .await
        .unwrap_err();

    assert!(matches!(err, GitHubError::Unauthorized));
}

#[tokio::test]
async fn forbidden_with_retry_after_maps_to_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("retry-after", "120")
                .insert_header("x-ratelimit-remaining", "0"),
        )
        .mount(&server)
        .await;

    let client = client_against(&server).await;
    let err = client
        .pr_detail(PrCoord {
            owner: "owner",
            name: "repo",
            number: 1,
        })
        .await
        .unwrap_err();

    match err {
        GitHubError::RateLimited { retry_after } => {
            assert_eq!(retry_after, Some(std::time::Duration::from_secs(120)));
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
    assert_eq!(client.rate().snapshot().remaining, 0);
}

#[tokio::test]
async fn get_conditional_returns_not_modified_on_304() {
    let server = MockServer::start().await;
    let etags = Arc::new(InMemoryEtagStore::new());
    let base = Url::parse(&server.uri()).unwrap();

    // Pre-seed the ETag store so the request sends If-None-Match.
    etags.put(
        "1:GET:/repos/owner/repo/pulls/42",
        prism_lib::github::EtagEntry::new("W/\"deadbeef\""),
    );

    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/pulls/42"))
        .and(header("if-none-match", "W/\"deadbeef\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;

    let client = GitHubClient::builder()
        .account(AccountHandle::new(1, "github.com", "tester"))
        .token_source(Arc::new(StaticTokenSource::new("ghp_test_pat")))
        .etag_store(etags.clone() as Arc<dyn prism_lib::github::EtagStore>)
        .base_rest_url(base.join("/").unwrap())
        .base_graphql_url(base.join("/graphql").unwrap())
        .build()
        .unwrap();

    let result = client
        .get_conditional("/repos/owner/repo/pulls/42")
        .await
        .unwrap();
    assert!(matches!(result, Conditional::NotModified));
}

#[tokio::test]
async fn get_conditional_stores_new_etag_on_200() {
    let server = MockServer::start().await;
    let etags = Arc::new(InMemoryEtagStore::new());
    let base = Url::parse(&server.uri()).unwrap();

    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/pulls/42"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "W/\"freshfresh\"")
                .set_body_raw(b"{\"number\":42}".to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let client = GitHubClient::builder()
        .account(AccountHandle::new(1, "github.com", "tester"))
        .token_source(Arc::new(StaticTokenSource::new("ghp_test_pat")))
        .etag_store(etags.clone() as Arc<dyn prism_lib::github::EtagStore>)
        .base_rest_url(base.join("/").unwrap())
        .base_graphql_url(base.join("/graphql").unwrap())
        .build()
        .unwrap();

    let result = client
        .get_conditional("/repos/owner/repo/pulls/42")
        .await
        .unwrap();
    match result {
        Conditional::Modified { etag, body } => {
            assert_eq!(etag.as_deref(), Some("W/\"freshfresh\""));
            assert_eq!(&body[..], b"{\"number\":42}");
        }
        other => panic!("expected Modified, got {other:?}"),
    }

    let stored = etags.get("1:GET:/repos/owner/repo/pulls/42").unwrap();
    assert_eq!(stored.etag, "W/\"freshfresh\"");
    assert!(stored.body_sha256.is_some());
}

#[tokio::test]
async fn server_error_status_maps_to_server_variant() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let client = client_against(&server).await;
    let err = client
        .pr_detail(PrCoord {
            owner: "owner",
            name: "repo",
            number: 1,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, GitHubError::Server { status: 503 }));
}

#[tokio::test]
async fn authorization_header_is_sent_per_request() {
    // Sanity check that the auth header is being attached on every call.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(header_exists("authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PR_DETAIL_FIXTURE))
        .mount(&server)
        .await;

    let client = client_against(&server).await;
    let _ = client
        .pr_detail(PrCoord {
            owner: "owner",
            name: "repo",
            number: 42,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn rest_timeline_deserialises_full_lifecycle() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/42/timeline"))
        .and(query_param("per_page", "100"))
        .and(header("authorization", "Bearer ghp_test_pat"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "W/\"timeline-v1\"")
                .insert_header("x-ratelimit-limit", "5000")
                .insert_header("x-ratelimit-remaining", "4998")
                .set_body_raw(
                    REST_TIMELINE_FIXTURE.as_bytes().to_vec(),
                    "application/json",
                ),
        )
        .mount(&server)
        .await;

    let client = client_against(&server).await;
    let result = list_pr_timeline(
        &client,
        RepoCoord {
            owner: "owner",
            repo: "repo",
        },
        42,
        1,
    )
    .await
    .unwrap();

    let events = match result {
        ListTimeline::Events(e) => e,
        ListTimeline::NotModified => panic!("expected Events, got NotModified"),
    };

    // 11 input events; `committed`, `labeled`, `assigned` are dropped — leaves 8.
    let event_names: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(
        event_names,
        vec![
            "ready_for_review",
            "review_requested",
            "reviewed",
            "convert_to_draft",
            "ready_for_review",
            "merged",
            "closed",
            "reopened",
        ],
    );

    // The `reviewed` event must carry submitted_at, not a missing timestamp.
    let reviewed = events.iter().find(|e| e.event == "reviewed").unwrap();
    assert_eq!(reviewed.created_at, datetime!(2026-05-03 10:00:00 UTC));
}

#[tokio::test]
async fn rest_timeline_drives_latest_status_change_to_reopened() {
    // End-to-end: REST -> sync derivation. The fixture's last qualifying
    // event is `reopened` at 2026-05-07T08:30Z, so that must win.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/42/timeline"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            REST_TIMELINE_FIXTURE.as_bytes().to_vec(),
            "application/json",
        ))
        .mount(&server)
        .await;

    let client = client_against(&server).await;
    let events = match list_pr_timeline(
        &client,
        RepoCoord {
            owner: "owner",
            repo: "repo",
        },
        42,
        1,
    )
    .await
    .unwrap()
    {
        ListTimeline::Events(e) => e,
        ListTimeline::NotModified => unreachable!(),
    };

    let derived = latest_status_change(&events).expect("derived");
    assert_eq!(derived.event_type, QualifyingEvent::Reopened);
    assert_eq!(derived.at, datetime!(2026-05-07 08:30:00 UTC));
}

#[tokio::test]
async fn rest_timeline_stores_etag_then_returns_not_modified_on_304() {
    let server = MockServer::start().await;
    let etags = Arc::new(InMemoryEtagStore::new());
    let base = Url::parse(&server.uri()).unwrap();

    // First call returns 200 + ETag.
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/42/timeline"))
        .and(query_param("per_page", "100"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "W/\"timeline-rev-1\"")
                .set_body_raw(
                    REST_TIMELINE_FIXTURE.as_bytes().to_vec(),
                    "application/json",
                ),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Second call (after If-None-Match) returns 304.
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/42/timeline"))
        .and(header("if-none-match", "W/\"timeline-rev-1\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;

    let client = GitHubClient::builder()
        .account(AccountHandle::new(1, "github.com", "tester"))
        .token_source(Arc::new(StaticTokenSource::new("ghp_test_pat")))
        .etag_store(etags.clone() as Arc<dyn EtagStore>)
        .base_rest_url(base.join("/").unwrap())
        .base_graphql_url(base.join("/graphql").unwrap())
        .build()
        .unwrap();

    let repo = RepoCoord {
        owner: "owner",
        repo: "repo",
    };

    let first = list_pr_timeline(&client, repo, 42, 1).await.unwrap();
    assert!(first.is_modified());

    let second = list_pr_timeline(&client, repo, 42, 1).await.unwrap();
    assert!(matches!(second, ListTimeline::NotModified));

    let stored = etags
        .get("1:GET:/repos/owner/repo/issues/42/timeline?per_page=100")
        .expect("etag entry");
    assert_eq!(stored.etag, "W/\"timeline-rev-1\"");
}

#[tokio::test]
async fn rest_timeline_404_maps_to_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/missing/issues/42/timeline"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let client = client_against(&server).await;
    let err = list_pr_timeline(
        &client,
        RepoCoord {
            owner: "owner",
            repo: "missing",
        },
        42,
        1,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, GitHubError::NotFound));
}
