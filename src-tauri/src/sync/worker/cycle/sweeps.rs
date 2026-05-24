//! Post-cycle housekeeping sweeps. Each runs inside its own transaction and
//! logs failures rather than propagating them - the worker treats the sweeps
//! as best-effort and the next cycle will retry.

use rusqlite::params;

use crate::db::DbHandle;

use super::RepoRow;

/// Wrap [`crate::triage::query::auto_archive_sweep`] in a transaction and
/// log the affected row count at INFO level. A failure inside the sweep is
/// logged and swallowed: the archive sweep is cosmetic and the next cycle
/// retries.
///
/// Reads `app_settings.auto_archive_days` (issue #333) so the sweep window
/// follows the user's Settings -> Sync choice. The settings load happens
/// inside the same transaction as the UPDATE so a concurrent write to
/// the singleton can't shift the window mid-sweep.
pub(super) fn run_auto_archive_sweep(db: &DbHandle) {
    let mut conn = match db.lock() {
        Ok(g) => g,
        Err(err) => {
            tracing::error!(%err, "auto-archive sweep: db poisoned");
            return;
        }
    };
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!(%err, "auto-archive sweep: begin tx failed");
            return;
        }
    };
    let days: i64 = match tx.query_row(
        "SELECT auto_archive_days FROM app_settings WHERE id = 1",
        [],
        |row| row.get(0),
    ) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(%err, "auto-archive sweep: read window failed");
            return;
        }
    };
    let archived = match crate::triage::query::auto_archive_sweep(&tx, days) {
        Ok(n) => n,
        Err(err) => {
            tracing::warn!(%err, "auto-archive sweep: update failed");
            return;
        }
    };
    if let Err(err) = tx.commit() {
        tracing::warn!(%err, "auto-archive sweep: commit failed");
        return;
    }
    tracing::info!(days, archived, "auto-archive sweep complete");
}

/// Wrap [`crate::triage::query::archive_retention_sweep`] in a transaction and
/// log the affected row count. Hard-deletes PRs whose every viewer relation
/// has been archived for more than 60 days; the FK cascade drops review
/// threads, comments, timeline events, and the relations themselves. A
/// failure inside the sweep is logged and swallowed - the sweep is
/// best-effort housekeeping and the next cycle retries.
pub(super) fn run_archive_retention_sweep(db: &DbHandle) {
    let mut conn = match db.lock() {
        Ok(g) => g,
        Err(err) => {
            tracing::error!(%err, "archive retention sweep: db poisoned");
            return;
        }
    };
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!(%err, "archive retention sweep: begin tx failed");
            return;
        }
    };
    let deleted = match crate::triage::query::archive_retention_sweep(&tx) {
        Ok(n) => n,
        Err(err) => {
            tracing::warn!(%err, "archive retention sweep: delete failed");
            return;
        }
    };
    if let Err(err) = tx.commit() {
        tracing::warn!(%err, "archive retention sweep: commit failed");
        return;
    }
    if deleted > 0 {
        tracing::info!(deleted, "archive retention sweep complete");
    }
}

pub(super) fn count_prs_across_repos(db: &DbHandle, repos: &[RepoRow]) -> u32 {
    let conn = match db.lock() {
        Ok(g) => g,
        Err(_) => return 0,
    };
    let mut total: u32 = 0;
    for repo in repos {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pull_requests WHERE repo_id = ?1",
                params![repo.id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        total = total.saturating_add(count.max(0) as u32);
    }
    total
}
