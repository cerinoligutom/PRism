//! Shared HTTP client for the GraphQL and REST surfaces.
//!
//! One `GitHubClient` per account. The struct holds a `reqwest::Client`, the
//! account context, the rate-limit budget, and an ETag store; it does **not**
//! hold the PAT (auth headers are injected per-request via the [`TokenSource`]
//! trait, so a leaked client never carries the secret).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::HeaderMap;
use reqwest::header::{HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::{Method, Response, StatusCode};
use secrecy::ExposeSecret;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use url::Url;

use crate::github::auth::{AccountHandle, TokenSource};
use crate::github::error::{GitHubError, GraphqlError};
use crate::github::etag::{graphql_key, rest_key, EtagEntry, EtagStore, InMemoryEtagStore};
use crate::github::rate_limit::RateBudget;

/// Default `User-Agent` sent with every request.
const DEFAULT_USER_AGENT: &str = concat!("PRism/", env!("CARGO_PKG_VERSION"));

/// Result of a conditional GET.
#[derive(Debug)]
pub enum Conditional<T> {
    /// Upstream returned 304 — the cached copy is still valid.
    NotModified,
    /// Upstream returned 200 (or another success) — fresh body, the new ETag
    /// if the server provided one, and the response headers (needed for
    /// `Link`-driven pagination on REST list endpoints, per RFC 5988).
    Modified {
        body: T,
        etag: Option<String>,
        headers: HeaderMap,
    },
}

impl<T> Conditional<T> {
    pub fn is_modified(&self) -> bool {
        matches!(self, Conditional::Modified { .. })
    }
}

/// Extract the URL targeted by `rel="next"` from an RFC 5988 `Link` header,
/// if present. Returns `None` when the header is missing or has no `next` rel.
pub fn parse_next_link(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(http::header::LINK)?.to_str().ok()?;
    for entry in raw.split(',') {
        let entry = entry.trim();
        let is_next = entry
            .split(';')
            .skip(1)
            .map(str::trim)
            .any(|p| p == "rel=\"next\"" || p == "rel=next");
        if !is_next {
            continue;
        }
        let start = entry.find('<')?;
        let end = entry[start + 1..].find('>')?;
        return Some(entry[start + 1..start + 1 + end].to_string());
    }
    None
}

/// Per-account HTTP entrypoint. Cheap to clone (everything inside is `Arc`).
pub struct GitHubClient {
    inner: reqwest::Client,
    account: AccountHandle,
    token_source: Arc<dyn TokenSource>,
    rate: Arc<RateBudget>,
    etags: Arc<dyn EtagStore>,
    base_rest_url: Url,
    base_graphql_url: Url,
}

impl std::fmt::Debug for GitHubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubClient")
            .field("account", &self.account)
            .field("base_rest_url", &self.base_rest_url.as_str())
            .field("base_graphql_url", &self.base_graphql_url.as_str())
            .finish_non_exhaustive()
    }
}

impl GitHubClient {
    pub fn builder() -> GitHubClientBuilder {
        GitHubClientBuilder::default()
    }

    pub fn account(&self) -> &AccountHandle {
        &self.account
    }

    pub fn rate(&self) -> &RateBudget {
        &self.rate
    }

    pub fn etags(&self) -> &Arc<dyn EtagStore> {
        &self.etags
    }

    /// Conditional GET against the REST surface.
    ///
    /// Sends `If-None-Match` if we have a stored ETag for `path`. A 304 response
    /// resolves to [`Conditional::NotModified`] without reading a body; a 200
    /// stores the new ETag (if any) and resolves to [`Conditional::Modified`].
    pub async fn get_conditional(&self, path: &str) -> Result<Conditional<Bytes>, GitHubError> {
        let url = self.base_rest_url.join(path.trim_start_matches('/'))?;
        let key = rest_key(self.account.id, "GET", path);
        let cached = self.etags.get(&key);

        let mut req = self
            .inner
            .request(Method::GET, url)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        req = self.attach_auth(req)?;
        if let Some(entry) = cached.as_ref() {
            req = req.header(http::header::IF_NONE_MATCH, entry.etag.as_str());
        }

        let response = req.send().await?;
        self.rate.update_from_headers(response.headers());

        match response.status() {
            StatusCode::NOT_MODIFIED => Ok(Conditional::NotModified),
            s if s.is_success() => {
                let etag = extract_etag(response.headers());
                let headers = response.headers().clone();
                let body = response.bytes().await?;
                if let Some(etag) = etag.clone() {
                    let entry = EtagEntry::new(etag).with_body_sha256(sha256(&body));
                    self.etags.put(&key, entry);
                }
                Ok(Conditional::Modified {
                    body,
                    etag,
                    headers,
                })
            }
            s => Err(map_error_status(s, response).await),
        }
    }

