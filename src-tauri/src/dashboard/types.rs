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
    /// Per-repo opt-in surface (issue #220, formerly named "Team"). PRs from
    /// repos with `is_tracked = 1` show up here regardless of whether the
    /// viewer has a personal relation to them. M8 lands a separate Teams-
    /// driven view; this one stays repo-flag-gated.
    Tracked,
    /// Archive bucket (ADR 0018). Returns only rows where the
    /// `pull_request_viewer_relations.archived_at` column is non-NULL. Ignores
    /// the four-view-split predicates (`is_authored`, `is_review_requested`,
    /// `is_involved`, `repos.is_tracked`) - archive is global across every
    /// relation a viewer holds. Wave 2 wires the route, sidebar entry, and the
    /// PR row archive action; this variant lands the read path only.
    Archive,
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
    /// Tracked accounts with a relation to this PR (Authored / Assigned /
    /// Watching). Sorted ascending. In the single-account-filter path the
    /// vector has length 1 - the active account id. In the unified path
    /// (`account_id = None`) it carries 1..N ids: every relation owner the
    /// `GROUP BY pr.id` merge folded together. For the Tracked view in
    /// unified mode a PR with no relation rows still surfaces (the view
    /// filter is on `repos.is_tracked`); in that shape the vector is empty.
    ///
    /// The frontend reads the first id as the representative account when it
    /// needs one (e.g. the `mark unread` action's per-account fallback target);
    /// the URL builder picks the host from this representative because the PR
    /// lives on exactly one host - the host of the repo's owning account.
    /// See ADR 0016 ("Dashboard row shape - option 1").
    pub account_ids: Vec<i64>,
    /// True when the viewer hasn't opened this PR since the last upstream
    /// update. Derived at query time as
    /// `read_at IS NULL OR pull_requests.updated_at > read_pr_updated_at`
    /// against the active account's `pull_request_viewer_relations` row.
    /// `None` collapses to `false` if the join misses (e.g. a Tracked-view PR
    /// the active account has no relation row for). In the unified path the
    /// per-relation flag is merged via `MAX` so the row reads unread when any
    /// in-scope account is unread. See ADR 0015 and
    /// `docs/contracts/triage-ux.md` ("Read-state derivation").
    pub unread: bool,
    /// Precomputed "needs my attention" composite. Read from
    /// `pull_request_viewer_relations.needs_attention` for the active account.
    /// Merged via `MAX` in the unified path so the row flags attention when
    /// any in-scope account needs the viewer. See ADR 0015
    /// ("Composite formula") for the four input conditions.
    pub needs_attention: bool,
    /// Running count of `@<viewer-login>` mentions the sync cycle has seen
    /// since the last read. Reset to zero by `mark_pr_read`. Summed across
    /// in-scope accounts in the unified path. See ADR 0015 ("Mention
    /// detection").
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

/// Per-view row counts for the active `(account_id)` scope. Powers the sidebar
/// count chips without round-tripping the full dashboard list for each view.
///
/// Each field equals the length of `list_pull_requests(view, ..., account_id,
/// &[])` for the matching variant. The numbers therefore mirror the dashboard
/// query's view predicates (including ADR 0018's archive filter on default
/// views and the `pr.state = 'open'` post-M6 guard) so the chip and the list
/// agree row-for-row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DashboardViewCounts {
    pub authored: i64,
    pub assigned: i64,
    pub watching: i64,
    pub tracked: i64,
    pub archive: i64,
}
