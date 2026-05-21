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
/// M2 shipped with `Updated` only. M4 (`docs/contracts/triage-ux.md`,
/// ADR 0015) adds `Stale` and `NeedsMe`. The new variants are wired through
/// at the type level by the contract PR; Wave 3-D implements the matching
/// `ORDER BY` clauses in `dashboard::query`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DashboardSort {
    /// `ORDER BY COALESCE(latest_status_change_at, updated_at) DESC, id DESC`.
    Updated,
    /// `ORDER BY updated_at ASC, id DESC` - oldest activity first.
    Stale,
    /// `ORDER BY needs_attention DESC,
    ///          COALESCE(latest_status_change_at, updated_at) DESC,
    ///          id DESC`.
    NeedsMe,
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
    /// GitHub avatar URL for `author_login`. Resolved via `LEFT JOIN users`
    /// at query time. `None` when the user hasn't been seen by any sync cycle
    /// yet (frontend falls back to the initials avatar). See ADR 0013.
    pub author_avatar_url: Option<String>,
    pub base_ref: String,
    pub head_ref: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub latest_status_change_at: Option<i64>,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub changed_files: Option<i64>,
    pub ci: Option<CiSummary>,
    /// Per-PR review-thread rollup written by the sync cycle. `None` when the
    /// PR has never had a thread (`threads_total == 0`); the frontend renders
    /// the muted em-dash state in that case.
    pub threads: Option<ThreadsSummary>,
    pub reviewers: Vec<ReviewerEntry>,
    pub repo: RepoRef,
    pub account_id: i64,
    /// True when the viewer hasn't opened this PR since the last upstream
    /// update. Derived at query time as
    /// `read_at IS NULL OR pull_requests.updated_at > read_pr_updated_at`
    /// against the active account's `pull_request_viewer_relations` row.
    /// `None` collapses to `false` if the join misses (e.g. a Team-view PR
    /// the active account has no relation row for). See ADR 0015 and
    /// `docs/contracts/triage-ux.md` ("Read-state derivation").
    pub unread: bool,
    /// Precomputed "needs my attention" composite. Read from
    /// `pull_request_viewer_relations.needs_attention` for the active account.
    /// See ADR 0015 ("Composite formula") for the four input conditions.
    pub needs_attention: bool,
    /// Running count of `@<viewer-login>` mentions the sync cycle has seen
    /// since the last read. Reset to zero by `mark_pr_read`. See
    /// ADR 0015 ("Mention detection").
    pub mentioned_count_unread: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiSummary {
    /// GraphQL `statusCheckRollup.state`: `"SUCCESS"`, `"FAILURE"`,
    /// `"PENDING"`, `"ERROR"`, `"EXPECTED"`.
    pub state: String,
    pub total: i64,
    pub passing: i64,
}

/// Per-PR review-thread rollup, pre-aggregated by the sync cycle into the
/// `pull_requests.threads_*` columns. See `docs/contracts/conversation-depth.md`
/// ("Dashboard rollup"), ADR 0010, and ADR 0012 (four-bucket redesign).
///
/// The four bucket fields are disjoint over the full thread set (including
/// outdated). `total` equals the sum of the four. Outdated threads sort into
/// whichever bucket matches their (resolved x involved) state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadsSummary {
    pub total: i64,
    pub unresolved_involved: i64,
    pub unresolved_uninvolved: i64,
    pub resolved_involved: i64,
    pub resolved_uninvolved: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewerEntry {
    pub login: String,
    pub state: ReviewerState,
    /// True when the reviewer's login matches the account's viewer login.
    pub is_you: bool,
    /// GitHub avatar URL for `login`. Resolved via `LEFT JOIN users` at query
    /// time. See ADR 0013.
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRef {
    pub id: i64,
    pub owner: String,
    pub name: String,
}
