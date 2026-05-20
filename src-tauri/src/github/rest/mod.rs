//! REST surface of the GitHub client.
//!
//! Per ADR 0006, REST is the fallback protocol: we only use it for endpoints
//! GraphQL doesn't cover. The primary REST consumers in v1 are the timeline
//! events API used by ADR 0007's status-change derivation and the repo-list
//! endpoint that backs Settings -> Repositories (M2-D).
//!
//! Endpoint wrappers live in submodules and call through to
//! [`crate::github::client::GitHubClient::get_conditional`] so they share the
//! ETag store, rate-limit budget, and auth header injection with the GraphQL
//! surface.

pub mod repos;
pub mod timeline;

pub use repos::{list_user_repos, ListRepos, RepoNode, RepoOwner, MAX_REPOS_PER_REFRESH};
pub use timeline::{list_pr_timeline, ListTimeline, RepoCoord};
