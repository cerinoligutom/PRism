//! DTO types for the dashboard query surface.
//!
//! Mirrors the TypeScript shapes documented in
//! `docs/contracts/dashboard-data.md`. The serde `kebab-case` rename is the
//! wire contract — frontend code reads `"changes-requested"`, not
//! `"ChangesRequested"`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DashboardView {
    Authored,
    Assigned,
    Watching,
    Team,
}

/// Sort order for the dashboard list.
///
/// M2 ships with `Updated` only. M4 will add `NeedsAttention`, `Stale`,
/// `Comments` once the underlying signals exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DashboardSort {
    Updated,
}

/// Reviewer's review state surfaced on the dashboard row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewerState {
    Approved,
    ChangesRequested,
    Commented,
    /// Requested but not yet submitted.
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardPullRequest {
    pub id: i64,
    pub number: i64,
    pub title: String,
    pub url: String,
    /// `"open"`, `"closed"`, or `"merged"`. The `"merged"` value is derived
    /// from GraphQL `merged` (see `sync::worker::write_pr_updates`).
    pub state: String,
    pub is_draft: bool,
    /// GraphQL `mergeable`: `"MERGEABLE"`, `"CONFLICTING"`, `"UNKNOWN"`.
    pub mergeable: Option<String>,
    /// GraphQL `reviewDecision`: `"APPROVED"`, `"CHANGES_REQUESTED"`,
    /// `"REVIEW_REQUIRED"`.
    pub review_decision: Option<String>,
    pub author_login: String,
    pub base_ref: String,
    pub head_ref: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub latest_status_change_at: Option<i64>,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub changed_files: Option<i64>,
    pub ci: Option<CiSummary>,
    pub reviewers: Vec<ReviewerEntry>,
    pub repo: RepoRef,
    pub account_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiSummary {
    /// GraphQL `statusCheckRollup.state`: `"SUCCESS"`, `"FAILURE"`,
    /// `"PENDING"`, `"ERROR"`, `"EXPECTED"`.
    pub state: String,
    pub total: i64,
    pub passing: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewerEntry {
    pub login: String,
    pub state: ReviewerState,
    /// True when the reviewer's login matches the account's viewer login.
    pub is_you: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRef {
    pub id: i64,
    pub owner: String,
    pub name: String,
}
