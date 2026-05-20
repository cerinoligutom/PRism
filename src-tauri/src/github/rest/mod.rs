//! REST surface of the GitHub client.
//!
//! Per ADR 0006, REST is the fallback protocol: we only use it for endpoints
//! GraphQL doesn't cover. The primary REST consumer in v1 is the timeline
//! events API used by ADR 0007's status-change derivation.
//!
//! Endpoint wrappers live in submodules and call through to
//! [`crate::github::client::GitHubClient::get_conditional`] so they share the
//! ETag store, rate-limit budget, and auth header injection with the GraphQL
//! surface.

pub mod timeline;

pub use timeline::{list_pr_timeline, ListTimeline, RepoCoord};
