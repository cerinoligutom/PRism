//! PAT validation against `GET /user`.
//!
//! Runs once at account-add time so the token is only ever stored if it
//! actually works. Returns the GitHub login, the granted scopes, and the
//! token expiry — everything the metadata store needs.
//!
//! `check_permissions` runs only from the onboarding "Validate" flow and
//! verifies each required PRism permission against GitHub. Only classic
//! PATs can be verified — the `x-oauth-scopes` header lists granted
//! scopes verbatim. Fine-grained PATs deliberately return Unknown across
//! the board because GitHub doesn't expose granted permissions through
//! any documented API and resource-endpoint probing is unreliable (the
//! permission check is bypassed for public-repo data). The UI treats
//! Unknown rows as informational so Connect for fine-grained PATs gates
//! only on token validity, not on permission verification.

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde::Serialize;
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
/// Classic PATs (which always populate `x-oauth-scopes`) are derived from
/// the scope set — no extra network calls. Fine-grained PATs return all
/// Unknown: GitHub publishes no introspection endpoint for them, and
/// probing resource endpoints is unreliable (public-repo data bypasses
/// the per-permission check). The frontend treats Unknown rows as
/// informational and doesn't gate Connect on them for fine-grained.
pub fn check_permissions(scopes: &[String]) -> PermissionChecks {
    if scopes.is_empty() {
        return fine_grained_unknown();
    }
    classic_checks(scopes)
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

fn fine_grained_unknown() -> PermissionChecks {
    PermissionChecks {
        contents: PermissionState::Unknown,
        pull_requests: PermissionState::Unknown,
        metadata: PermissionState::Unknown,
        members: PermissionState::Unknown,
    }
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

    #[test]
    fn classic_with_repo_and_read_org_grants_everything() {
        let checks = check_permissions(&s("repo read:org read:user"));
        assert_eq!(checks.contents, PermissionState::Granted);
        assert_eq!(checks.pull_requests, PermissionState::Granted);
        assert_eq!(checks.metadata, PermissionState::Granted);
        assert_eq!(checks.members, PermissionState::Granted);
    }

    #[test]
    fn classic_with_only_public_repo_still_grants_contents() {
        let checks = check_permissions(&s("public_repo"));
        assert_eq!(checks.contents, PermissionState::Granted);
        assert_eq!(checks.pull_requests, PermissionState::Granted);
        // read:org missing means members not granted.
        assert_eq!(checks.members, PermissionState::Missing);
    }

    #[test]
    fn classic_without_repo_marks_contents_missing() {
        let checks = check_permissions(&s("read:user read:org"));
        assert_eq!(checks.contents, PermissionState::Missing);
        assert_eq!(checks.pull_requests, PermissionState::Missing);
        assert_eq!(checks.metadata, PermissionState::Missing);
        assert_eq!(checks.members, PermissionState::Granted);
    }

    #[test]
    fn fine_grained_reports_all_unknown() {
        // Empty scopes ≡ fine-grained PAT. GitHub doesn't expose granted
        // permissions for these, so we deliberately return Unknown across
        // the board — the UI renders the rows as informational and
        // doesn't gate Connect on them.
        let checks = check_permissions(&[]);
        assert_eq!(checks.contents, PermissionState::Unknown);
        assert_eq!(checks.pull_requests, PermissionState::Unknown);
        assert_eq!(checks.metadata, PermissionState::Unknown);
        assert_eq!(checks.members, PermissionState::Unknown);
    }
}
