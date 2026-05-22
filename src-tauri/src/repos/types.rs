//! DTOs returned by the `repos::commands` Tauri surface.

use serde::{Deserialize, Serialize};

/// Per-repo row sent to the Settings -> Repositories panel. Mirrors the
/// columns the panel reads or mutates; everything else stays in SQLite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoSummary {
    pub id: i64,
    pub account_id: i64,
    pub owner: String,
    pub name: String,
    /// `"public"`, `"private"`, or `"internal"` — straight from GitHub.
    pub visibility: String,
    pub is_tracked: bool,
}
