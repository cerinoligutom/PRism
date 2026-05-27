//! Conversation surface — per-thread state, conversation stats, and the
//! drawer's cache-read command.
//!
//! ADR 0029: sync owns conversation persistence. The commands here are pure
//! readers; the lazy hydrator and its `PR_COMMENTS_QUERY` round-trip went away
//! with that change. The shared interface contract is
//! `docs/contracts/conversation-depth.md`.

pub mod commands;
pub mod query;
pub mod types;
pub(crate) mod writer;

pub use types::{
    CommentBreakdown, ConversationStats, HydratedConversation, IssueComment, PullRequestReview,
    PullRequestThread, ReviewsSummary, ThreadComment, ThreadHeadComment, ThreadState,
    TimelineEventRecord,
};
