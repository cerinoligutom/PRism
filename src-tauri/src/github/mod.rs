//! GitHub API client surface.
//!
//! Two protocols share one HTTP layer (ADR 0006):
//!
//! - [`graphql`] — primary surface (PR detail, review threads, reviews).
//! - [`rest`] — fallback for endpoints GraphQL doesn't cover (timeline events).
//!
//! The shared infrastructure (HTTP client, auth, rate limits, ETag store, error
//! mapping) lives in [`client`], [`auth`], [`rate_limit`], [`etag`], and
//! [`error`] respectively. Both clients construct a [`GitHubClient`] per
//! account.

pub mod auth;
pub mod client;
pub mod error;
pub mod etag;
pub mod graphql;
pub mod rate_limit;
pub mod rest;

pub use auth::{AccountHandle, AccountId, AuthError, StaticTokenSource, TokenSource};
pub use client::{Conditional, GitHubClient, GitHubClientBuilder};
pub use error::{GitHubError, GraphqlError};
pub use etag::{graphql_key, rest_key, EtagEntry, EtagStore, InMemoryEtagStore};
pub use rate_limit::{RateBudget, RateResource, RateSnapshot, ResourceSnapshot};
pub use rest::{
    list_pr_timeline, list_user_repos, ListRepos, ListTimeline, RepoCoord, RepoNode, RepoOwner,
    MAX_REPOS_PER_REFRESH,
};
