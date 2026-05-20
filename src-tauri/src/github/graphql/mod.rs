//! GraphQL surface of the GitHub client.
//!
//! Per ADR 0006, GraphQL is the primary protocol for PR detail and reviews.
//! Query strings live in [`queries`]; helper methods on `GitHubClient` live in
//! [`client`].

pub mod client;
pub mod queries;

pub use client::{PrCoord, TimelinePage};
pub use queries::{
    Actor, Comment, CommentConnection, DiscoveryData, DiscoveryNode, DiscoveryPullRequest,
    DiscoveryRepository, DiscoverySearch, IssueCommentConnection, IssueCommentNode,
    IssueCommentNodeConnection, PageInfo, PrCommentsData, PrCommentsRepository, PrCommit,
    PrCommitConnection, PrCommitNode, PrDetailData, PrTimelineData, PullRequestComments,
    PullRequestDetail, PullRequestReviewConnection, PullRequestReviewNode, PullRequestTimeline,
    RequestedReviewer, ReviewCommentConnection, ReviewCommentNode, ReviewRequest,
    ReviewRequestConnection, ReviewThread, ReviewThreadComments, ReviewThreadCommentsConnection,
    ReviewThreadConnection, StatusCheckContext, StatusCheckContexts, StatusCheckRollup,
    TimelineConnection, TimelineEvent, DISCOVERY_QUERY, PR_COMMENTS_QUERY, PR_DETAIL_QUERY,
    PR_TIMELINE_QUERY,
};
