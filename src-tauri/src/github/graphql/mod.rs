//! GraphQL surface of the GitHub client.
//!
//! Per ADR 0006, GraphQL is the primary protocol for PR detail and reviews.
//! Query strings live in [`queries`]; helper methods on `GitHubClient` live in
//! [`client`].

pub mod client;
pub mod queries;

pub use client::{PrCoord, TimelinePage};
pub use queries::{
    Actor, Comment, CommentConnection, PageInfo, PrDetailData, PrTimelineData, PullRequestDetail,
    PullRequestTimeline, ReviewThread, ReviewThreadConnection, TimelineConnection, TimelineEvent,
    PR_DETAIL_QUERY, PR_TIMELINE_QUERY,
};
