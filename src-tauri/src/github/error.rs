//! Error type for the GitHub client surface.
//!
//! `GitHubError` is the single error returned by both the GraphQL and REST
//! clients so that the sync worker can map states (`Unauthorized`, `RateLimited`,
//! ...) without knowing which protocol produced them.

use std::time::Duration;
use thiserror::Error;

use crate::github::auth::AuthError;

/// A single error entry returned in a GraphQL response's `errors` array.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct GraphqlError {
    pub message: String,
    #[serde(default)]
    pub path: Option<Vec<serde_json::Value>>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Error)]
pub enum GitHubError {
    #[error("network: {0}")]
    Network(#[from] reqwest::Error),

    #[error("auth: {0}")]
    Auth(#[from] AuthError),

    #[error("rate limited; retry after {retry_after:?}")]
    RateLimited { retry_after: Option<Duration> },

    #[error("token expired or invalid")]
    Unauthorized,

    #[error("not found")]
    NotFound,

    #[error("github 5xx: {status}")]
    Server { status: u16 },

    #[error("graphql errors: {0:?}")]
    Graphql(Vec<GraphqlError>),

    #[error("deserialise: {0}")]
    Deserialize(#[from] serde_json::Error),

    #[error("invalid header value: {0}")]
    InvalidHeader(String),

    #[error("invalid url: {0}")]
    InvalidUrl(#[from] url::ParseError),
}
