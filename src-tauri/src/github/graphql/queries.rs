//! GraphQL query strings and response types.
//!
//! Four queries ship in v1:
//!
//! 1. `PR_DETAIL_QUERY` - full PR shape with `reviewThreads.isResolved`, which is
//!    the only place GitHub exposes thread resolution state (ADR 0006).
//! 2. `PR_TIMELINE_QUERY` - the timeline event types listed in ADR 0007, plus
//!    cursors for pagination.
//! 3. `PR_COMMENTS_QUERY` - the lazy-hydration query M3 uses to pull full thread
//!    and issue-comment bodies on drawer / route open (ADR 0010).
//! 4. `DISCOVERY_QUERY` - the search-API call the discovery phase fans out three
//!    times per account per cycle to enumerate Authored / Assigned / Watching
//!    PRs (ADR 0009).
//!
//! The query strings are deliberately verbose rather than fragment-heavy to keep
//! the request body diffable in tests and easy to inspect in fixture files.

use serde::Deserialize;

/// PR detail. Includes review thread resolution state, which is GraphQL-only.
pub const PR_DETAIL_QUERY: &str = r#"
query PrDetail($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      id
      number
      title
      isDraft
      state
      merged
      mergeable
      url
      createdAt
      updatedAt
      author { login avatarUrl }
      baseRefName
      headRefName
      reviewDecision
      additions
      deletions
      changedFiles
      reviewRequests(first: 20) {
        nodes {
          requestedReviewer {
            __typename
            ... on User { login avatarUrl }
            ... on Team { slug }
          }
        }
      }
      commits(last: 1) {
        nodes {
          commit {
            statusCheckRollup {
              state
              contexts(first: 100) {
                totalCount
                nodes {
                  __typename
                  ... on CheckRun { conclusion status }
                  ... on StatusContext { state }
                }
              }
            }
          }
        }
      }
      reviewThreads(first: 100) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          startLine
          originalLine
          comments(first: 1) {
            totalCount
            nodes {
              id
              url
              author { login avatarUrl }
              bodyText
              createdAt
            }
          }
        }
      }
      reviews(first: 30) {
        nodes {
          id
          state
          body
          bodyHTML
          submittedAt
          url
          author { login avatarUrl }
        }
      }
      issueComments: comments(first: 50) {
        totalCount
      }
    }
  }
}
"#;

/// Timeline events sufficient to reconstruct "latest status change at" per ADR 0007.
///
/// The qualifying union members are `ReadyForReviewEvent`, `ConvertToDraftEvent`,
/// `ReviewRequestedEvent`, `PullRequestReview`, `MergedEvent`, `ClosedEvent`,
/// `ReopenedEvent`. We pull `__typename` plus `createdAt` for each so a downstream
/// pure function can walk newest-first.
pub const PR_TIMELINE_QUERY: &str = r#"
query PrTimeline($owner: String!, $name: String!, $number: Int!, $after: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      id
      timelineItems(
        first: 100,
        after: $after,
        itemTypes: [
          READY_FOR_REVIEW_EVENT,
          CONVERT_TO_DRAFT_EVENT,
          REVIEW_REQUESTED_EVENT,
          PULL_REQUEST_REVIEW,
          MERGED_EVENT,
          CLOSED_EVENT,
          REOPENED_EVENT
        ]
      ) {
        pageInfo { hasNextPage endCursor }
        nodes {
          __typename
          ... on ReadyForReviewEvent { createdAt actor { login avatarUrl } }
          ... on ConvertToDraftEvent { createdAt actor { login avatarUrl } }
          ... on ReviewRequestedEvent { createdAt actor { login avatarUrl } }
          ... on PullRequestReview { createdAt state author { login avatarUrl } }
          ... on MergedEvent { createdAt actor { login avatarUrl } }
          ... on ClosedEvent { createdAt actor { login avatarUrl } }
          ... on ReopenedEvent { createdAt actor { login avatarUrl } }
        }
      }
    }
  }
}
"#;