    /// POST a GraphQL query and deserialise the `data` payload into `T`.
    ///
    /// Errors in the GraphQL response's `errors` array surface as
    /// [`GitHubError::Graphql`] even if the HTTP status is 200.
    pub async fn post_graphql<T>(
        &self,
        query: &str,
        vars: serde_json::Value,
    ) -> Result<T, GitHubError>
    where
        T: DeserializeOwned,
    {
        let (data, _body) = self.post_graphql_with_raw(query, vars).await?;
        Ok(data)
    }

    /// Variant of [`post_graphql`] that returns the raw response bytes alongside
    /// the parsed payload. The body-hash cache (issue #234) uses these bytes as
    /// the canonical "did the upstream answer change since last poll" signal
    /// before deciding whether to run the per-node DB writes.
    pub async fn post_graphql_with_raw<T>(
        &self,
        query: &str,
        vars: serde_json::Value,
    ) -> Result<(T, Bytes), GitHubError>
    where
        T: DeserializeOwned,
    {
        let payload = serde_json::json!({
            "query": query,
            "variables": vars,
        });

        let mut req = self
            .inner
            .request(Method::POST, self.base_graphql_url.clone())
            .header(ACCEPT, "application/json")
            .json(&payload);
        req = self.attach_auth(req)?;

        let response = req.send().await?;
        self.rate.update_from_headers(response.headers());

        let status = response.status();
        if !status.is_success() {
            return Err(map_error_status(status, response).await);
        }

        let body = response.bytes().await?;
        let envelope: GraphqlEnvelope<T> = serde_json::from_slice(&body)?;
        if let Some(errors) = envelope.errors {
            if !errors.is_empty() {
                return Err(GitHubError::Graphql(errors));
            }
        }
        let data = envelope.data.ok_or_else(|| {
            GitHubError::Graphql(vec![GraphqlError {
                message: "graphql response had neither data nor errors".to_string(),
                path: None,
                kind: None,
            }])
        })?;
        Ok((data, body))
    }

    /// Cache the body of a GraphQL response keyed by `query_hash`.
    ///
    /// The GraphQL endpoint doesn't honour `If-None-Match`; the worker uses
    /// this as a "did the canonical query body change since last poll" check.
    pub fn cache_graphql_body(&self, query_hash: &str, body: &[u8]) {
        let key = graphql_key(self.account.id, query_hash);
        let entry =
            EtagEntry::new(format!("sha256:{}", hex(sha256(body)))).with_body_sha256(sha256(body));
        self.etags.put(&key, entry);
    }

    /// Look up the previously-cached GraphQL response metadata.
    pub fn graphql_cache_entry(&self, query_hash: &str) -> Option<EtagEntry> {
        self.etags.get(&graphql_key(self.account.id, query_hash))
    }

    /// Compare `body` against the cached SHA for `query_hash`. Writes the new
    /// hash back on miss. Returns `true` when the body is byte-identical to
    /// the previous cycle's response (a 304-equivalent skip per ADR 0004).
    pub fn graphql_body_unchanged(&self, query_hash: &str, body: &[u8]) -> bool {
        let new_sha = sha256(body);
        let matched = self
            .graphql_cache_entry(query_hash)
            .and_then(|e| e.body_sha256)
            .is_some_and(|prev| prev == new_sha);
        if !matched {
            self.cache_graphql_body(query_hash, body);
        }
        matched
    }

