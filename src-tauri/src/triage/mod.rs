//! Triage UX - per-account read-state, mention detection, and the
//! "needs my attention" composite signal.
//!
//! The shared interface contract for this module is
//! `docs/contracts/triage-ux.md`. Wave 1 lands the DTO types and the Tauri
//! command shell; Wave 2-D implements `list_filter_chip_counts`; Wave 2-B
//! extends the sync cycle to set the per-comment `mentions_viewer` bit and
//! recompute `needs_attention`. The PR-level read/unread commands (ADR 0031
//! bold-title axis) were retired in ADR 0033 with the single-dot redesign.

pub mod commands;
pub mod query;
pub mod types;
pub mod units;

pub use types::{ChipKey, FilterChipCounts, SidebarAttentionCounts};