/// Full thread + issue-comment bodies for the lazy hydrator (M3, ADR 0010).
///
/// Called once per `fetch_pr_conversation` invocation. The sync cycle pulls a
/// head-comment snapshot via `PR_DETAIL_QUERY`; this query fills in the rest of
/// the conversation when the drawer / route opens. Capped at 100 threads per
/// page x 100 comments per thread + 100 issue comments per page (the lazy
/// hydrator caps total pulls at 200 comments / 200 issue comments per the
/// contract).
pub const PR_COMMENTS_QUERY: &str = r#"
query PrComments($owner: String!, $name: String!, $number: Int!, $threadsAfter: String, $issueCommentsAfter: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $threadsAfter) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          comments(first: 100) {
            pageInfo { hasNextPage endCursor }
            nodes {
              id
              url
              databaseId
              author { login avatarUrl }
              body
              bodyHTML
              bodyText
              createdAt
              path
              line
              originalLine
            }
          }
        }
      }
      issueComments: comments(first: 100, after: $issueCommentsAfter) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          url
          databaseId
          author { login }
          body
          bodyHTML
          bodyText
          createdAt
        }
      }
    }
  }
}
"#;

/// PR discovery via the GraphQL Search API. ADR 0009.
///
/// Called three times per account per cycle with different `q` strings - one
/// each for Authored, Review-requested, and Involves. `@me` resolves on the
/// server side, so the viewer's login never leaves the device.
pub const DISCOVERY_QUERY: &str = r#"
query DiscoverPrs($q: String!, $after: String) {
  search(type: ISSUE, query: $q, first: 50, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes {
      __typename
      ... on PullRequest {
        id
        databaseId
        number
        title
        url
        state
        isDraft
        createdAt
        updatedAt
        author { login avatarUrl }
        baseRefName
        headRefName
        repository {
          databaseId
          owner { login }
          name
          isPrivate
        }
      }
    }
  }
}
"#;