    fn attach_auth(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, GitHubError> {
        let secret = self.token_source.token(&self.account)?;
        let header_value = format!("Bearer {}", secret.expose_secret());
        let mut hv = HeaderValue::from_str(&header_value)
            .map_err(|e| GitHubError::InvalidHeader(e.to_string()))?;
        hv.set_sensitive(true);
        Ok(req.header(AUTHORIZATION, hv))
    }
}

/// GraphQL response envelope. `data` is generic so callers pick the shape.
#[derive(Debug, serde::Deserialize)]
struct GraphqlEnvelope<T> {
    #[serde(default = "default_none::<T>")]
    data: Option<T>,
    #[serde(default)]
    errors: Option<Vec<GraphqlError>>,
}

fn default_none<T>() -> Option<T> {
    None
}

async fn map_error_status(status: StatusCode, response: Response) -> GitHubError {
    match status {
        StatusCode::UNAUTHORIZED => GitHubError::Unauthorized,
        StatusCode::NOT_FOUND => GitHubError::NotFound,
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
            let retry_after = parse_retry_after(response.headers());
            GitHubError::RateLimited { retry_after }
        }
        s if s.is_server_error() => GitHubError::Server { status: s.as_u16() },
        s => GitHubError::Server { status: s.as_u16() },
    }
}

fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn extract_etag(headers: &HeaderMap) -> Option<String> {
    headers
        .get(http::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

pub(crate) fn hex(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

/// Fluent builder for [`GitHubClient`].
pub struct GitHubClientBuilder {
    account: Option<AccountHandle>,
    token_source: Option<Arc<dyn TokenSource>>,
    etags: Option<Arc<dyn EtagStore>>,
    rate: Option<Arc<RateBudget>>,
    user_agent: Option<String>,
    base_rest_url: Option<Url>,
    base_graphql_url: Option<Url>,
    extra_headers: Vec<(HeaderName, HeaderValue)>,
    timeout: Option<Duration>,
}

impl Default for GitHubClientBuilder {
    fn default() -> Self {
        Self {
            account: None,
            token_source: None,
            etags: None,
            rate: None,
            user_agent: None,
            base_rest_url: None,
            base_graphql_url: None,
            extra_headers: Vec::new(),
            timeout: Some(Duration::from_secs(30)),
        }
    }
}

impl GitHubClientBuilder {
    pub fn account(mut self, account: AccountHandle) -> Self {
        self.account = Some(account);
        self
    }

    pub fn token_source(mut self, source: Arc<dyn TokenSource>) -> Self {
        self.token_source = Some(source);
        self
    }

    pub fn etag_store(mut self, store: Arc<dyn EtagStore>) -> Self {
        self.etags = Some(store);
        self
    }

    pub fn rate_budget(mut self, budget: Arc<RateBudget>) -> Self {
        self.rate = Some(budget);
        self
    }

    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }

    pub fn base_rest_url(mut self, url: Url) -> Self {
        self.base_rest_url = Some(url);
        self
    }

    pub fn base_graphql_url(mut self, url: Url) -> Self {
        self.base_graphql_url = Some(url);
        self
    }

    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = Some(t);
        self
    }

    pub fn build(self) -> Result<GitHubClient, GitHubError> {
        let account = self
            .account
            .ok_or_else(|| GitHubError::InvalidHeader("account is required".into()))?;
        let token_source = self
            .token_source
            .ok_or_else(|| GitHubError::InvalidHeader("token_source is required".into()))?;
        let etags: Arc<dyn EtagStore> = self
            .etags
            .unwrap_or_else(|| Arc::new(InMemoryEtagStore::new()));
        let rate = self.rate.unwrap_or_else(|| Arc::new(RateBudget::new()));
        let ua = self
            .user_agent
            .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string());

        let (rest_url, graphql_url) =
            resolve_endpoints(&account.host, self.base_rest_url, self.base_graphql_url)?;

        let mut default_headers = HeaderMap::new();
        default_headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&ua)
                .map_err(|e| GitHubError::InvalidHeader(format!("user-agent: {e}")))?,
        );
        for (name, value) in self.extra_headers {
            default_headers.insert(name, value);
        }

        let mut builder = reqwest::Client::builder().default_headers(default_headers);
        if let Some(t) = self.timeout {
            builder = builder.timeout(t);
        }
        let inner = builder.build().map_err(GitHubError::Network)?;

        Ok(GitHubClient {
            inner,
            account,
            token_source,
            rate,
            etags,
            base_rest_url: rest_url,
            base_graphql_url: graphql_url,
        })
    }
}

