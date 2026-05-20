//! GraphQL query strings and response types.
//!
//! Three queries ship in v1:
//!
//! 1. `PR_DETAIL_QUERY` - full PR shape with `reviewThreads.isResolved`, which is
//!    the only place GitHub exposes thread resolution state (ADR 0006).
//! 2. `PR_TIMELINE_QUERY` - the timeline event types listed in ADR 0007, plus
//!    cursors for pagination.
//! 3. `DISCOVERY_QUERY` - the search-API call the discovery phase fans out three
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
      author { login }
      baseRefName
      headRefName
      reviewDecision
      reviewThreads(first: 100) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          isResolved
          isOutdated
          path
          comments(first: 1) {
            nodes {
              id
              author { login }
              bodyText
              createdAt
            }
          }
        }
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
          ... on ReadyForReviewEvent { createdAt actor { login } }
          ... on ConvertToDraftEvent { createdAt actor { login } }
          ... on ReviewRequestedEvent { createdAt actor { login } }
          ... on PullRequestReview { createdAt state author { login } }
          ... on MergedEvent { createdAt actor { login } }
          ... on ClosedEvent { createdAt actor { login } }
          ... on ReopenedEvent { createdAt actor { login } }
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
        author { login }
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
    pub review_threads: ReviewThreadConnection,
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
    pub comments: CommentConnection,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CommentConnection {
    pub nodes: Vec<Comment>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    #[serde(default)]
    pub author: Option<Actor>,
    pub body_text: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Actor {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub has_next_page: bool,
    #[serde(default)]
    pub end_cursor: Option<String>,
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
    fn discovery_repository_visibility_reports_private() {
        let repo = DiscoveryRepository {
            database_id: 1,
            owner: Actor {
                login: "owner".into(),
            },
            name: "repo".into(),
            is_private: true,
        };
        assert_eq!(repo.visibility(), "private");
    }
}
