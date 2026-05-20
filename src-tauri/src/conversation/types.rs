//! DTO types for the conversation query surface.
//!
//! Mirrors the TypeScript shapes documented in
//! `docs/contracts/conversation-depth.md`. The serde `kebab-case` rename is
//! the wire contract for the enum — frontend code reads `"unresolved"`, not
//! `"Unresolved"`.

use serde::{Deserialize, Serialize};

/// Per-thread state surfaced on the threads list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThreadState {
    Unresolved,
    Resolved,
    Outdated,
}

/// One review thread on a PR.
///
/// The `head_comment` snapshot is populated during the sync cycle from the
/// GraphQL `comments(first:1)` head; full comment bodies live on
/// [`ThreadComment`] after the lazy hydrator runs (`fetch_pr_conversation`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestThread {
    pub id: i64,
    pub node_id: String,
    pub pull_request_id: i64,
    pub state: ThreadState,
    pub path: Option<String>,
    pub line: Option<i64>,
    pub start_line: Option<i64>,
    pub original_line: Option<i64>,
    pub reply_count: i64,
    pub head_comment: Option<ThreadHeadComment>,
    pub created_at: Option<i64>,
    pub resolved_at: Option<i64>,
    pub last_reply_at: Option<i64>,
    /// True when the active account's login appears as a comment author
    /// anywhere in this thread.
    pub is_you_in: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadHeadComment {
    pub author_login: String,
    pub body_text: String,
    pub created_at: i64,
}

/// Aggregated conversation stats for the stats card on the conversation
/// surface. All values computed at read time from the local cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationStats {
    pub threads_total: i64,
    pub threads_unresolved: i64,
    pub threads_resolved: i64,
    pub threads_outdated: i64,
    /// Oldest `review_threads.created_at` among non-resolved + non-outdated
    /// threads. `None` when there are zero active threads.
    pub oldest_unresolved_at: Option<i64>,
    /// Average gap (in seconds) between consecutive `review_comments.created_at`
    /// within each thread, averaged across threads with two or more comments.
    /// `None` when no thread has a reply yet.
    pub avg_response_seconds: Option<i64>,
    /// `resolved / (total - outdated)`. `0.0` when total-non-outdated is zero.
    pub resolution_rate: f64,
    pub comment_breakdown: CommentBreakdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentBreakdown {
    /// Count of `review_comments` rows for this PR.
    pub review: i64,
    /// `pull_requests.issue_comments_count` rollup written by the sync cycle.
    pub issue: i64,
    /// Count of `reviews` rows for this PR with a non-empty `body`.
    pub summary: i64,
    /// `review + issue + summary`.
    pub total: i64,
}

/// One submitted `PullRequestReview` (top-level review body, separate from
/// inline review-thread comments).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestReview {
    pub id: i64,
    pub node_id: String,
    pub author_login: String,
    /// GraphQL `PullRequestReviewState`: `APPROVED`, `CHANGES_REQUESTED`,
    /// `COMMENTED`, `DISMISSED`, `PENDING`.
    pub state: String,
    pub body: Option<String>,
    pub submitted_at: Option<i64>,
}

/// One comment inside a review thread. Hydrated lazily by
/// `fetch_pr_conversation`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadComment {
    pub id: i64,
    pub thread_id: i64,
    pub author_login: String,
    pub body: String,
    pub created_at: i64,
    pub line: Option<i64>,
    pub side: Option<String>,
}

/// One issue comment (PR-level, not attached to a thread). Hydrated lazily
/// by `fetch_pr_conversation`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueComment {
    pub id: i64,
    pub author_login: String,
    pub body: String,
    pub created_at: i64,
}

/// Aggregate returned by `fetch_pr_conversation` — the complete conversation
/// state for one PR, hydrated from GitHub and persisted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HydratedConversation {
    pub pull_request_id: i64,
    pub threads: Vec<PullRequestThread>,
    pub thread_comments: Vec<ThreadComment>,
    pub issue_comments: Vec<IssueComment>,
    pub reviews: Vec<PullRequestReview>,
    pub stats: ConversationStats,
}