/// Derive REST + GraphQL base URLs from a host string, honouring explicit overrides.
fn resolve_endpoints(
    host: &str,
    rest_override: Option<Url>,
    graphql_override: Option<Url>,
) -> Result<(Url, Url), GitHubError> {
    let (rest, graphql) = match host {
        "github.com" => (
            Url::parse("https://api.github.com/").unwrap(),
            Url::parse("https://api.github.com/graphql").unwrap(),
        ),
        enterprise => (
            Url::parse(&format!("https://{enterprise}/api/v3/"))?,
            Url::parse(&format!("https://{enterprise}/api/graphql"))?,
        ),
    };
    Ok((
        rest_override.unwrap_or(rest),
        graphql_override.unwrap_or(graphql),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::auth::StaticTokenSource;

    #[test]
    fn resolve_endpoints_for_dotcom_uses_api_subdomain() {
        let (rest, gql) = resolve_endpoints("github.com", None, None).unwrap();
        assert_eq!(rest.as_str(), "https://api.github.com/");
        assert_eq!(gql.as_str(), "https://api.github.com/graphql");
    }

    #[test]
    fn resolve_endpoints_for_enterprise_uses_api_v3_path() {
        let (rest, gql) = resolve_endpoints("ghe.acme.io", None, None).unwrap();
        assert_eq!(rest.as_str(), "https://ghe.acme.io/api/v3/");
        assert_eq!(gql.as_str(), "https://ghe.acme.io/api/graphql");
    }

    #[test]
    fn resolve_endpoints_honours_overrides() {
        let rest = Url::parse("https://example.test/rest/").unwrap();
        let gql = Url::parse("https://example.test/gql").unwrap();
        let (r, g) =
            resolve_endpoints("github.com", Some(rest.clone()), Some(gql.clone())).unwrap();
        assert_eq!(r, rest);
        assert_eq!(g, gql);
    }

    #[test]
    fn builder_requires_account_and_token_source() {
        let err = GitHubClient::builder().build().unwrap_err();
        assert!(matches!(err, GitHubError::InvalidHeader(_)));

        let err = GitHubClient::builder()
            .account(AccountHandle::new(1, "github.com", "alice"))
            .build()
            .unwrap_err();
        assert!(matches!(err, GitHubError::InvalidHeader(_)));
    }

    #[test]
    fn builder_defaults_provide_etag_store_and_rate_budget() {
        let client = GitHubClient::builder()
            .account(AccountHandle::new(1, "github.com", "alice"))
            .token_source(Arc::new(StaticTokenSource::new("ghp_test")))
            .build()
            .unwrap();
        assert_eq!(client.account().id, 1);
        assert!(!client.rate().snapshot().is_observed());
    }

    #[test]
    fn parse_next_link_pulls_url_from_rel_next() {
        let mut h = HeaderMap::new();
        h.insert(
            http::header::LINK,
            HeaderValue::from_static(
                "<https://api.github.com/repos/x/y/issues/1/timeline?page=2>; rel=\"next\", \
                 <https://api.github.com/repos/x/y/issues/1/timeline?page=5>; rel=\"last\"",
            ),
        );
        assert_eq!(
            parse_next_link(&h).as_deref(),
            Some("https://api.github.com/repos/x/y/issues/1/timeline?page=2"),
        );
    }

    #[test]
    fn parse_next_link_returns_none_when_only_prev_and_first() {
        let mut h = HeaderMap::new();
        h.insert(
            http::header::LINK,
            HeaderValue::from_static(
                "<https://api.github.com/x?page=4>; rel=\"prev\", \
                 <https://api.github.com/x?page=1>; rel=\"first\"",
            ),
        );
        assert!(parse_next_link(&h).is_none());
    }

    #[test]
    fn parse_next_link_returns_none_when_header_absent() {
        let h = HeaderMap::new();
        assert!(parse_next_link(&h).is_none());
    }

    #[test]
    fn hex_roundtrips_known_bytes() {
        let h = hex([
            0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0,
        ]);
        assert!(h.starts_with("deadbeef"));
        assert_eq!(h.len(), 64);
    }
}