// ===== PR detail =====

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PrDetailData {
    pub repository: Option<PrDetailRepository>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PrDetailRepository {
    #[serde(rename = "pullRequest")]
    pub pull_request: Option<PullRequestDetail>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestDetail {
    pub id: String,
    pub number: i64,
    pub title: String,
    pub is_draft: bool,
    pub state: String,
    pub merged: bool,
    pub mergeable: String,
    pub url: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub author: Option<Actor>,
    pub base_ref_name: String,
    pub head_ref_name: String,
    #[serde(default)]
    pub review_decision: Option<String>,
    #[serde(default)]
    pub additions: Option<i64>,
    #[serde(default)]
    pub deletions: Option<i64>,
    #[serde(default)]
    pub changed_files: Option<i64>,
    #[serde(default)]
    pub review_requests: Option<ReviewRequestConnection>,
    #[serde(default)]
    pub commits: Option<PrCommitConnection>,
    pub review_threads: ReviewThreadConnection,
    #[serde(default)]
    pub reviews: Option<PullRequestReviewConnection>,
    #[serde(default)]
    pub issue_comments: Option<IssueCommentConnection>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ReviewRequestConnection {
    pub nodes: Vec<ReviewRequest>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRequest {
    #[serde(default)]
    pub requested_reviewer: Option<RequestedReviewer>,
}

/// Discriminated union over `User` and `Team` reviewer nodes. Both branches
/// carry an identifier (`login` for users, `slug` for teams) that's persisted
/// to `requested_reviewers.login`; the variant distinguishes `reviewer_type`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "__typename", rename_all_fields = "camelCase")]
pub enum RequestedReviewer {
    User {
        login: String,
        #[serde(default)]
        avatar_url: Option<String>,
    },
    Team {
        slug: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PrCommitConnection {
    pub nodes: Vec<PrCommitNode>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PrCommitNode {
    pub commit: PrCommit,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrCommit {
    #[serde(default)]
    pub status_check_rollup: Option<StatusCheckRollup>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct StatusCheckRollup {
    pub state: String,
    pub contexts: StatusCheckContexts,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusCheckContexts {
    pub total_count: i64,
    pub nodes: Vec<StatusCheckContext>,
}

/// One entry under `statusCheckRollup.contexts`. `CheckRun` carries
/// `conclusion`/`status` (a `null` conclusion means the run is still in
/// progress); `StatusContext` is the legacy commit-status shape with `state`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "__typename")]
pub enum StatusCheckContext {
    CheckRun {
        #[serde(default)]
        conclusion: Option<String>,
        #[serde(default)]
        status: Option<String>,
    },
    StatusContext {
        state: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThreadConnection {
    pub page_info: PageInfo,
    pub nodes: Vec<ReviewThread>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThread {
    pub id: String,
    pub is_resolved: bool,
    pub is_outdated: bool,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<i64>,
    #[serde(default)]
    pub start_line: Option<i64>,
    #[serde(default)]
    pub original_line: Option<i64>,
    pub comments: CommentConnection,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommentConnection {
    pub total_count: i64,
    pub nodes: Vec<Comment>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    /// GitHub permalink for the comment. Surfaced so the sync worker can derive
    /// the thread's permalink from the head comment (review threads themselves
    /// have no `url` field on GitHub's GraphQL schema). See issue #115.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub author: Option<Actor>,
    pub body_text: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PullRequestReviewConnection {
    pub nodes: Vec<PullRequestReviewNode>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestReviewNode {
    pub id: String,
    pub state: String,
    #[serde(default)]
    pub body: Option<String>,
    /// Pre-rendered HTML from GitHub for the review summary body. Persisted
    /// to `reviews.body_html` and rendered through `PRismMarkdown` on the
    /// conversation surface. `None` for legacy fixtures and for queries that
    /// don't request `bodyHTML`. See ADR 0014 and issue #138.
    #[serde(default, rename = "bodyHTML")]
    pub body_html: Option<String>,
    #[serde(default)]
    pub submitted_at: Option<String>,
    /// GitHub permalink for the review. Persisted to `reviews.url` so the
    /// frontend can offer a per-review "Open in GitHub" affordance, mirroring
    /// the thread-level pattern from M3.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub author: Option<Actor>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IssueCommentConnection {
    pub total_count: i64,
}

/// Author / actor surfaced by GraphQL. `avatar_url` is populated by every
/// query that selects `avatarUrl` on its `author { ... }` / `actor { ... }`
/// branches; older queries that only requested `login` deserialise with
/// `avatar_url = None`, which is what the users-table writer expects (it
/// upserts only when an avatar URL is present).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Actor {
    pub login: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

impl Actor {
    /// Convenience constructor for tests / fixtures that only carry a login.
    pub fn new(login: impl Into<String>) -> Self {
        Self {
            login: login.into(),
            avatar_url: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub has_next_page: bool,
    #[serde(default)]
    pub end_cursor: Option<String>,
}

// ===== PR comments (lazy hydration) =====

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PrCommentsData {
    pub repository: Option<PrCommentsRepository>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PrCommentsRepository {
    #[serde(rename = "pullRequest")]
    pub pull_request: Option<PullRequestComments>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestComments {
    pub review_threads: ReviewThreadCommentsConnection,
    pub issue_comments: IssueCommentNodeConnection,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThreadCommentsConnection {
    pub page_info: PageInfo,
    pub nodes: Vec<ReviewThreadComments>,
}

/// One review thread, paired with the hydrated comment array.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThreadComments {
    pub id: String,
    pub comments: ReviewCommentConnection,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewCommentConnection {
    pub page_info: PageInfo,
    pub nodes: Vec<ReviewCommentNode>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewCommentNode {
    pub id: String,
    /// GitHub permalink for the comment. Persisted to `review_comments.url`
    /// so the conversation surface can offer a per-comment "Open in GitHub"
    /// action. See issue #115.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub database_id: Option<i64>,
    #[serde(default)]
    pub author: Option<Actor>,
    pub body: String,
    /// Pre-rendered HTML from GitHub for the comment body. Persisted to
    /// `review_comments.body_html` and rendered through `PRismMarkdown`. See
    /// ADR 0014 and issue #138. `None` for legacy fixtures.
    #[serde(default, rename = "bodyHTML")]
    pub body_html: Option<String>,
    pub body_text: String,
    pub created_at: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<i64>,
    #[serde(default)]
    pub original_line: Option<i64>,
    #[serde(default)]
    pub side: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IssueCommentNodeConnection {
    pub page_info: PageInfo,
    pub nodes: Vec<IssueCommentNode>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IssueCommentNode {
    pub id: String,
    /// GitHub permalink for the issue comment. Persisted to `issue_comments.url`
    /// for parity with review comments; the conversation surface doesn't render
    /// issue comments yet (M3 contract) but the data is captured for a future
    /// PR. See issue #115.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub database_id: Option<i64>,
    #[serde(default)]
    pub author: Option<Actor>,
    pub body: String,
    /// Pre-rendered HTML from GitHub for the issue-comment body. Persisted to
    /// `issue_comments.body_html` for parity with review comments. See ADR
    /// 0014 and issue #138.
    #[serde(default, rename = "bodyHTML")]
    pub body_html: Option<String>,
    pub body_text: String,
    pub created_at: String,
}

// ===== Timeline =====

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PrTimelineData {
    pub repository: Option<PrTimelineRepository>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PrTimelineRepository {
    #[serde(rename = "pullRequest")]
    pub pull_request: Option<PullRequestTimeline>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestTimeline {
    pub id: String,
    pub timeline_items: TimelineConnection,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineConnection {
    pub page_info: PageInfo,
    pub nodes: Vec<TimelineEvent>,
}

/// Discriminated union over GitHub's timeline event types. The `__typename`
/// field drives serde's tagging; unknown event types fall through to `Other`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "__typename")]
pub enum TimelineEvent {
    ReadyForReviewEvent {
        #[serde(rename = "createdAt")]
        created_at: String,
        #[serde(default)]
        actor: Option<Actor>,
    },
    ConvertToDraftEvent {
        #[serde(rename = "createdAt")]
        created_at: String,
        #[serde(default)]
        actor: Option<Actor>,
    },
    ReviewRequestedEvent {
        #[serde(rename = "createdAt")]
        created_at: String,
        #[serde(default)]
        actor: Option<Actor>,
    },
    PullRequestReview {
        #[serde(rename = "createdAt")]
        created_at: String,
        state: String,
        #[serde(default)]
        author: Option<Actor>,
    },
    MergedEvent {
        #[serde(rename = "createdAt")]
        created_at: String,
        #[serde(default)]
        actor: Option<Actor>,
    },
    ClosedEvent {
        #[serde(rename = "createdAt")]
        created_at: String,
        #[serde(default)]
        actor: Option<Actor>,
    },
    ReopenedEvent {
        #[serde(rename = "createdAt")]
        created_at: String,
        #[serde(default)]
        actor: Option<Actor>,
    },
    #[serde(other)]
    Other,
}

impl TimelineEvent {
    /// The event's `createdAt`. `Other` returns `None`.
    pub fn created_at(&self) -> Option<&str> {
        match self {
            Self::ReadyForReviewEvent { created_at, .. }
            | Self::ConvertToDraftEvent { created_at, .. }
            | Self::ReviewRequestedEvent { created_at, .. }
            | Self::PullRequestReview { created_at, .. }
            | Self::MergedEvent { created_at, .. }
            | Self::ClosedEvent { created_at, .. }
            | Self::ReopenedEvent { created_at, .. } => Some(created_at.as_str()),
            Self::Other => None,
        }
    }
}

// ===== Discovery (Search API) =====

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DiscoveryData {
    pub search: DiscoverySearch,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverySearch {
    pub page_info: PageInfo,
    pub nodes: Vec<DiscoveryNode>,
}

/// Search-result node. Type ISSUE returns issues and PRs; non-PR nodes
/// deserialise as `Other` and the worker skips them. The query string adds
/// `is:pr` belt-and-braces, so `Other` is rare in practice.
///
/// The `PullRequest` variant is boxed because the inner payload is hundreds
/// of bytes and the `Other` variant is empty; without the indirection clippy
/// (rightly) flags the size disparity.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "__typename")]
pub enum DiscoveryNode {
    PullRequest(Box<DiscoveryPullRequest>),
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryPullRequest {
    /// GraphQL global node id - kept for parity with other types; not persisted.
    pub id: String,
    /// Integer id stable across hosts; written to `pull_requests.id`.
    pub database_id: i64,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub is_draft: bool,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub author: Option<Actor>,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub repository: DiscoveryRepository,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryRepository {
    pub database_id: i64,
    pub owner: Actor,
    pub name: String,
    pub is_private: bool,
}

impl DiscoveryRepository {
    pub fn visibility(&self) -> &'static str {
        if self.is_private {
            "private"
        } else {
            "public"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_detail_query_includes_is_resolved() {
        assert!(PR_DETAIL_QUERY.contains("isResolved"));
        assert!(PR_DETAIL_QUERY.contains("reviewThreads"));
    }

    #[test]
    fn pr_detail_query_includes_dashboard_enrichment_fields() {
        for field in [
            "additions",
            "deletions",
            "changedFiles",
            "reviewRequests(first: 20)",
            "requestedReviewer",
            "... on User { login avatarUrl }",
            "... on Team { slug }",
            "commits(last: 1)",
            "statusCheckRollup",
            "contexts(first: 100)",
            "totalCount",
            "... on CheckRun { conclusion status }",
            "... on StatusContext { state }",
        ] {
            assert!(
                PR_DETAIL_QUERY.contains(field),
                "pr detail query missing field: {field}"
            );
        }
    }

    #[test]
    fn pr_detail_query_includes_conversation_depth_fields() {
        // Thread line range + comments.totalCount + head comment shape.
        for field in [
            "line",
            "startLine",
            "originalLine",
            "comments(first: 1)",
            "reviews(first: 30)",
            "submittedAt",
            "issueComments: comments(first: 50)",
        ] {
            assert!(
                PR_DETAIL_QUERY.contains(field),
                "pr detail query missing conversation-depth field: {field}"
            );
        }
    }

    #[test]
    fn pr_detail_query_omits_url_on_review_thread() {
        // Regression: `PullRequestReviewThread` has no `url` field on the
        // GitHub GraphQL schema (issue #115). The thread's permalink is
        // derived from the head comment's `url` at write time.
        let after_thread_open = PR_DETAIL_QUERY
            .split("reviewThreads(first: 100)")
            .nth(1)
            .expect("query opens reviewThreads block");
        let thread_block = after_thread_open
            .split("reviews(first: 30)")
            .next()
            .expect("query closes the thread block before reviews");
        // The block contains `comments(first: 1)` whose selection includes
        // `url`. Strip that out before asserting no thread-level `url`.
        let head_comment_open = thread_block
            .find("comments(first: 1)")
            .expect("thread carries a comments(first:1) selection");
        let pre_head = &thread_block[..head_comment_open];
        assert!(
            !pre_head.contains("url"),
            "PR_DETAIL_QUERY still selects `url` on PullRequestReviewThread"
        );
    }

    #[test]
    fn pr_detail_query_selects_url_on_head_comment() {
        // The thread permalink now comes from the head comment.
        let head_block = PR_DETAIL_QUERY
            .split("comments(first: 1)")
            .nth(1)
            .expect("query carries comments(first: 1)");
        assert!(
            head_block.contains("url"),
            "head comment selection must include `url`"
        );
    }

    #[test]
    fn review_thread_deserialises_with_line_range_and_total_count() {
        let json = serde_json::json!({
            "id": "PRRT_1",
            "isResolved": false,
            "isOutdated": false,
            "path": "src/lib.rs",
            "line": 42,
            "startLine": 40,
            "originalLine": 41,
            "comments": {
                "totalCount": 3,
                "nodes": [{
                    "id": "PRRC_1",
                    "url": "https://github.com/owner/repo/pull/1#discussion_r1",
                    "author": { "login": "alice" },
                    "bodyText": "Looks good",
                    "createdAt": "2026-05-19T10:00:00Z"
                }]
            }
        });
        let thread: ReviewThread = serde_json::from_value(json).unwrap();
        assert_eq!(thread.line, Some(42));
        assert_eq!(thread.start_line, Some(40));
        assert_eq!(thread.original_line, Some(41));
        assert_eq!(thread.comments.total_count, 3);
        assert_eq!(thread.comments.nodes.len(), 1);
        assert_eq!(
            thread.comments.nodes[0].url.as_deref(),
            Some("https://github.com/owner/repo/pull/1#discussion_r1")
        );
    }

    #[test]
    fn head_comment_deserialises_with_missing_url_as_none() {
        // Older fixtures (and tests written before issue #115) won't carry
        // the new `url` on the head comment. Confirm it defaults to None
        // rather than failing the whole detail parse.
        let json = serde_json::json!({
            "id": "PRRC_legacy",
            "author": { "login": "alice" },
            "bodyText": "head body",
            "createdAt": "2026-05-19T10:00:00Z"
        });
        let comment: Comment = serde_json::from_value(json).unwrap();
        assert!(comment.url.is_none());
    }

    #[test]
    fn pull_request_review_node_deserialises_full_shape() {
        let json = serde_json::json!({
            "id": "PRR_1",
            "state": "APPROVED",
            "body": "LGTM",
            "submittedAt": "2026-05-19T12:00:00Z",
            "author": { "login": "alice" }
        });
        let review: PullRequestReviewNode = serde_json::from_value(json).unwrap();
        assert_eq!(review.id, "PRR_1");
        assert_eq!(review.state, "APPROVED");
        assert_eq!(review.body.as_deref(), Some("LGTM"));
        assert_eq!(review.submitted_at.as_deref(), Some("2026-05-19T12:00:00Z"));
        assert_eq!(review.author.unwrap().login, "alice");
    }

    #[test]
    fn pull_request_review_node_deserialises_with_null_optionals() {
        // PENDING reviews have no submittedAt or body.
        let json = serde_json::json!({
            "id": "PRR_2",
            "state": "PENDING",
            "body": null,
            "submittedAt": null,
            "author": { "login": "bob" }
        });
        let review: PullRequestReviewNode = serde_json::from_value(json).unwrap();
        assert!(review.body.is_none());
        assert!(review.submitted_at.is_none());
    }

    #[test]
    fn issue_comment_connection_deserialises_total_count_only() {
        let json = serde_json::json!({ "totalCount": 17 });
        let ic: IssueCommentConnection = serde_json::from_value(json).unwrap();
        assert_eq!(ic.total_count, 17);
    }

    #[test]
    fn requested_reviewer_deserialises_user_and_team() {
        let user: RequestedReviewer =
            serde_json::from_value(serde_json::json!({ "__typename": "User", "login": "alice" }))
                .unwrap();
        assert_eq!(
            user,
            RequestedReviewer::User {
                login: "alice".into(),
                avatar_url: None,
            }
        );

        let team: RequestedReviewer =
            serde_json::from_value(serde_json::json!({ "__typename": "Team", "slug": "platform" }))
                .unwrap();
        assert_eq!(
            team,
            RequestedReviewer::Team {
                slug: "platform".into()
            }
        );

        let unknown: RequestedReviewer =
            serde_json::from_value(serde_json::json!({ "__typename": "Bot" })).unwrap();
        assert_eq!(unknown, RequestedReviewer::Other);
    }

    #[test]
    fn status_check_context_deserialises_check_run_and_status_context() {
        let check: StatusCheckContext = serde_json::from_value(serde_json::json!({
            "__typename": "CheckRun",
            "conclusion": "SUCCESS",
            "status": "COMPLETED"
        }))
        .unwrap();
        assert_eq!(
            check,
            StatusCheckContext::CheckRun {
                conclusion: Some("SUCCESS".into()),
                status: Some("COMPLETED".into()),
            }
        );

        let in_progress: StatusCheckContext = serde_json::from_value(serde_json::json!({
            "__typename": "CheckRun",
            "status": "IN_PROGRESS"
        }))
        .unwrap();
        assert_eq!(
            in_progress,
            StatusCheckContext::CheckRun {
                conclusion: None,
                status: Some("IN_PROGRESS".into()),
            }
        );

        let status: StatusCheckContext = serde_json::from_value(serde_json::json!({
            "__typename": "StatusContext",
            "state": "FAILURE"
        }))
        .unwrap();
        assert_eq!(
            status,
            StatusCheckContext::StatusContext {
                state: "FAILURE".into()
            }
        );
    }

    #[test]
    fn timeline_query_includes_all_qualifying_events() {
        for tag in [
            "READY_FOR_REVIEW_EVENT",
            "CONVERT_TO_DRAFT_EVENT",
            "REVIEW_REQUESTED_EVENT",
            "PULL_REQUEST_REVIEW",
            "MERGED_EVENT",
            "CLOSED_EVENT",
            "REOPENED_EVENT",
        ] {
            assert!(
                PR_TIMELINE_QUERY.contains(tag),
                "timeline query missing event type {tag}"
            );
        }
    }

    #[test]
    fn timeline_event_deserialises_unknown_typename_as_other() {
        let json = serde_json::json!({ "__typename": "SomethingNew" });
        let evt: TimelineEvent = serde_json::from_value(json).unwrap();
        assert_eq!(evt, TimelineEvent::Other);
        assert_eq!(evt.created_at(), None);
    }

    #[test]
    fn timeline_event_deserialises_merged() {
        let json = serde_json::json!({
            "__typename": "MergedEvent",
            "createdAt": "2026-05-19T10:00:00Z",
            "actor": { "login": "alice" }
        });
        let evt: TimelineEvent = serde_json::from_value(json).unwrap();
        assert_eq!(evt.created_at(), Some("2026-05-19T10:00:00Z"));
    }

    #[test]
    fn discovery_query_uses_search_with_pull_request_inline_fragment() {
        assert!(DISCOVERY_QUERY.contains("search(type: ISSUE"));
        assert!(DISCOVERY_QUERY.contains("... on PullRequest"));
        assert!(DISCOVERY_QUERY.contains("databaseId"));
        assert!(DISCOVERY_QUERY.contains("repository"));
    }

    #[test]
    fn discovery_node_deserialises_pull_request() {
        let json = serde_json::json!({
            "__typename": "PullRequest",
            "id": "PR_kwDOABC",
            "databaseId": 12345,
            "number": 7,
            "title": "Add discovery",
            "url": "https://github.com/owner/repo/pull/7",
            "state": "OPEN",
            "isDraft": false,
            "createdAt": "2026-05-18T10:00:00Z",
            "updatedAt": "2026-05-19T10:00:00Z",
            "author": { "login": "alice" },
            "baseRefName": "main",
            "headRefName": "feat/discovery",
            "repository": {
                "databaseId": 999,
                "owner": { "login": "owner" },
                "name": "repo",
                "isPrivate": false
            }
        });
        let node: DiscoveryNode = serde_json::from_value(json).unwrap();
        match node {
            DiscoveryNode::PullRequest(pr) => {
                assert_eq!(pr.database_id, 12345);
                assert_eq!(pr.number, 7);
                assert_eq!(pr.repository.database_id, 999);
                assert_eq!(pr.repository.owner.login, "owner");
                assert_eq!(pr.repository.visibility(), "public");
            }
            other => panic!("expected PullRequest, got {other:?}"),
        }
    }

    #[test]
    fn discovery_node_deserialises_non_pull_request_as_other() {
        let json = serde_json::json!({ "__typename": "Issue", "id": "I_1" });
        let node: DiscoveryNode = serde_json::from_value(json).unwrap();
        assert_eq!(node, DiscoveryNode::Other);
    }

    #[test]
    fn pr_comments_query_includes_threads_and_issue_comments() {
        for field in [
            "reviewThreads(first: 100",
            "issueComments: comments(first: 100",
            "comments(first: 100)",
            "databaseId",
            "bodyText",
            "originalLine",
            // Comment-level permalinks for both review and issue comments
            // (issue #115).
            "url",
        ] {
            assert!(
                PR_COMMENTS_QUERY.contains(field),
                "pr comments query missing field: {field}"
            );
        }
    }

    #[test]
    fn pr_comments_data_deserialises_full_payload() {
        let json = serde_json::json!({
            "repository": {
                "pullRequest": {
                    "reviewThreads": {
                        "pageInfo": { "hasNextPage": false, "endCursor": null },
                        "nodes": [{
                            "id": "PRRT_1",
                            "comments": {
                                "pageInfo": { "hasNextPage": false, "endCursor": null },
                                "nodes": [{
                                    "id": "PRRC_1",
                                    "url": "https://github.com/owner/repo/pull/1#discussion_r4242",
                                    "databaseId": 4242,
                                    "author": { "login": "alice" },
                                    "body": "**hello**",
                                    "bodyText": "hello",
                                    "createdAt": "2026-05-19T10:00:00Z",
                                    "path": "src/lib.rs",
                                    "line": 12,
                                    "originalLine": 10
                                }]
                            }
                        }]
                    },
                    "issueComments": {
                        "pageInfo": { "hasNextPage": true, "endCursor": "c1" },
                        "nodes": [{
                            "id": "IC_1",
                            "url": "https://github.com/owner/repo/pull/1#issuecomment-9001",
                            "databaseId": 9001,
                            "author": { "login": "bob" },
                            "body": "looks good",
                            "bodyText": "looks good",
                            "createdAt": "2026-05-19T11:00:00Z"
                        }]
                    }
                }
            }
        });
        let parsed: PrCommentsData = serde_json::from_value(json).unwrap();
        let pr = parsed.repository.unwrap().pull_request.unwrap();
        assert_eq!(pr.review_threads.nodes.len(), 1);
        assert_eq!(pr.review_threads.nodes[0].comments.nodes.len(), 1);
        let c = &pr.review_threads.nodes[0].comments.nodes[0];
        assert_eq!(c.database_id, Some(4242));
        assert_eq!(c.path.as_deref(), Some("src/lib.rs"));
        assert_eq!(
            c.url.as_deref(),
            Some("https://github.com/owner/repo/pull/1#discussion_r4242")
        );
        // `side` lives on PullRequestReviewThread.diffSide, not on the
        // comment; this query no longer requests it, so the field is None
        // and the DB column stays NULL until a future query pulls diffSide.
        assert_eq!(c.side, None);
        assert_eq!(pr.issue_comments.nodes.len(), 1);
        assert_eq!(
            pr.issue_comments.nodes[0].url.as_deref(),
            Some("https://github.com/owner/repo/pull/1#issuecomment-9001")
        );
        assert!(pr.issue_comments.page_info.has_next_page);
        assert_eq!(
            pr.issue_comments.page_info.end_cursor.as_deref(),
            Some("c1")
        );
    }

    #[test]
    fn pr_detail_query_selects_body_html_on_reviews() {
        // ADR 0014 / issue #138: pre-rendered HTML for review summary bodies.
        let reviews_block = PR_DETAIL_QUERY
            .split("reviews(first: 30)")
            .nth(1)
            .expect("query carries reviews(first: 30)");
        assert!(
            reviews_block.contains("bodyHTML"),
            "reviews selection must request bodyHTML"
        );
    }

    #[test]
    fn pr_comments_query_selects_body_html_on_review_and_issue_comments() {
        // bodyHTML appears in both the per-thread comments selection and the
        // PR-level issueComments selection so the lazy hydrator can persist
        // GitHub's pre-rendered HTML alongside the markdown body.
        assert_eq!(
            PR_COMMENTS_QUERY.matches("bodyHTML").count(),
            2,
            "PR_COMMENTS_QUERY should select bodyHTML on both review + issue comments"
        );
    }

    #[test]
    fn pull_request_review_node_deserialises_with_body_html() {
        let json = serde_json::json!({
            "id": "PRR_1",
            "state": "COMMENTED",
            "body": "**lgtm**",
            "bodyHTML": "<p><strong>lgtm</strong></p>",
            "submittedAt": "2026-05-19T12:00:00Z",
            "author": { "login": "alice" }
        });
        let review: PullRequestReviewNode = serde_json::from_value(json).unwrap();
        assert_eq!(
            review.body_html.as_deref(),
            Some("<p><strong>lgtm</strong></p>")
        );
    }

    #[test]
    fn pull_request_review_node_deserialises_with_missing_body_html_as_none() {
        let json = serde_json::json!({
            "id": "PRR_legacy",
            "state": "APPROVED",
            "author": { "login": "alice" }
        });
        let review: PullRequestReviewNode = serde_json::from_value(json).unwrap();
        assert!(review.body_html.is_none());
    }

    #[test]
    fn review_comment_node_deserialises_with_body_html() {
        let json = serde_json::json!({
            "id": "PRRC_1",
            "databaseId": 4242,
            "author": { "login": "alice" },
            "body": "`hi`",
            "bodyHTML": "<p><code>hi</code></p>",
            "bodyText": "hi",
            "createdAt": "2026-05-19T10:00:00Z"
        });
        let comment: ReviewCommentNode = serde_json::from_value(json).unwrap();
        assert_eq!(comment.body_html.as_deref(), Some("<p><code>hi</code></p>"));
    }

    #[test]
    fn review_comment_node_deserialises_with_missing_body_html_as_none() {
        let json = serde_json::json!({
            "id": "PRRC_legacy",
            "body": "plain",
            "bodyText": "plain",
            "createdAt": "2026-05-19T10:00:00Z"
        });
        let comment: ReviewCommentNode = serde_json::from_value(json).unwrap();
        assert!(comment.body_html.is_none());
    }

    #[test]
    fn issue_comment_node_deserialises_with_body_html() {
        let json = serde_json::json!({
            "id": "IC_1",
            "databaseId": 9001,
            "author": { "login": "bob" },
            "body": "looks good",
            "bodyHTML": "<p>looks good</p>",
            "bodyText": "looks good",
            "createdAt": "2026-05-19T11:00:00Z"
        });
        let comment: IssueCommentNode = serde_json::from_value(json).unwrap();
        assert_eq!(comment.body_html.as_deref(), Some("<p>looks good</p>"));
    }

    #[test]
    fn issue_comment_node_deserialises_with_missing_body_html_as_none() {
        let json = serde_json::json!({
            "id": "IC_legacy",
            "body": "plain",
            "bodyText": "plain",
            "createdAt": "2026-05-19T11:00:00Z"
        });
        let comment: IssueCommentNode = serde_json::from_value(json).unwrap();
        assert!(comment.body_html.is_none());
    }

    #[test]
    fn discovery_repository_visibility_reports_private() {
        let repo = DiscoveryRepository {
            database_id: 1,
            owner: Actor::new("owner"),
            name: "repo".into(),
            is_private: true,
        };
        assert_eq!(repo.visibility(), "private");
    }
}
