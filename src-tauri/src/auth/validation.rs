//! PAT validation against `GET /user`.
//!
//! Runs once at account-add time so the token is only ever stored if it
//! actually works. Returns the GitHub login, the granted scopes, and the
//! token expiry — everything the metadata store needs.
//!
//! `check_permissions` runs only from the onboarding "Validate" flow and
//! verifies each required PRism permission against GitHub. Classic PATs
//! are derived from the `x-oauth-scopes` header (cheap). Fine-grained PATs
//! are probed via a small set of representative endpoints — see the
//! `fine_grained_probes` doc comment for the rationale.

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Resolved identity for a PAT, written to the account store on success.
#[derive(Debug, Clone)]
pub struct ValidatedToken {
    pub login: String,
    pub scopes: Vec<String>,
    /// RFC-3339 expiry from the `github-authentication-token-expiration`
    /// response header, if GitHub sent one.
    pub expires_at: Option<String>,
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("token rejected by GitHub (401). Check that it has not expired or been revoked.")]
    Unauthorized,
    #[error("token lacks required permissions")]
    Forbidden,
    #[error("GitHub returned status {0}")]
    Unexpected(u16),
    #[error("network error reaching {host}: {message}")]
    Network { host: String, message: String },
}

#[derive(Debug, Deserialize)]
struct UserResponse {
    login: String,
}

/// Resolves the `/user` endpoint for either github.com or a GitHub
/// Enterprise host. Mirrors the routing GitHub's own clients use.
pub fn user_endpoint(host: &str) -> String {
    if host.eq_ignore_ascii_case("github.com") {
        "https://api.github.com/user".into()
    } else {
        // GHE: https://<host>/api/v3/user
        let trimmed = host.trim_end_matches('/');
        format!("https://{trimmed}/api/v3/user")
    }
}

const USER_AGENT: &str = "PRism/0.1 (+https://github.com/cerinoligutom/PRism)";

/// Calls `GET /user` against the given host with the supplied PAT.
///
/// Surfaces `Unauthorized` on 401 (the contract the sync layer relies on
/// to map "expired token" to UI state), and `Forbidden` on 403 — neither
/// gets reported to the caller as "network error". Other non-2xx statuses
/// surface as `Unexpected(status)`.
pub async fn validate_token(
    host: &str,
    token: &SecretString,
) -> Result<ValidatedToken, ValidationError> {
    let url = user_endpoint(host);
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| ValidationError::Network {
            host: host.into(),
            message: e.to_string(),
        })?;

    let response = client
        .get(&url)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", token.expose_secret()),
        )
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| ValidationError::Network {
            host: host.into(),
            message: e.to_string(),
        })?;

    let status = response.status();
    match status.as_u16() {
        200 => {
            let scopes = read_scopes(response.headers());
            let expires_at = read_expiry(response.headers());
            let body: UserResponse =
                response
                    .json()
                    .await
                    .map_err(|e| ValidationError::Network {
                        host: host.into(),
                        message: e.to_string(),
                    })?;
            Ok(ValidatedToken {
                login: body.login,
                scopes,
                expires_at,
            })
        }
        401 => Err(ValidationError::Unauthorized),
        403 => Err(ValidationError::Forbidden),
        other => Err(ValidationError::Unexpected(other)),
    }
}

