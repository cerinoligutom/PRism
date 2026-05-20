//! REST wrapper for `GET /user/repos`.
//!
//! Used by the Settings -> Repositories panel (M2-D) to populate the per-account
//! repo list the user opts into the Team view. Walks `Link rel="next"` for
//! pagination (RFC 5988) and returns at most [`MAX_REPOS_PER_REFRESH`] entries
//! per call so a runaway pagination cycle can't exhaust the rate budget.
//!
//! A 304 on the first page short-circuits to [`ListRepos::NotModified`]; a 304
//! on a later page just stops the walk. Each page is cached independently in
//! the ETag store keyed by its path+query.

use bytes::Bytes;
use serde::Deserialize;
use url::Url;

use crate::github::client::{parse_next_link, Conditional, GitHubClient};
use crate::github::error::GitHubError;

/// Affiliation filter sent to `/user/repos`. Mirrors the GitHub API field of
/// the same name; covers any repo the viewer owns, collaborates on, or is a
/// member of via an organisation.
///
/// Commas are URL-encoded as `%2C` so the literal `,` only appears between
/// `Link` header entries (RFC 5988). `parse_next_link` splits the header on
/// `,` to walk pages; an unencoded comma inside the URL would break that
/// split.
const AFFILIATION: &str = "owner%2Ccollaborator%2Corganization_member";

/// Page size for the listing. 100 is the GitHub maximum.
const PER_PAGE: u32 = 100;

/// Hard cap so a runaway pagination cycle can't burn the whole rate budget.
/// Five pages at `per_page=100` covers up to 500 repos, which is well above
/// the v1 expectation for an individual user.
pub const MAX_REPOS_PER_REFRESH: usize = 500;

/// Result of a conditional repo-list fetch.
#[derive(Debug)]
pub enum ListRepos {
    /// Upstream returned 304 on the first page; cached repos are still
    /// authoritative.
    NotModified,
    /// Fresh repo list from upstream (possibly truncated at
    /// [`MAX_REPOS_PER_REFRESH`]).
    Repos(Vec<RepoNode>),
}

impl ListRepos {
    pub fn is_modified(&self) -> bool {
        matches!(self, ListRepos::Repos(_))
    }
}

/// Wire shape for one element of the `/user/repos` list response. Only the
/// fields PRism actually persists are deserialised; the rest are skipped.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RepoNode {
    /// GitHub's repo id. Kept on the wire shape for completeness; the local
    /// `repos.id` is autoincremented per account (see `repos::store`).
    pub id: i64,
    pub name: String,
    pub owner: RepoOwner,
    /// `"public"`, `"private"`, or `"internal"`.
    pub visibility: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RepoOwner {
    pub login: String,
}

/// Fetch the viewer's repos via `/user/repos`, walking `Link rel="next"` until
/// exhausted, [`MAX_REPOS_PER_REFRESH`] is hit, or a 304 stops the walk.
pub async fn list_user_repos(client: &GitHubClient) -> Result<ListRepos, GitHubError> {
    let mut path = format!("/user/repos?affiliation={AFFILIATION}&per_page={PER_PAGE}");
    let mut all_repos: Vec<RepoNode> = Vec::new();
    let mut page_index = 0usize;

    loop {
        if all_repos.len() >= MAX_REPOS_PER_REFRESH {
            break;
        }
        match client.get_conditional(&path).await? {
            Conditional::NotModified => {
                if page_index == 0 {
                    return Ok(ListRepos::NotModified);
                }
                break;
            }
            Conditional::Modified { body, headers, .. } => {
                let repos = parse_repos_page(&body)?;
                all_repos.extend(repos);
                if all_repos.len() >= MAX_REPOS_PER_REFRESH {
                    all_repos.truncate(MAX_REPOS_PER_REFRESH);
                    break;
                }
                match parse_next_link(&headers).and_then(|s| relative_path(&s)) {
                    Some(next) => path = next,
                    None => break,
                }
                page_index += 1;
            }
        }
    }

    Ok(ListRepos::Repos(all_repos))
}

/// Strip scheme + host from an absolute URL emitted by GitHub's `Link` header,
/// leaving `/path?query` so it can be fed back to `client.get_conditional`
/// (which is path-relative and keys the ETag store by path).
fn relative_path(absolute: &str) -> Option<String> {
    let url = Url::parse(absolute).ok()?;
    let mut out = url.path().to_string();
    if let Some(q) = url.query() {
        out.push('?');
        out.push_str(q);
    }
    Some(out)
}

fn parse_repos_page(bytes: &Bytes) -> Result<Vec<RepoNode>, GitHubError> {
    Ok(serde_json::from_slice(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repo_with_owner_login() {
        let json = br#"[
            {
                "id": 12345,
                "name": "PRism",
                "owner": { "login": "cerinoligutom" },
                "visibility": "public"
            }
        ]"#;
        let repos = parse_repos_page(&Bytes::from_static(json)).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].id, 12345);
        assert_eq!(repos[0].name, "PRism");
        assert_eq!(repos[0].owner.login, "cerinoligutom");
        assert_eq!(repos[0].visibility, "public");
    }

    #[test]
    fn parses_private_repo_visibility() {
        let json = br#"[
            {
                "id": 1,
                "name": "secret",
                "owner": { "login": "ada" },
                "visibility": "private"
            }
        ]"#;
        let repos = parse_repos_page(&Bytes::from_static(json)).unwrap();
        assert_eq!(repos[0].visibility, "private");
    }

    #[test]
    fn parses_empty_page() {
        let json = b"[]";
        let repos = parse_repos_page(&Bytes::from_static(json)).unwrap();
        assert!(repos.is_empty());
    }

    #[test]
    fn list_repos_is_modified_predicate() {
        assert!(ListRepos::Repos(vec![]).is_modified());
        assert!(!ListRepos::NotModified.is_modified());
    }

    #[test]
    fn relative_path_strips_scheme_and_host() {
        let got = relative_path("https://api.github.com/user/repos?page=2&per_page=100");
        assert_eq!(got.as_deref(), Some("/user/repos?page=2&per_page=100"));
    }

    #[test]
    fn relative_path_handles_url_without_query() {
        let got = relative_path("https://api.github.com/user/repos");
        assert_eq!(got.as_deref(), Some("/user/repos"));
    }

    #[test]
    fn relative_path_returns_none_for_invalid_url() {
        assert!(relative_path("not a url").is_none());
    }
}
