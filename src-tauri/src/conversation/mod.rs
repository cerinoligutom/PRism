//! Conversation surface - per-thread state, conversation stats, and the
//! lazy-hydration command path.
//!
//! The shared interface contract for this module is
//! `docs/contracts/conversation-depth.md`. Wave 1 lands the DTO types and the
//! Tauri command shell; Wave 2-B implements the SQL composition and the lazy
//! hydrator body.

pub mod commands;
pub mod query;
pub mod types;

pub use commands::{AccountStoreHandle, ClientFactoryHandle};
pub use types::{
    CommentBreakdown, ConversationStats, HydratedConversation, IssueComment, PullRequestReview,
    PullRequestThread, ReviewsSummary, ThreadComment, ThreadHeadComment, ThreadState,
    TimelineEventRecord,
};
