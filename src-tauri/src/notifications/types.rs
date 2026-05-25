//! Wire shapes for the persistent notifications inbox.

use serde::{Deserialize, Serialize};

/// One inbox row, mirroring the `notifications` table.
///
/// The snapshot fields (`owner`, `repo`, `pr_number`, `pr_node_id`, `pr_title`)
/// duplicate state already in `pull_requests` at insert time so the row stays
/// meaningful after a PR prune. `pull_request_id` is the soft link the
/// State-A click path uses to open the local detail surface; it goes NULL
/// when the source PR row is deleted (ON DELETE SET NULL).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Notification {
    pub id: i64,
    /// Mirror of [`crate::notify::NotificationKind`] serialised as snake_case:
    /// `needs_attention` or `mention`.
    pub kind: String,
    pub account_id: i64,
    pub pull_request_id: Option<i64>,
    pub owner: String,
    pub repo: String,
    pub pr_number: i64,
    pub pr_node_id: Option<String>,
    pub pr_title: String,
    pub title: String,
    pub body: Option<String>,
    /// Unix seconds. Newest first in `list_notifications`.
    pub created_at: i64,
}

/// Insert payload handed to [`super::store::insert`]. Doesn't carry `id` or
/// `created_at`; the DB owns both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationInsert {
    pub kind: String,
    pub account_id: i64,
    pub pull_request_id: Option<i64>,
    pub owner: String,
    pub repo: String,
    pub pr_number: i64,
    pub pr_node_id: Option<String>,
    pub pr_title: String,
    pub title: String,
    pub body: Option<String>,
}