fn read_scopes(headers: &reqwest::header::HeaderMap) -> Vec<String> {
    headers
        .get("x-oauth-scopes")
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(',')
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn read_expiry(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get("github-authentication-token-expiration")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// Per-permission grant state surfaced to the onboarding UI so it can show
/// each row as pending / granted / missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Missing,
    /// We weren't able to verify the permission — e.g. a fine-grained PAT
    /// with no repositories selected leaves us nothing to probe, and the
    /// Members permission requires knowing an org the user belongs to.
    /// Unknown rows do not block Connect; the UI flags them so the user
    /// can verify manually.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct PermissionChecks {
    pub contents: PermissionState,
    pub pull_requests: PermissionState,
    pub metadata: PermissionState,
    pub members: PermissionState,
}

/// Returns the per-permission grant state for a validated token.
///
/// Branches on whether `x-oauth-scopes` was populated: classic PATs always
/// set it, fine-grained PATs never do. Classic tokens are derived from the
/// scope set (no extra network calls). Fine-grained tokens are probed
/// against representative endpoints, so this is up to 3 extra requests.
pub async fn check_permissions(
    host: &str,
    token: &SecretString,
    scopes: &[String],
) -> Result<PermissionChecks, ValidationError> {
    if !scopes.is_empty() {
        return Ok(classic_checks(scopes));
    }
    let base = api_base(host);
    fine_grained_probes(&base, token).await
}

fn classic_checks(scopes: &[String]) -> PermissionChecks {
    let has = |s: &str| scopes.iter().any(|x| x == s);
    // `repo` is the umbrella scope PRism's create-token URL pre-fills; it
    // implicitly covers contents, pull_requests, and metadata for both
    // public and private repos. `public_repo` is the public-only subset.
    let has_repo = has("repo") || has("public_repo");
    // `read:org` is the minimum org membership read scope. The fuller
    // `admin:org` / `write:org` also grant it.
    let has_org = has("read:org") || has("admin:org") || has("write:org");
    let repo_state = if has_repo {
        PermissionState::Granted
    } else {
        PermissionState::Missing
    };
    PermissionChecks {
        contents: repo_state,
        pull_requests: repo_state,
        metadata: repo_state,
        members: if has_org {
            PermissionState::Granted
        } else {
            PermissionState::Missing
        },
    }
}

/// Fine-grained PATs don't expose granted permissions in a single header,
/// so we infer them from the response codes of representative endpoints.
///
/// 1. `/user/repos?per_page=1` — confirms Metadata. The response also gives
///    us a sample repository full_name we can target for the next two
///    probes; without one, Contents and Pull requests stay Unknown.
/// 2. `/repos/{full_name}/contents` — confirms Contents.
/// 3. `/repos/{full_name}/pulls?per_page=1&state=all` — confirms Pull requests.
///
/// Members is left Unknown for fine-grained: probing it requires picking an
/// org the PAT might be scoped to, which we can't know without the user
/// telling us. The UI marks Unknown rows as "verify manually" rather than
/// blocking Connect.
async fn fine_grained_probes(
    base: &str,
    token: &SecretString,
) -> Result<PermissionChecks, ValidationError> {
    let client = build_probe_client()?;

    let (metadata, sample_repo) = probe_user_repos(&client, base, token).await?;
    if matches!(metadata, PermissionState::Missing) {
        // Metadata is the umbrella prerequisite for the other two reads;
        // if it's denied, none of the rest can succeed either.
        return Ok(PermissionChecks {
            contents: PermissionState::Missing,
            pull_requests: PermissionState::Missing,
            metadata,
            members: PermissionState::Unknown,
        });
    }

    let (contents, pull_requests) = match sample_repo {
        Some(repo) => {
            let c = probe_repo_endpoint(&client, base, token, &repo, "contents").await;
            let p = probe_repo_endpoint(&client, base, token, &repo, "pulls?per_page=1&state=all")
                .await;
            (c, p)
        }
        None => (PermissionState::Unknown, PermissionState::Unknown),
    };

    Ok(PermissionChecks {
        contents,
        pull_requests,
        metadata,
        members: PermissionState::Unknown,
    })
}

fn build_probe_client() -> Result<reqwest::Client, ValidationError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| ValidationError::Network {
            host: String::new(),
            message: e.to_string(),
        })
}

fn api_base(host: &str) -> String {
    if host.eq_ignore_ascii_case("github.com") {
        "https://api.github.com".into()
    } else {
        let trimmed = host.trim_end_matches('/');
        format!("https://{trimmed}/api/v3")
    }
}

#[derive(Debug, Deserialize)]
struct RepoListItem {
    full_name: String,
}

async fn probe_user_repos(
    client: &reqwest::Client,
    base: &str,
    token: &SecretString,
) -> Result<(PermissionState, Option<String>), ValidationError> {
    let url = format!("{base}/user/repos?per_page=1");
    // Treat transient network errors during probing as Unknown rather than
    // propagating — by this point we already know the token authenticates,
    // so failing the whole validation on a flaky probe would mis-blame the
    // token.
    let response = match send_probe(client, &url, token).await {
        Ok(r) => r,
        Err(_) => return Ok((PermissionState::Unknown, None)),
    };
    let status = response.status().as_u16();
    match status {
        200 => match response.json::<Vec<RepoListItem>>().await {
            Ok(repos) => {
                let sample = repos.into_iter().next().map(|r| r.full_name);
                Ok((PermissionState::Granted, sample))
            }
            // 200 with an unparseable body means the endpoint did respond
            // (so metadata is granted) — we just can't fish out a sample.
            Err(_) => Ok((PermissionState::Granted, None)),
        },
        403 | 404 => Ok((PermissionState::Missing, None)),
        _ => Ok((PermissionState::Unknown, None)),
    }
}

