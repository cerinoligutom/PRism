//! Dashboard query surface.
//!
//! The shared interface contract for this module is
//! `docs/contracts/dashboard-data.md`. Wave 1 lands the DTO types and the
//! Tauri command shell; Wave 2-C implements the SQL composition.

pub mod commands;
pub mod query;
pub mod types;

pub use types::{
    CiSummary, DashboardPullRequest, DashboardSort, DashboardView, DashboardViewCounts, RepoRef,
    ReviewerEntry, ReviewerState, ThreadsSummary,
};
