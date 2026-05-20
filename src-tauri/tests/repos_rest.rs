//! Integration tests for the `/user/repos` REST wrapper used by Settings ->
//! Repositories (M2-D).
//!
//! The wiremock server stands in for `api.github.com`; the client is pointed
//! at `mock.uri()` via the builder's URL override.

use std::sync::Arc;

use prism_lib::github::{
    list_user_repos, AccountHandle, EtagStore, GitHubClient, GitHubError, InMemoryEtagStore,
    ListRepos, StaticTokenSource,
};
use url::Url;
use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PAGE_FIXTURE: &str = r#"[
    {
        "id": 1,
        "name": "alpha",
        "owner": { "login": "ada" },
        "visibility": "public"
    },
    {
        "id": 2,
        "name": "beta",
        "owner": { "login": "ada" },
        "visibility": "private"
    }
]"#;

async fn client_against(server: &MockServer, etags: Arc<dyn EtagStore>) -> GitHubClient {
    let base = Url::parse(&server.uri()).unwrap();
    GitHubClient::builder()
        .account(AccountHandle::new(1, "github.com", "tester"))
        .token_source(Arc::new(StaticTokenSource::new("ghp_test_pat")))
        .etag_store(etags)
        .base_rest_url(base.join("/").unwrap())
        .base_graphql_url(base.join("/graphql").unwrap())
        .build()
        .unwrap()
}

#[tokio::test]
async fn list_user_repos_returns_single_page_when_no_link_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("per_page", "100"))
        .and(query_param(
            "affiliation",
            "owner,collaborator,organization_member",
        ))
        // wiremock decodes query params before matching, so the matcher sees
        // the comma-separated form even though the wire sends `%2C`.
        .and(header("authorization", "Bearer ghp_test_pat"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "W/\"repos-v1\"")
                .insert_header("x-ratelimit-limit", "5000")
                .insert_header("x-ratelimit-remaining", "4999")
                .set_body_raw(PAGE_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let etags: Arc<dyn EtagStore> = Arc::new(InMemoryEtagStore::new());
    let client = client_against(&server, etags).await;
    let result = list_user_repos(&client).await.unwrap();

    let repos = match result {
        ListRepos::Repos(r) => r,
        ListRepos::NotModified => panic!("expected Repos, got NotModified"),
    };

    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0].name, "alpha");
    assert_eq!(repos[0].owner.login, "ada");
    assert_eq!(repos[0].visibility, "public");
    assert_eq!(repos[1].name, "beta");
    assert_eq!(repos[1].visibility, "private");
}

#[tokio::test]
async fn list_user_repos_walks_link_rel_next_pagination() {
    let server = MockServer::start().await;
    // Real GitHub echoes the request's encoded form back in the Link header.
    // `parse_next_link` splits on `,` so commas inside the URL must stay
    // encoded as `%2C`.
    let next_link = format!(
        "<{}/user/repos?affiliation=owner%2Ccollaborator%2Corganization_member&per_page=100&page=2>; rel=\"next\", \
         <{}/user/repos?affiliation=owner%2Ccollaborator%2Corganization_member&per_page=100&page=2>; rel=\"last\"",
        server.uri(),
        server.uri(),
    );

    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("per_page", "100"))
        .and(query_param_is_missing("page"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", next_link.as_str())
                .set_body_raw(PAGE_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let page_two = r#"[
        {
            "id": 3,
            "name": "gamma",
            "owner": { "login": "bob" },
            "visibility": "public"
        }
    ]"#;

    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(page_two.as_bytes().to_vec(), "application/json"),
        )
        .mount(&server)
        .await;

    let etags: Arc<dyn EtagStore> = Arc::new(InMemoryEtagStore::new());
    let client = client_against(&server, etags).await;
    let result = list_user_repos(&client).await.unwrap();

    let repos = match result {
        ListRepos::Repos(r) => r,
        ListRepos::NotModified => panic!("expected Repos, got NotModified"),
    };

    let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "beta", "gamma"]);
}

