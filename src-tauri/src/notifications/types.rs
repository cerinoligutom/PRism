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
    /// Unix seconds the row was marked read. ADR 0031 narrows this to the
    /// orphan-row fallback: a LIVE row's unread state is derived per-row
    /// against its own unit watermark, so `read_at` is meaningful only when
    /// `pull_request_id IS NULL` (the source PR was pruned). `mark_read` /
    /// `mark_all_read` write it for orphan rows only.
    pub read_at: Option<i64>,
    /// Conversation unit this row points at (ADR 0031): `'thread'` |
    /// `'general'` | `NULL` (legacy / pre-0025 PR-level row).
    pub unit_kind: Option<String>,
    /// Review thread `node_id` when `unit_kind = 'thread'`, else `NULL`.
    pub unit_ref: Option<String>,
    /// Deep link to the exact unit (thread url or PR conversation url).
    pub deep_link_url: Option<String>,
    /// Derived per-row unread flag (ADR 0031). A live row is unread iff its
    /// own unit still needs the viewer; an orphan row is unread iff
    /// `read_at IS NULL`; a legacy `unit_kind IS NULL` live row falls back to
    /// `read_at`. Computed by [`super::store::list`], not stored.
    pub unread: bool,
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
    /// Conversation unit reference (ADR 0031). `'thread'` | `'general'`;
    /// `None` only on the legacy / PR-level insert path.
    pub unit_kind: Option<String>,
    pub unit_ref: Option<String>,
    pub deep_link_url: Option<String>,
}
