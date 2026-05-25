//! Wire shapes for the notification pipeline.
//!
//! Two structs live here:
//!
//! * [`NotificationTrigger`] is what `recompute_needs_attention` will emit
//!   (issue #192) once it observes one of the ADR 0017 transitions. It carries
//!   enough identity (`account_id`, `pull_request_id`, [`NotificationKind`])
//!   for the downstream formatter to look up the PR row and compose the user
//!   facing copy. The trigger itself is intentionally identity only - no PR
//!   title, repo slug, or author - so the emitter doesn't have to denormalise
//!   the recompute payload.
//!
//! * [`Notification`] is the formatted unit the [`super::NotificationSink`]
//!   consumes. The `payload` blob is what the frontend deep link router will
//!   read once issue #201 lands - keeping the click target on the
//!   notification itself rather than weaving routing through the sink keeps
//!   the trait surface narrow.
//!
//! The trigger -> notification formatting step lands with the trigger emitter
//! in #192. This crate ships the two types and the boundary so #192 doesn't
//! have to also invent them.
//!
//! ADR 0017 records the trigger taxonomy (decision 1) and the permission
//! lifecycle (decision 5) the sink enforces against the `app_settings` row.

use serde::{Deserialize, Serialize};

/// Which ADR 0017 transition fired the trigger.
///
/// The two variants line up with the two signals `recompute_needs_attention`
/// already detects (ADR 0015): the 0->1 flip on `needs_attention` and the
/// increment on `mentioned_count_unread`. Mapped to user prefs via the two
/// per-trigger toggles on the `app_settings` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// A PR newly crossed into the "needs my attention" bucket
    /// (`pull_request_viewer_relations.needs_attention` flipped 0 -> 1).
    NeedsAttention,
    /// The viewer was mentioned in a new unresolved comment
    /// (`mentioned_count_unread` increased on this cycle).
    Mention,
}

/// One newly-observed attention signal, addressed at the relation row that
/// produced it. Emitted by the recompute helper after a transition.
///
/// The emitter (issue #192) hands these to the [`super::NotificationSink`].
/// The sink decides whether to dispatch based on the master switch + per
/// trigger toggle + OS permission state (ADR 0017 decision 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationTrigger {
    pub account_id: i64,
    pub pull_request_id: i64,
    pub kind: NotificationKind,
}

/// Formatted dispatch unit consumed by [`super::NotificationSink::dispatch`].
///
/// The trigger -> notification formatting step lives in
/// [`super::formatter::format_trigger`]. Keeping the boundary explicit means
/// the sink doesn't have to know how triggers turn into copy, and the
/// emitter doesn't have to know how the OS plugin is addressed - either
/// side can evolve independently.
///
/// `payload` is forwarded to the frontend when the user clicks the toast
/// (issue #201). Conventionally `{ account_id, pull_request_id }` so the
/// router can push onto the PR detail surface; the sink enqueues it onto
/// [`super::pending::PendingPayloadQueue`] before firing the toast and
/// doesn't otherwise inspect it.
///
/// The `snapshot` fields back the persistent inbox (issue #378). The
/// production sink writes a row into `notifications` from this struct before
/// firing the OS toast so the user can recover a missed toast at
/// `/dashboard/notifications`. Inbox insertion is best-effort; a DB failure
/// is logged but does not block the OS toast (different reliability
/// requirements).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub payload: serde_json::Value,
    /// Inbox snapshot pulled from the same lookup the formatter ran for the
    /// title / body copy. `None` skips the inbox write entirely - useful for
    /// future trigger paths that intentionally bypass persistence.
    pub snapshot: Option<NotificationSnapshot>,
}

/// Snapshot of the source PR carried alongside a [`Notification`] so the
/// persistent inbox row stays meaningful after the local PR row is pruned.
/// Mirrors the columns on `notifications` modulo the DB-owned `id` /
/// `created_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSnapshot {
    pub kind: NotificationKind,
    pub account_id: i64,
    pub pull_request_id: Option<i64>,
    pub owner: String,
    pub repo: String,
    pub pr_number: i64,
    pub pr_node_id: Option<String>,
    pub pr_title: String,
}
