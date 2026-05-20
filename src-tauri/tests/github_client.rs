//! Integration tests for the shared GitHub client.
//!
//! The wiremock server stands in for `api.github.com`. We point the client at
//! `mock.uri()` via the builder's URL overrides; nothing in production code
//! hardcodes the host.

use std::sync::Arc;

use prism_lib::github::graphql::PrCoord;
use prism_lib::github::{
    AccountHandle, Conditional, EtagStore, GitHubClient, GitHubError, InMemoryEtagStore,
    StaticTokenSource,
};
use serde_json::json;
use url::Url;
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PR_DETAIL_FIXTURE: &str = include_str!("fixtures/pr_detail.json");
const PR_TIMELINE_PAGE_1: &str = include_str!("fixtures/pr_timeline_page1.json");
const PR_TIMELINE_PAGE_2: &str = include_str!("fixtures/pr_timeline_page2.json");
const GRAPHQL_ERRORS: &str = include_str!("fixtures/graphql_errors.json");

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