async fn probe_repo_endpoint(
    client: &reqwest::Client,
    base: &str,
    token: &SecretString,
    repo_full_name: &str,
    suffix: &str,
) -> PermissionState {
    let url = format!("{base}/repos/{repo_full_name}/{suffix}");
    match send_probe(client, &url, token).await {
        Ok(response) => match response.status().as_u16() {
            // Both 200 (full payload) and 301 (repo moved) imply the
            // permission was honoured. 404 on a repo we just listed
            // shouldn't happen, but treat it as Unknown rather than
            // claiming missing.
            200 | 301 => PermissionState::Granted,
            403 => PermissionState::Missing,
            _ => PermissionState::Unknown,
        },
        Err(_) => PermissionState::Unknown,
    }
}

async fn send_probe(
    client: &reqwest::Client,
    url: &str,
    token: &SecretString,
) -> Result<reqwest::Response, ValidationError> {
    client
        .get(url)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", token.expose_secret()),
        )
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| ValidationError::Network {
            host: url.into(),
            message: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn server_host(server: &MockServer) -> String {
        // wiremock binds to `127.0.0.1:<port>` — strip the scheme so the host
        // string flows through user_endpoint() the same shape a GHE host would.
        server
            .uri()
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .to_string()
    }

    /// `user_endpoint` is what the production code calls — but it forces HTTPS,
    /// and wiremock serves HTTP. The helper here lets the tests target the
    /// mock server by overriding the path the same code path uses.
    async fn validate_against(
        server: &MockServer,
        token: &str,
    ) -> Result<ValidatedToken, ValidationError> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .unwrap();
        let url = format!("{}/api/v3/user", server.uri());
        let response = client
            .get(&url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| ValidationError::Network {
                host: server_host(server),
                message: e.to_string(),
            })?;

        let status = response.status();
        match status.as_u16() {
            200 => {
                let scopes = read_scopes(response.headers());
                let expires_at = read_expiry(response.headers());
                let body: UserResponse = response.json().await.unwrap();
                Ok(ValidatedToken {
                    login: body.login,
                    scopes,
                    expires_at,
                })
            }
            401 => Err(ValidationError::Unauthorized),
            403 => Err(ValidationError::Forbidden),
            other => Err(ValidationError::Unexpected(other)),
        }
    }

    #[test]
    fn user_endpoint_routes_dotcom_to_api_subdomain() {
        assert_eq!(user_endpoint("github.com"), "https://api.github.com/user");
    }

    #[test]
    fn user_endpoint_routes_enterprise_to_api_v3() {
        assert_eq!(
            user_endpoint("github.acme.corp"),
            "https://github.acme.corp/api/v3/user"
        );
    }

    #[tokio::test]
    async fn validation_succeeds_on_200_and_parses_scopes_and_expiry() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/user"))
            .and(header("authorization", "Bearer good-token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-oauth-scopes", "repo, read:org, read:user")
                    .insert_header(
                        "github-authentication-token-expiration",
                        "2026-12-01 00:00:00 UTC",
                    )
                    .set_body_json(serde_json::json!({ "login": "ada" })),
            )
            .mount(&server)
            .await;

        let token = SecretString::from("good-token");
        let got = validate_against(&server, token.expose_secret())
            .await
            .unwrap();
        assert_eq!(got.login, "ada");
        assert_eq!(got.scopes, vec!["repo", "read:org", "read:user"]);
        assert_eq!(got.expires_at.as_deref(), Some("2026-12-01 00:00:00 UTC"));
    }

    #[tokio::test]
    async fn validation_returns_unauthorized_on_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/user"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let err = validate_against(&server, "bad").await.expect_err("401");
        assert!(matches!(err, ValidationError::Unauthorized));
    }

    #[tokio::test]
    async fn validation_returns_forbidden_on_403() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/user"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let err = validate_against(&server, "scoped-out")
            .await
            .expect_err("403");
        assert!(matches!(err, ValidationError::Forbidden));
    }

    #[tokio::test]
    async fn validation_with_no_scopes_header_returns_empty_vec() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/user"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "login": "ada" })),
            )
            .mount(&server)
            .await;

        let got = validate_against(&server, "tok").await.unwrap();
        assert!(got.scopes.is_empty());
        assert!(got.expires_at.is_none());
    }

    // ────── Permission checks ──────

    fn s(v: &str) -> Vec<String> {
        v.split_whitespace().map(|t| t.to_string()).collect()
    }

    #[tokio::test]
    async fn classic_with_repo_and_read_org_grants_everything() {
        let token = SecretString::from("ignored-for-classic");
        let checks = check_permissions("github.com", &token, &s("repo read:org read:user"))
            .await
            .unwrap();
        assert_eq!(checks.contents, PermissionState::Granted);
        assert_eq!(checks.pull_requests, PermissionState::Granted);
        assert_eq!(checks.metadata, PermissionState::Granted);
        assert_eq!(checks.members, PermissionState::Granted);
    }

    #[tokio::test]
    async fn classic_with_only_public_repo_still_grants_contents() {
        let token = SecretString::from("ignored");
        let checks = check_permissions("github.com", &token, &s("public_repo"))
            .await
            .unwrap();
        assert_eq!(checks.contents, PermissionState::Granted);
        assert_eq!(checks.pull_requests, PermissionState::Granted);
        // read:org missing means members not granted.
        assert_eq!(checks.members, PermissionState::Missing);
    }

    #[tokio::test]
    async fn classic_without_repo_marks_contents_missing() {
        let token = SecretString::from("ignored");
        let checks = check_permissions("github.com", &token, &s("read:user"))
            .await
            .unwrap();
        assert_eq!(checks.contents, PermissionState::Missing);
        assert_eq!(checks.pull_requests, PermissionState::Missing);
        assert_eq!(checks.metadata, PermissionState::Missing);
    }

    #[tokio::test]
    async fn fine_grained_all_granted_with_sample_repo() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/user/repos"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{ "full_name": "ada/diffs" }])),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/repos/ada/diffs/contents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/repos/ada/diffs/pulls"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let token = SecretString::from("fg-token");
        let base = format!("{}/api/v3", server.uri());
        let checks = fine_grained_probes(&base, &token).await.unwrap();
        assert_eq!(checks.contents, PermissionState::Granted);
        assert_eq!(checks.pull_requests, PermissionState::Granted);
        assert_eq!(checks.metadata, PermissionState::Granted);
        assert_eq!(checks.members, PermissionState::Unknown);
    }

    #[tokio::test]
    async fn fine_grained_metadata_denied_short_circuits_everything() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/user/repos"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let token = SecretString::from("fg-token");
        let base = format!("{}/api/v3", server.uri());
        let checks = fine_grained_probes(&base, &token).await.unwrap();
        assert_eq!(checks.metadata, PermissionState::Missing);
        assert_eq!(checks.contents, PermissionState::Missing);
        assert_eq!(checks.pull_requests, PermissionState::Missing);
    }

    #[tokio::test]
    async fn fine_grained_empty_repo_list_leaves_contents_and_pr_unknown() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/user/repos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let token = SecretString::from("fg-token");
        let base = format!("{}/api/v3", server.uri());
        let checks = fine_grained_probes(&base, &token).await.unwrap();
        assert_eq!(checks.metadata, PermissionState::Granted);
        assert_eq!(checks.contents, PermissionState::Unknown);
        assert_eq!(checks.pull_requests, PermissionState::Unknown);
    }

    #[tokio::test]
    async fn fine_grained_partial_grant_marks_pr_missing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/user/repos"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{ "full_name": "ada/diffs" }])),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/repos/ada/diffs/contents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/repos/ada/diffs/pulls"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let token = SecretString::from("fg-token");
        let base = format!("{}/api/v3", server.uri());
        let checks = fine_grained_probes(&base, &token).await.unwrap();
        assert_eq!(checks.contents, PermissionState::Granted);
        assert_eq!(checks.pull_requests, PermissionState::Missing);
        assert_eq!(checks.metadata, PermissionState::Granted);
    }
}
