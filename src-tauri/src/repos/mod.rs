//! Repository discovery + opt-in surface for the Tracked view (M2-D).
//!
//! Responsibilities split between submodules:
//! - [`types`] — the `RepoSummary` DTO returned to the frontend.
//! - [`store`] — pure SQL helpers that read and upsert `repos` rows.
//! - [`commands`] — Tauri commands the Settings -> Repositories panel invokes.
//!
//! The REST repo-list endpoint lives under [`crate::github::rest::repos`] and
//! is called from [`commands::refresh_account_repos`]. See
//! `docs/contracts/dashboard-data.md` Wave 2-D for the full design.

pub mod commands;
pub mod store;
pub mod types;

pub use commands::{list_repos_for_account, refresh_account_repos, set_repo_tracked};
pub use types::RepoSummary;
