//! Read-only SQL composition for the conversation Tauri commands.
//!
//! Wave 1 lands this module as a stub. Wave 2-B fills in:
//!
//! - `list_pr_threads` — join `review_threads` + `review_comments` (head)
//!   + `accounts` (for `is_you_in`).
//! - `get_pr_conversation_stats` — the four-tile stats card math.
//! - `fetch_pr_conversation` — lazy hydrator that calls GitHub for full
//!   thread replies + issue-comment bodies, persists them, and returns the
//!   hydrated DTO.
//!
//! See `docs/contracts/conversation-depth.md` for the SQL shapes and the
//! conversation-stats definitions.
