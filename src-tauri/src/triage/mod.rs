//! Triage UX - per-account read-state, mention detection, and the
//! "needs my attention" composite signal.
//!
//! The shared interface contract for this module is
//! `docs/contracts/triage-ux.md`. Wave 1 lands the DTO types and the Tauri
//! command shell; Wave 2-A implements `mark_pr_read` / `mark_pr_unread`;
//! Wave 2-D implements `list_filter_chip_counts`; Wave 2-B extends the sync
//! cycle to populate `mentioned_count_unread` and `needs_attention`.

pub mod commands;
pub mod query;
pub mod types;

pub use types::{ChipKey, FilterChipCounts};