#[tokio::test]
async fn list_user_repos_returns_not_modified_on_304_first_page() {
    let server = MockServer::start().await;
    let etags = Arc::new(InMemoryEtagStore::new());

    // First call: returns 200 + ETag, populates the store.
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param("per_page", "100"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "W/\"repos-v1\"")
                .set_body_raw(PAGE_FIXTURE.as_bytes().to_vec(), "application/json"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;

    // Second call (after If-None-Match): returns 304.
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(header("if-none-match", "W/\"repos-v1\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;

    let client = client_against(&server, etags.clone() as Arc<dyn EtagStore>).await;

    let first = list_user_repos(&client).await.unwrap();
    assert!(first.is_modified());

    let second = list_user_repos(&client).await.unwrap();
    assert!(matches!(second, ListRepos::NotModified));
}

#[tokio::test]
async fn list_user_repos_401_maps_to_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let etags: Arc<dyn EtagStore> = Arc::new(InMemoryEtagStore::new());
    let client = client_against(&server, etags).await;
    let err = list_user_repos(&client).await.unwrap_err();
    assert!(matches!(err, GitHubError::Unauthorized));
}

#[tokio::test]
async fn list_user_repos_403_with_retry_after_maps_to_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .respond_with(ResponseTemplate::new(403).insert_header("retry-after", "60"))
        .mount(&server)
        .await;

    let etags: Arc<dyn EtagStore> = Arc::new(InMemoryEtagStore::new());
    let client = client_against(&server, etags).await;
    let err = list_user_repos(&client).await.unwrap_err();
    match err {
        GitHubError::RateLimited { retry_after } => {
            assert_eq!(retry_after, Some(std::time::Duration::from_secs(60)));
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn list_user_repos_5xx_maps_to_server_variant() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;

    let etags: Arc<dyn EtagStore> = Arc::new(InMemoryEtagStore::new());
    let client = client_against(&server, etags).await;
    let err = list_user_repos(&client).await.unwrap_err();
    assert!(matches!(err, GitHubError::Server { status: 502 }));
}

#[tokio::test]
async fn list_user_repos_truncates_at_safety_cap() {
    use prism_lib::github::MAX_REPOS_PER_REFRESH;

    // Synthesise 6 pages of 100 repos each. The cap is 500, so we should see
    // page 1..=5 fetched and the result truncated at 500. Page 6 is not
    // mocked: if the loop tries to fetch it the test would fail.
    let server = MockServer::start().await;

    let make_page = |start_id: i64| -> String {
        let mut entries = Vec::with_capacity(100);
        for i in 0..100 {
            entries.push(format!(
                r#"{{ "id": {id}, "name": "r{id}", "owner": {{ "login": "ada" }}, "visibility": "public" }}"#,
                id = start_id + i
            ));
        }
        format!("[{}]", entries.join(","))
    };

    let link_for_page = |next: i64| -> String {
        format!(
            "<{}/user/repos?affiliation=owner%2Ccollaborator%2Corganization_member&per_page=100&page={next}>; rel=\"next\"",
            server.uri(),
        )
    };

    // Page 1 (no `page` param).
    Mock::given(method("GET"))
        .and(path("/user/repos"))
        .and(query_param_is_missing("page"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("link", link_for_page(2).as_str())
                .set_body_raw(make_page(1).into_bytes(), "application/json"),
        )
        .mount(&server)
        .await;

    for page in 2..=5 {
        let body = make_page((page - 1) * 100 + 1);
        let link = if page < 5 {
            Some(link_for_page(page + 1))
        } else {
            None
        };
        let mut tmpl =
            ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "application/json");
        if let Some(l) = link {
            tmpl = tmpl.insert_header("link", l.as_str());
        }
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .and(query_param("page", page.to_string()))
            .respond_with(tmpl)
            .mount(&server)
            .await;
    }

    let etags: Arc<dyn EtagStore> = Arc::new(InMemoryEtagStore::new());
    let client = client_against(&server, etags).await;
    let result = list_user_repos(&client).await.unwrap();

    let repos = match result {
        ListRepos::Repos(r) => r,
        ListRepos::NotModified => panic!("expected Repos"),
    };
    assert_eq!(repos.len(), MAX_REPOS_PER_REFRESH);
}
