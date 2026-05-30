//! Wire shapes for the notification pipeline.
//!
//! Two structs live here:
//!
//! * [`NotificationTrigger`] is what the per-cycle sync recompute emits when a
//!   PR crosses the per-PR dispatch watermark (ADR 0031 edge-with-re-arm). It
//!   carries the identity the formatter needs (`account_id`,
//!   `pull_request_id`) plus the conversation **unit** that holds the newest
//!   crossing activity (`unit_kind` / `unit_ref` / `deep_link_url` /
//!   `newest_activity_at`) so the toast can name the file:line and deep-link
//!   the exact thread.
//!
//! * [`Notification`] is the formatted unit the [`super::NotificationSink`]
//!   consumes. The `payload` blob is what the frontend deep link router reads
//!   when the user clicks the toast (issue #201); it threads the unit fields
//!   so the click reconciles the exact unit.
//!
//! ADR 0017 records the permission lifecycle (decision 5) the sink enforces
//! against the `app_settings` row; ADR 0031 records the unit model.

use serde::{Deserialize, Serialize};

/// Inbox `kind` taxonomy. ADR 0031 collapses dispatch onto a single
/// conversation-unit signal, so [`NotificationKind::NeedsAttention`] is the
/// only kind the live path emits now. [`NotificationKind::Mention`] is
/// retained for the storage round-trip of legacy rows written before the
/// re-arm slice; nothing emits it anymore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// A conversation unit on the PR crossed the per-PR dispatch watermark
    /// with genuinely-new other-authored activity (ADR 0031).
    NeedsAttention,
    /// Legacy mention kind (pre-0031). Retained for storage compatibility;
    /// the live dispatch path no longer emits it.
    Mention,
}

/// Which conversation unit a trigger / inbox row points at (ADR 0031). A
/// review thread is keyed on its GraphQL `node_id`; the PR's general comment
/// stream is one dismissible unit per PR with no ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationUnitKind {
    /// A review thread (`unit_ref` carries its `node_id`).
    Thread,
    /// The PR's general comment stream (`unit_ref` is `None`).
    General,
}

impl NotificationUnitKind {
    /// String storage for the `notifications.unit_kind` column. Matches the
    /// `'thread'` / `'general'` values the per-unit roll-up SQL compares
    /// against and the `#[serde(rename_all = "snake_case")]` wire form.
    pub fn as_storage(self) -> &'static str {
        match self {
            NotificationUnitKind::Thread => "thread",
            NotificationUnitKind::General => "general",
        }
    }
}

/// One newly-observed unit crossing, addressed at the PR that produced it and
/// tagged with the conversation unit holding the newest crossing activity
/// (ADR 0031 edge-with-re-arm). The per-cycle sync recompute emits these after
/// the per-PR dispatch watermark advances; the worker hands them to the
/// [`super::NotificationSink`] once the enclosing transaction commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationTrigger {
    pub account_id: i64,
    pub pull_request_id: i64,
    pub kind: NotificationKind,
    /// The conversation unit this emit is tagged with (the unit holding the
    /// newest crossing activity in this cycle).
    pub unit_kind: NotificationUnitKind,
    /// The review thread `node_id` when `unit_kind` is
    /// [`NotificationUnitKind::Thread`]; `None` for the general stream.
    pub unit_ref: Option<String>,
    /// Deep link for the toast click: the thread's `url`, or the PR
    /// conversation URL for the general stream. `None` when neither resolves.
    pub deep_link_url: Option<String>,
    /// `created_at` of the newest crossing comment. The sink advances the
    /// per-PR `last_emitted_activity_at` to this value (MAX-only) so the same
    /// activity never re-fires.
    pub newest_activity_at: i64,
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
    /// The conversation unit this row points at (ADR 0031). Drives the
    /// per-row derived unread flag and the deep link. `None` only for the
    /// legacy / PR-level inbox path (none of the live emitters leave it
    /// unset).
    pub unit_kind: Option<NotificationUnitKind>,
    /// The thread `node_id` when `unit_kind` is
    /// [`NotificationUnitKind::Thread`]; `None` for the general stream.
    pub unit_ref: Option<String>,
    /// Deep link the toast click reconciles - the thread url or the PR
    /// conversation URL.
    pub deep_link_url: Option<String>,
}
