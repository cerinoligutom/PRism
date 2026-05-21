//! Tauri command surface for the triage module.
//!
//! Wave 1 lands the command shell with `unimplemented!()` bodies so the
//! types check, the frontend can wire up `invoke()` calls behind a feature
//! flag, and the parallel Wave-2 agents can implement the bodies without
//! touching the dispatcher in `lib.rs`. See
//! `docs/contracts/triage-ux.md` ("Tauri command surface") for the contract.

use tauri::State;

use crate::dashboard::DashboardView;
use crate::db::DbHandle;
use crate::triage::types::FilterChipCounts;

/// Mark a PR as read for the given account. Sets
/// `pull_request_viewer_relations.read_at` to the current Unix timestamp,
/// captures `pull_requests.updated_at` into `read_pr_updated_at`, resets
/// `mentioned_count_unread` to zero, and pushes
/// `mention_scan_watermark_at` to the current timestamp so future sync
/// cycles only count comments newer than the open.
///
/// The composite `needs_attention` flag is recomputed against the new
/// state inside the same transaction so the next dashboard read reflects
/// the open.
///
/// Idempotent: re-marking an already-read PR is a no-op apart from
/// refreshing the timestamps.
#[tauri::command]
pub fn mark_pr_read(
    pull_request_id: i64,
    account_id: i64,
    _db: State<'_, DbHandle>,
) -> Result<(), String> {
    let _ = (pull_request_id, account_id);
    unimplemented!("M4-A");
}

/// Flip a PR back to unread for the given account. Clears
/// `read_at` and `read_pr_updated_at` so the derived `unread` projection
/// returns true. `mentioned_count_unread` is _not_ rewritten - the next
/// sync cycle re-counts comments past the existing
/// `mention_scan_watermark_at` if any matched.
///
/// Used by the "Mark unread" menu action (M4-F polish).
#[tauri::command]
pub fn mark_pr_unread(
    pull_request_id: i64,
    account_id: i64,
    _db: State<'_, DbHandle>,
) -> Result<(), String> {
    let _ = (pull_request_id, account_id);
    unimplemented!("M4-A");
}

/// Count how many PRs in the current view would match each filter chip
/// _independently_ of the other chips. The chips compose as AND at the
/// list level, but the counts are per-chip so the user sees what would
/// match if they toggled a single chip alone.
///
/// The view scope still applies (chips never cross view boundaries) so the
/// caller passes the active `DashboardView` + `account_id`. Returns
/// `FilterChipCounts` with one i64 per chip - see the type doc for the
/// per-chip predicate definitions.
#[tauri::command]
pub fn list_filter_chip_counts(
    view: DashboardView,
    account_id: i64,
    _db: State<'_, DbHandle>,
) -> Result<FilterChipCounts, String> {
    let _ = (view, account_id);
    unimplemented!("M4-D");
}
