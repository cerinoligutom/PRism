//! Shared SQL helpers for the triage module.
//!
//! Wave 1 left this module empty. Wave 2-A lands the
//! [`recompute_needs_attention`] helper used by `mark_pr_read` /
//! `mark_pr_unread` (this module's command bodies) and intentionally
//! duplicated by Wave 2-B's per-cycle recompute inside
//! `sync::worker::write_pr_updates` so the two write paths stay decoupled
//! across the parallel implementation waves. See `docs/contracts/triage-ux.md`
//! ("Sync cycle changes") and ADR 0015 ("Composite formula") for the
//! single source of truth for the four input signals.
//!
//! Wave 2-D additionally lands the per-chip count SQL consumed by
//! [`crate::triage::commands::list_filter_chip_counts`]. The five counts share
//! the same view-scoped FROM clause; each chip's predicate is documented in
//! [`crate::triage::types::ChipKey`].
//!
//! Wave 2-C extends this module with [`count_sidebar_attention`], the
//! per-view COUNT(*) that backs the sidebar count-chip `.has-attention`
//! boost.

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use crate::dashboard::DashboardView;
use crate::notify::{NotificationKind, NotificationTrigger};
use crate::triage::types::{ChipKey, FilterChipCounts, SidebarAttentionCounts};

/// Persist the read-state flip for one `(account_id, pull_request_id)` pair.
/// UPSERTs the relation row, sets `read_at` + `mention_scan_watermark_at` to
/// now, snapshots `pull_requests.updated_at` into `read_pr_updated_at`, and
/// resets `mentioned_count_unread` to zero. Callers wrap the call in their
/// own transaction so the recompute that follows ([`recompute_needs_attention`])
/// runs in the same atomic block.
///
/// Shared by `triage::commands::mark_pr_read` and the auto-mark hook in
/// `conversation::commands::fetch_pr_conversation`.
pub fn mark_read(
    conn: &Connection,
    account_id: i64,
    pull_request_id: i64,
) -> Result<(), rusqlite::Error> {
    // `query_row` errors `QueryReturnedNoRows` on a missing PR; flatten to
    // `None` via `optional()` so the auto-mark hook can fire safely while
    // the dashboard is mid-load (no PR row yet) without abort-on-error.
    // The UPSERT below still sets `read_pr_updated_at = NULL` in that case,
    // which the dashboard's unread derivation treats as "always unread"
    // until the next sync.
    let pr_updated_at: Option<i64> = conn
        .query_row(
            "SELECT updated_at FROM pull_requests WHERE id = ?1",
            params![pull_request_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    conn.execute(
        "INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, relation_observed_at,
             read_at, read_pr_updated_at, mentioned_count_unread,
             mention_scan_watermark_at)
            VALUES (?1, ?2, strftime('%s','now'),
                    strftime('%s','now'), ?3, 0,
                    strftime('%s','now'))
         ON CONFLICT(account_id, pull_request_id) DO UPDATE SET
            read_at                    = strftime('%s','now'),
            read_pr_updated_at         = excluded.read_pr_updated_at,
            mentioned_count_unread     = 0,
            mention_scan_watermark_at  = strftime('%s','now')",
        params![account_id, pull_request_id, pr_updated_at],
    )?;
    Ok(())
}

/// Clear the read watermark for one `(account_id, pull_request_id)` pair.
/// Leaves `mentioned_count_unread` and `mention_scan_watermark_at` untouched;
/// the next sync's scanner is the only thing that increments the counter.
/// No-op when the relation row doesn't exist.
pub fn mark_unread(
    conn: &Connection,
    account_id: i64,
    pull_request_id: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET read_at = NULL,
                read_pr_updated_at = NULL
          WHERE account_id = ?1 AND pull_request_id = ?2",
        params![account_id, pull_request_id],
    )?;
    Ok(())
}

/// Set `archived_at = now` for one `(account_id, pull_request_id)` pair.
/// UPSERTs the relation row so an account whose viewer has never opened the
/// PR's drawer (no row yet) can still archive it from the unified row's
/// overflow menu. Per ADR 0018, manual + auto archive share this column; the
/// sync sweep writes the same value on closed/merged PRs older than 30 days.
///
/// Other triage columns are left to their schema defaults on insert and to
/// their existing values on conflict - read-state, mention counters, and
/// `needs_attention` are independent of archive. Callers wrap in their own
/// transaction so a multi-account fan-out commits atomically.
pub fn mark_archived(
    conn: &Connection,
    account_id: i64,
    pull_request_id: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, relation_observed_at, archived_at)
            VALUES (?1, ?2, strftime('%s','now'), strftime('%s','now'))
         ON CONFLICT(account_id, pull_request_id) DO UPDATE SET
            archived_at = strftime('%s','now')",
        params![account_id, pull_request_id],
    )?;
    Ok(())
}

/// Clear `archived_at` (set back to NULL) for one `(account_id,
/// pull_request_id)` pair. Mirrors [`mark_archived`]'s UPSERT shape so the
/// command can be invoked against a PR the viewer has never opened; the row
/// is created with the archive column already null, which is a no-op from
/// the dashboard query's perspective. Other columns follow the same
/// preservation rule as [`mark_archived`].
pub fn mark_unarchived(
    conn: &Connection,
    account_id: i64,
    pull_request_id: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, relation_observed_at, archived_at)
            VALUES (?1, ?2, strftime('%s','now'), NULL)
         ON CONFLICT(account_id, pull_request_id) DO UPDATE SET
            archived_at = NULL",
        params![account_id, pull_request_id],
    )?;
    Ok(())
}

/// Run the auto-archive sweep once. Sets `archived_at = now` on every
/// `pull_request_viewer_relations` row whose underlying PR is closed or
/// merged. Idempotent: the `archived_at IS NULL` predicate skips
/// already-archived rows so re-runs produce the same result as the first
/// run.
///
/// Account-agnostic by design (ADR 0018, decision 2 - revised post-M6):
/// the predicate depends only on `pull_requests.state`, so the sweep
/// writes across every relation owner in one statement instead of
/// fanning per-account. Returns the number of rows archived this call.
///
/// The original 30-day inactivity TTL was dropped during M6 smoke
/// feedback: closed/merged PRs are now hidden from default views by a
/// `pr.state = 'open'` predicate in [`crate::dashboard::query`], so
/// leaving them around in default views during the 30-day inactivity
/// window served no UX purpose. Immediate archive routes them straight
/// to the Archive view, and [`archive_retention_sweep`] hard-deletes
/// them 60 days later.
pub fn auto_archive_sweep(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET archived_at = strftime('%s','now')
          WHERE archived_at IS NULL
            AND pull_request_id IN (
                SELECT id FROM pull_requests
                 WHERE state IN ('closed', 'merged')
            )",
        [],
    )
}

/// Hard-delete PRs whose every viewer relation has been archived for more
/// than 60 days, plus everything that cascades from them (review_threads,
/// review_comments, issue_comments, timeline_events, reviews,
/// requested_reviewers, pull_request_viewer_relations). Bounds local DB
/// growth so long-running PRism installs don't accumulate years of merged
/// PRs.
///
/// Eligibility predicate (per the post-M6 smoke feedback):
///
/// 1. The PR has at least one viewer relation row (Tracked-view PRs with no
///    relations are someone-else's territory; the retention sweep stays
///    out of that lane).
/// 2. Every relation row has `archived_at IS NOT NULL`.
/// 3. Every relation row's `archived_at` is older than 60 days.
///
/// Returns the number of `pull_requests` rows deleted. Cascade-deleted
/// rows in related tables are not counted (SQLite's cascade is silent).
///
/// Runs in its own transaction one cycle after the auto-archive sweep
/// inside [`crate::sync::worker`]. A failure aborts the transaction but
/// doesn't propagate up - the cleanup is a best-effort housekeeping pass.
pub fn archive_retention_sweep(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "DELETE FROM pull_requests
          WHERE id IN (
              SELECT pr.id
                FROM pull_requests pr
               WHERE EXISTS (
                       SELECT 1 FROM pull_request_viewer_relations rel
                        WHERE rel.pull_request_id = pr.id
                     )
                 AND NOT EXISTS (
                       SELECT 1 FROM pull_request_viewer_relations rel
                        WHERE rel.pull_request_id = pr.id
                          AND (rel.archived_at IS NULL
                               OR rel.archived_at >= strftime('%s','now','-60 days'))
                     )
          )",
        [],
    )
}

/// Recompute the `pull_request_viewer_relations.needs_attention` boolean for
/// one `(account_id, pull_request_id)` pair using the four ADR-0015 signals:
///
/// 1. Viewer authored the PR AND at least one unresolved thread is involved
///    by the viewer (a `review_comments` row authored by the viewer's
///    account login on an unresolved `review_threads` row). Per ADR 0016
///    the involvement test reads `review_threads` / `review_comments`
///    directly instead of the retired `pr.threads_unresolved_involved`
///    column.
/// 2. Viewer is in `requested_reviewers` for the PR (presence implies pending;
///    the table never stores submitted reviews - those flow through
///    `reviews`).
/// 3. `mentioned_count_unread > 0` for the (account, PR) pair.
/// 4. Viewer authored the PR AND `review_decision = 'CHANGES_REQUESTED'`.
///
/// The UPDATE is a no-op when the relation row doesn't exist for the pair
/// (Tracked-view PRs never get a row - see contract). Callers that need the row
/// present should UPSERT first.
///
/// Returns a (possibly empty) [`Vec<NotificationTrigger>`] describing the
/// transitions observed in this call (ADR 0017 decision 1). Callers (the sync
/// worker, the read/unread commands, the conversation hydrator) hand the
/// triggers to the [`NotificationSink`](crate::notify::NotificationSink) once
/// the enclosing transaction commits - dispatching from inside the helper
/// would either hold the DB lock during the OS plugin call or fire a toast
/// for a write that later rolled back.
///
/// `previous_mentioned_count` records the mention counter value from before
/// any writes the caller applied in the same transaction. When `Some(n)`, a
/// strict increase between `n` and the post-UPDATE value produces a Mention
/// trigger; `None` disables Mention-trigger detection (the read / unread
/// commands' transactions don't care about increases, only resets). The
/// `needs_attention` 0 -> 1 transition is detected against the helper's own
/// entry snapshot, so callers don't pass a baseline for it.
pub fn recompute_needs_attention(
    conn: &Connection,
    account_id: i64,
    pull_request_id: i64,
    previous_mentioned_count: Option<i64>,
) -> Result<Vec<NotificationTrigger>, rusqlite::Error> {
    // Snapshot the pre-UPDATE `needs_attention`. The Mention transition is
    // measured against the explicit baseline above, not this snapshot,
    // because the helper's own UPDATE doesn't change the counter. A missing
    // relation row reads as None - no transition, no trigger.
    let before_attention: Option<i64> = conn
        .query_row(
            "SELECT needs_attention
               FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pull_request_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;

    conn.execute(
        "UPDATE pull_request_viewer_relations AS rel
            SET needs_attention = (
                SELECT CASE WHEN
                    EXISTS (
                        SELECT 1
                          FROM pull_requests pr
                          JOIN accounts a ON a.id = rel.account_id
                          JOIN review_threads t ON t.pull_request_id = pr.id
                         WHERE pr.id = rel.pull_request_id
                           AND pr.author_login = a.login
                           AND t.is_resolved = 0
                           AND EXISTS (
                               SELECT 1 FROM review_comments c
                                JOIN accounts a2 ON a2.login = c.author_login
                                WHERE c.review_thread_id = t.id
                                  AND a2.id = rel.account_id
                           )
                    )
                    OR EXISTS (
                        SELECT 1
                          FROM requested_reviewers rr
                          JOIN accounts a ON a.id = rel.account_id
                         WHERE rr.pull_request_id = rel.pull_request_id
                           AND rr.login = a.login
                    )
                    OR rel.mentioned_count_unread > 0
                    OR EXISTS (
                        SELECT 1
                          FROM pull_requests pr
                          JOIN accounts a ON a.id = rel.account_id
                         WHERE pr.id = rel.pull_request_id
                           AND pr.author_login = a.login
                           AND pr.review_decision = 'CHANGES_REQUESTED'
                    )
                THEN 1 ELSE 0 END
            )
          WHERE rel.account_id = ?1
            AND rel.pull_request_id = ?2",
        params![account_id, pull_request_id],
    )?;

    let Some(before_attention) = before_attention else {
        return Ok(Vec::new());
    };

    let after: Option<(i64, i64)> = conn
        .query_row(
            "SELECT needs_attention, mentioned_count_unread
               FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pull_request_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let Some((after_attention, after_mentions)) = after else {
        return Ok(Vec::new());
    };

    let mut triggers = Vec::new();
    if before_attention == 0 && after_attention == 1 {
        triggers.push(NotificationTrigger {
            account_id,
            pull_request_id,
            kind: NotificationKind::NeedsAttention,
        });
    }
    // Strict increase fires one Mention trigger regardless of the delta size:
    // a single 0 -> 2 jump is one event from the user's perspective, not two
    // (ADR 0017 decision 1).
    if let Some(prev) = previous_mentioned_count {
        if after_mentions > prev {
            triggers.push(NotificationTrigger {
                account_id,
                pull_request_id,
                kind: NotificationKind::Mention,
            });
        }
    }
    Ok(triggers)
}

/// Mark every relation row matching the active view + chip filter as read.
///
/// Issue #336: a "Mark all read" command that bulk-flips the read state for
/// every PR the user sees in the active dashboard view. The write follows the
/// same per-row semantics as [`mark_read`] (sets `read_at`, snapshots
/// `pull_requests.updated_at` into `read_pr_updated_at`, resets the mention
/// counter, advances the mention scan watermark) but applied to every
/// relation row that survives the view + chip predicate.
///
/// `account_id = Some(id)` flips only the active account's relation rows.
/// `account_id = None` (ADR 0016 unified mode) flips every relation row for
/// every PR in the view. The Tracked view's PRs without a relation row are
/// not affected - mark-all-read doesn't UPSERT new rows (use the per-row
/// `mark_pr_read` for that case).
///
/// After the read flip, recomputes `needs_attention` on the same row set so a
/// PR whose only attention signal was an unread mention drops out of the
/// attention bucket in the same transaction. Per ADR 0017 decision 1, read
/// clears don't surface as notifications, so this helper doesn't emit
/// notification triggers.
///
/// Returns the number of distinct PRs whose relation rows the call touched.
/// In single-account mode that equals the rows-affected count of the read
/// flip UPDATE (one relation per PR per account). In unified mode the
/// number collapses multi-account relations into one row per PR so the
/// frontend can report "marked N PRs" rather than "wrote N relation rows".
///
/// Callers wrap the call in a transaction so the read flip and the recompute
/// commit together. See [`crate::triage::commands::mark_view_read`].
pub fn mark_view_read(
    conn: &Connection,
    view: DashboardView,
    account_id: Option<i64>,
    active_chips: &[ChipKey],
) -> Result<i64, rusqlite::Error> {
    let (matching_pr_ids_sql, base_params) = matching_pr_ids_subquery(view, account_id);
    let chip_clause = mark_view_chip_clause(active_chips);

    let in_clause = if chip_clause.is_empty() {
        matching_pr_ids_sql
    } else {
        format!("{matching_pr_ids_sql} {chip_clause}")
    };

    // ADR 0018: default views target unarchived relations; Archive view
    // targets the archived ones. The dashboard query encodes the same flip
    // through its view-specific FROM clauses; we encode it on the outer
    // UPDATE's `rel` because the IN sub-query only enumerates PR ids.
    let archive_filter = match view {
        DashboardView::Archive => "AND rel.archived_at IS NOT NULL",
        _ => "AND rel.archived_at IS NULL",
    };
    let account_scope = match account_id {
        Some(_) => "AND rel.account_id = ?1",
        None => "",
    };

    // Distinct PR count - mirrors what the frontend reports back to the user.
    // Counts against the same WHERE the UPDATEs use so a PR with no relation
    // row matching the account/archive scope doesn't inflate the result
    // (Tracked view with no relation for the active account is the canonical
    // case).
    let count_sql = format!(
        "SELECT COUNT(DISTINCT rel.pull_request_id)
           FROM pull_request_viewer_relations rel
          WHERE rel.pull_request_id IN (
              SELECT DISTINCT pr.id {in_clause}
          ) {account_scope}
            {archive_filter}"
    );
    let distinct_pr_count: i64 =
        conn.query_row(&count_sql, params_from_iter(base_params.iter()), |row| {
            row.get(0)
        })?;
    if distinct_pr_count == 0 {
        return Ok(0);
    }

    // Read flip. The `read_pr_updated_at` snapshot uses a correlated subquery
    // against `pull_requests` so the watermark matches the per-row `mark_read`
    // behaviour. The outer WHERE adds the account + archive scope so the
    // UPDATE never touches relations the active view wouldn't have shown.
    let read_flip_sql = format!(
        "UPDATE pull_request_viewer_relations AS rel
            SET read_at                    = strftime('%s','now'),
                read_pr_updated_at         = (SELECT pr.updated_at
                                                FROM pull_requests pr
                                               WHERE pr.id = rel.pull_request_id),
                mentioned_count_unread     = 0,
                mention_scan_watermark_at  = strftime('%s','now')
          WHERE rel.pull_request_id IN (
              SELECT DISTINCT pr.id {in_clause}
          ) {account_scope}
            {archive_filter}"
    );
    conn.execute(&read_flip_sql, params_from_iter(base_params.iter()))?;

    // Bulk needs_attention recompute against the same row set. Same CASE
    // WHEN as `recompute_needs_attention` so the four ADR-0015 signals stay
    // in step. The mention counter just dropped to zero, so signal 3 will
    // miss; signals 1, 2, and 4 still resolve from the relation row's
    // surrounding data.
    let recompute_sql = format!(
        "UPDATE pull_request_viewer_relations AS rel
            SET needs_attention = (
                SELECT CASE WHEN
                    EXISTS (
                        SELECT 1
                          FROM pull_requests pr
                          JOIN accounts a ON a.id = rel.account_id
                          JOIN review_threads t ON t.pull_request_id = pr.id
                         WHERE pr.id = rel.pull_request_id
                           AND pr.author_login = a.login
                           AND t.is_resolved = 0
                           AND EXISTS (
                               SELECT 1 FROM review_comments c
                                JOIN accounts a2 ON a2.login = c.author_login
                                WHERE c.review_thread_id = t.id
                                  AND a2.id = rel.account_id
                           )
                    )
                    OR EXISTS (
                        SELECT 1
                          FROM requested_reviewers rr
                          JOIN accounts a ON a.id = rel.account_id
                         WHERE rr.pull_request_id = rel.pull_request_id
                           AND rr.login = a.login
                    )
                    OR rel.mentioned_count_unread > 0
                    OR EXISTS (
                        SELECT 1
                          FROM pull_requests pr
                          JOIN accounts a ON a.id = rel.account_id
                         WHERE pr.id = rel.pull_request_id
                           AND pr.author_login = a.login
                           AND pr.review_decision = 'CHANGES_REQUESTED'
                    )
                THEN 1 ELSE 0 END
            )
          WHERE rel.pull_request_id IN (
              SELECT DISTINCT pr.id {in_clause}
          ) {account_scope}
            {archive_filter}"
    );
    conn.execute(&recompute_sql, params_from_iter(base_params.iter()))?;

    Ok(distinct_pr_count)
}

/// FROM + WHERE for a `SELECT DISTINCT pr.id ...` subquery that enumerates
/// every PR id the dashboard's per-view query would surface. Mirrors the
/// shape used by [`crate::dashboard::query`] so a row that lands in the list
/// also lands in this set.
///
/// Single-account paths bind `?1` for the active account id; unified paths
/// take no parameters. The Tracked view's single-account path includes PRs
/// that don't have a relation row (the LEFT-JOINed `rel` is NULL for those);
/// the caller's UPDATE then matches zero rows for them, which is the right
/// behaviour - bulk mark-read doesn't UPSERT, and per-row `mark_pr_read`
/// handles the relation-less Tracked case.
fn matching_pr_ids_subquery(view: DashboardView, account_id: Option<i64>) -> (String, Vec<i64>) {
    match (view, account_id) {
        (DashboardView::Authored, Some(id)) => (
            "FROM pull_request_viewer_relations rel
             JOIN pull_requests pr ON pr.id = rel.pull_request_id
             WHERE rel.is_authored = 1
               AND rel.account_id = ?1
               AND rel.archived_at IS NULL
               AND pr.state = 'open'"
                .to_string(),
            vec![id],
        ),
        (DashboardView::Assigned, Some(id)) => (
            "FROM pull_request_viewer_relations rel
             JOIN pull_requests pr ON pr.id = rel.pull_request_id
             WHERE rel.is_review_requested = 1
               AND rel.account_id = ?1
               AND rel.archived_at IS NULL
               AND pr.state = 'open'"
                .to_string(),
            vec![id],
        ),
        (DashboardView::Watching, Some(id)) => (
            "FROM pull_request_viewer_relations rel
             JOIN pull_requests pr ON pr.id = rel.pull_request_id
             WHERE rel.is_involved = 1
               AND rel.account_id = ?1
               AND rel.archived_at IS NULL
               AND pr.state = 'open'"
                .to_string(),
            vec![id],
        ),
        (DashboardView::Tracked, Some(id)) => (
            "FROM pull_requests pr
             JOIN repos r ON r.id = pr.repo_id
             LEFT JOIN pull_request_viewer_relations rel
                ON rel.pull_request_id = pr.id
               AND rel.account_id = ?1
               AND rel.archived_at IS NULL
             WHERE r.is_tracked = 1
               AND r.account_id = ?1
               AND pr.state = 'open'"
                .to_string(),
            vec![id],
        ),
        (DashboardView::Archive, Some(id)) => (
            "FROM pull_request_viewer_relations rel
             JOIN pull_requests pr ON pr.id = rel.pull_request_id
             WHERE rel.account_id = ?1
               AND rel.archived_at IS NOT NULL"
                .to_string(),
            vec![id],
        ),
        (DashboardView::Authored, None) => (
            "FROM pull_requests pr
             LEFT JOIN pull_request_viewer_relations rel
                ON rel.pull_request_id = pr.id
               AND rel.archived_at IS NULL
             WHERE pr.state = 'open'
               AND EXISTS (
                   SELECT 1 FROM pull_request_viewer_relations rel_filter
                    WHERE rel_filter.pull_request_id = pr.id
                      AND rel_filter.is_authored = 1
                      AND rel_filter.archived_at IS NULL
               )"
            .to_string(),
            Vec::new(),
        ),
        (DashboardView::Assigned, None) => (
            "FROM pull_requests pr
             LEFT JOIN pull_request_viewer_relations rel
                ON rel.pull_request_id = pr.id
               AND rel.archived_at IS NULL
             WHERE pr.state = 'open'
               AND EXISTS (
                   SELECT 1 FROM pull_request_viewer_relations rel_filter
                    WHERE rel_filter.pull_request_id = pr.id
                      AND rel_filter.is_review_requested = 1
                      AND rel_filter.archived_at IS NULL
               )"
            .to_string(),
            Vec::new(),
        ),
        (DashboardView::Watching, None) => (
            "FROM pull_requests pr
             LEFT JOIN pull_request_viewer_relations rel
                ON rel.pull_request_id = pr.id
               AND rel.archived_at IS NULL
             WHERE pr.state = 'open'
               AND EXISTS (
                   SELECT 1 FROM pull_request_viewer_relations rel_filter
                    WHERE rel_filter.pull_request_id = pr.id
                      AND rel_filter.is_involved = 1
                      AND rel_filter.archived_at IS NULL
               )"
            .to_string(),
            Vec::new(),
        ),
        (DashboardView::Tracked, None) => (
            "FROM pull_requests pr
             JOIN repos r ON r.id = pr.repo_id
             LEFT JOIN pull_request_viewer_relations rel
                ON rel.pull_request_id = pr.id
               AND rel.archived_at IS NULL
             WHERE r.is_tracked = 1
               AND pr.state = 'open'
               AND (NOT EXISTS (
                       SELECT 1 FROM pull_request_viewer_relations rel_any
                        WHERE rel_any.pull_request_id = pr.id
                    )
                    OR EXISTS (
                       SELECT 1 FROM pull_request_viewer_relations rel_un
                        WHERE rel_un.pull_request_id = pr.id
                          AND rel_un.archived_at IS NULL
                    ))"
            .to_string(),
            Vec::new(),
        ),
        (DashboardView::Archive, None) => (
            "FROM pull_requests pr
             LEFT JOIN pull_request_viewer_relations rel
                ON rel.pull_request_id = pr.id
               AND rel.archived_at IS NOT NULL
             WHERE EXISTS (
                 SELECT 1 FROM pull_request_viewer_relations rel_filter
                  WHERE rel_filter.pull_request_id = pr.id
                    AND rel_filter.archived_at IS NOT NULL
             )"
            .to_string(),
            Vec::new(),
        ),
    }
}

/// AND-compose the chip predicates for the active chip set. Mirrors the
/// dashboard query's `chip_where_clause` helper (which is module-private
/// there) without coupling the triage write path to the dashboard module.
fn mark_view_chip_clause(active_chips: &[ChipKey]) -> String {
    if active_chips.is_empty() {
        return String::new();
    }
    let mut clause = String::new();
    for chip in active_chips {
        clause.push_str(" AND (");
        clause.push_str(chip_predicate(*chip));
        clause.push(')');
    }
    clause
}

/// SQL predicate fragment for `chip` in the chip-count and chip-WHERE-composition
/// queries. The `rel.*` references resolve against the
/// `pull_request_viewer_relations` row joined to the active account; callers
/// must ensure the join is present before composing this fragment in.
///
/// The `Stale` predicate hard-codes the 7-day window pinned in the contract
/// (`604800` seconds); the value lives in one place so a future revision only
/// edits this string.
///
/// `UnresolvedThreads` reads `review_threads` directly (ADR 0016) since the
/// `pr.threads_*` columns are no longer maintained. The predicate is
/// account-independent: any unresolved thread on the PR is enough.
pub fn chip_predicate(chip: ChipKey) -> &'static str {
    match chip {
        ChipKey::NeedsAttention => "rel.needs_attention = 1",
        ChipKey::UnresolvedThreads => {
            "EXISTS (SELECT 1 FROM review_threads t \
                      WHERE t.pull_request_id = pr.id AND t.is_resolved = 0)"
        }
        ChipKey::CiFailing => "pr.ci_state IN ('FAILURE', 'ERROR')",
        ChipKey::Stale => "(strftime('%s','now') - pr.updated_at) > 604800",
        ChipKey::Drafts => "pr.is_draft = 1",
    }
}

/// Common view-scoped FROM clause for the chip-count SELECTs. Joins
/// `pull_requests` with the account's relation row so every chip predicate
/// (including `rel.needs_attention = 1`) can reference `rel.*` uniformly.
///
/// Single-account (`Some(id)`):
/// Authored / Assigned / Watching gate on the matching relation flag against
/// the active account; the LEFT JOIN inside the tracked variant lets
/// `rel.needs_attention` resolve to NULL (which the count predicate then
/// ignores) when the active account has no relation row for a Tracked-view PR.
///
/// Unified (`None`) (ADR 0016, issue #171):
/// Mirrors the dashboard query's union-mode FROM shape so the chip-count and
/// the chip-filtered dashboard list see the same row set. The view filter for
/// Authored / Assigned / Watching becomes an EXISTS sub-query (matching any
/// tracked account's relation row); Tracked stays gated on `r.is_tracked`.
/// The relation is brought in via LEFT JOIN _without_ an account predicate so
/// `rel.needs_attention` resolves against every relation owner, mirroring the
/// dashboard query's `MAX(needs_attention)` semantics. The chip predicate is
/// applied after this join, and the caller wraps the SELECT in
/// `COUNT(DISTINCT pr.id)` so a PR matched via two relation rows still counts
/// as one.
///
/// The single-account path uses `?1` for the account id; the union path takes
/// no parameters. The caller binds the matching length-0 or length-1 vector
/// when running the prepared statement.
fn chip_count_from_clause(view: DashboardView, account_id: Option<i64>) -> &'static str {
    // ADR 0018, decision 5: chip counts exclude archived rows so the chip
    // numbers match the dashboard list shapes. Single-account paths add
    // `AND rel.archived_at IS NULL` to the WHERE for the relation-backed
    // views and to the LEFT JOIN's ON clause for the Tracked view. Unified
    // paths mirror the dashboard query's `archived_at IS NULL` predicate on
    // both the EXISTS sub-query and the LEFT JOIN.
    //
    // The Archive view (ADR 0018) is intentionally not part of this match.
    // The chip set doesn't apply to the archive surface in this PR (see
    // `archive_view_query` in `dashboard::query` for the rationale); the
    // frontend hides the chip rail on the archive route in W2. Reaching here
    // with `DashboardView::Archive` would mean a caller bypassed that guard;
    // the explicit panic surfaces the misuse during development.
    match (view, account_id) {
        (DashboardView::Authored, Some(_)) => {
            "FROM pull_request_viewer_relations rel
             JOIN pull_requests pr ON pr.id = rel.pull_request_id
             WHERE rel.is_authored = 1
               AND rel.account_id = ?1
               AND rel.archived_at IS NULL"
        }
        (DashboardView::Assigned, Some(_)) => {
            "FROM pull_request_viewer_relations rel
             JOIN pull_requests pr ON pr.id = rel.pull_request_id
             WHERE rel.is_review_requested = 1
               AND rel.account_id = ?1
               AND rel.archived_at IS NULL"
        }
        (DashboardView::Watching, Some(_)) => {
            "FROM pull_request_viewer_relations rel
             JOIN pull_requests pr ON pr.id = rel.pull_request_id
             WHERE rel.is_involved = 1
               AND rel.account_id = ?1
               AND rel.archived_at IS NULL"
        }
        (DashboardView::Tracked, Some(_)) => {
            "FROM pull_requests pr
             JOIN repos r ON r.id = pr.repo_id
             LEFT JOIN pull_request_viewer_relations rel
                ON rel.pull_request_id = pr.id
               AND rel.account_id = ?1
               AND rel.archived_at IS NULL
             WHERE r.is_tracked = 1 AND r.account_id = ?1"
        }
        (DashboardView::Authored, None) => {
            "FROM pull_requests pr
             LEFT JOIN pull_request_viewer_relations rel
                ON rel.pull_request_id = pr.id
               AND rel.archived_at IS NULL
             WHERE EXISTS (
                SELECT 1 FROM pull_request_viewer_relations rel_filter
                 WHERE rel_filter.pull_request_id = pr.id
                   AND rel_filter.is_authored = 1
                   AND rel_filter.archived_at IS NULL
             )"
        }
        (DashboardView::Assigned, None) => {
            "FROM pull_requests pr
             LEFT JOIN pull_request_viewer_relations rel
                ON rel.pull_request_id = pr.id
               AND rel.archived_at IS NULL
             WHERE EXISTS (
                SELECT 1 FROM pull_request_viewer_relations rel_filter
                 WHERE rel_filter.pull_request_id = pr.id
                   AND rel_filter.is_review_requested = 1
                   AND rel_filter.archived_at IS NULL
             )"
        }
        (DashboardView::Watching, None) => {
            "FROM pull_requests pr
             LEFT JOIN pull_request_viewer_relations rel
                ON rel.pull_request_id = pr.id
               AND rel.archived_at IS NULL
             WHERE EXISTS (
                SELECT 1 FROM pull_request_viewer_relations rel_filter
                 WHERE rel_filter.pull_request_id = pr.id
                   AND rel_filter.is_involved = 1
                   AND rel_filter.archived_at IS NULL
             )"
        }
        (DashboardView::Tracked, None) => {
            "FROM pull_requests pr
             JOIN repos r ON r.id = pr.repo_id
             LEFT JOIN pull_request_viewer_relations rel
                ON rel.pull_request_id = pr.id
               AND rel.archived_at IS NULL
             WHERE r.is_tracked = 1
               AND (NOT EXISTS (
                       SELECT 1 FROM pull_request_viewer_relations rel_any
                        WHERE rel_any.pull_request_id = pr.id
                    )
                    OR EXISTS (
                       SELECT 1 FROM pull_request_viewer_relations rel_un
                        WHERE rel_un.pull_request_id = pr.id
                          AND rel_un.archived_at IS NULL
                    ))"
        }
        (DashboardView::Archive, _) => {
            // Archive view has no chip rail in this PR; reaching here is a
            // programmer error.
            panic!("chip counts are not supported for the Archive view")
        }
    }
}

/// Run one chip's count SELECT against the view-scoped FROM clause.
///
/// Single-account uses `COUNT(*)` because the FROM has at most one relation
/// row per PR (the active account's), so per-row counts and per-PR counts
/// agree byte-identical with the pre-#171 behaviour.
///
/// The unified path LEFT JOINs every relation row for the PR, so a PR matched
/// by two accounts can produce two rows. `COUNT(DISTINCT pr.id)` collapses
/// those back to one, matching the dashboard query's union-path `GROUP BY
/// pr.id` row count.
fn count_chip(
    conn: &Connection,
    view: DashboardView,
    account_id: Option<i64>,
    chip: ChipKey,
) -> Result<i64, rusqlite::Error> {
    let from_and_where = chip_count_from_clause(view, account_id);
    let predicate = chip_predicate(chip);
    let projection = if account_id.is_some() {
        "COUNT(*)"
    } else {
        "COUNT(DISTINCT pr.id)"
    };
    let sql = format!("SELECT {projection} {from_and_where} AND {predicate}");
    let params: Vec<i64> = account_id.map_or_else(Vec::new, |id| vec![id]);
    conn.query_row(&sql, params_from_iter(params.iter()), |row| row.get(0))
}

/// Project the five chip counts for the active view + account scope. Each
/// count is independent of the other chips per the contract's "Counts rule":
/// the number returned for chip `X` is the count of PRs that would match if
/// `X` were toggled alone within the view scope.
///
/// `account_id = Some(id)` keeps the single-account behaviour byte-identical
/// to before #171. `account_id = None` (ADR 0016 unified default) fans the
/// count across every tracked account and dedupes by `pr.id` so a PR matched
/// via two accounts still contributes one to each chip it triggers.
///
/// `DashboardView::Archive` (ADR 0018) short-circuits to zeros. The archive
/// view doesn't expose the chip rail in this PR; the W2 frontend hides the
/// chip controls on the archive route. Returning zeros keeps the command
/// shape uniform if a caller plumbs through the view without checking.
pub fn list_filter_chip_counts(
    conn: &Connection,
    view: DashboardView,
    account_id: Option<i64>,
) -> Result<FilterChipCounts, rusqlite::Error> {
    if matches!(view, DashboardView::Archive) {
        return Ok(FilterChipCounts {
            needs_attention: 0,
            unresolved_threads: 0,
            ci_failing: 0,
            stale: 0,
            drafts: 0,
        });
    }
    Ok(FilterChipCounts {
        needs_attention: count_chip(conn, view, account_id, ChipKey::NeedsAttention)?,
        unresolved_threads: count_chip(conn, view, account_id, ChipKey::UnresolvedThreads)?,
        ci_failing: count_chip(conn, view, account_id, ChipKey::CiFailing)?,
        stale: count_chip(conn, view, account_id, ChipKey::Stale)?,
        drafts: count_chip(conn, view, account_id, ChipKey::Drafts)?,
    })
}

/// Count PRs whose `pull_request_viewer_relations.needs_attention = 1` for
/// the given account, bucketed by the four dashboard views. The partial
/// index `idx_pr_viewer_relations_attention` keeps the per-account scan to
/// the attention rows; the per-view buckets then gate by the matching
/// relation flag or, for Tracked, the repo's `is_tracked = 1` AND
/// `account_id = ?` predicates the dashboard Tracked view uses.
///
/// The Tracked count is account-scoped because `needs_attention` is itself a
/// per-account signal: even though the Tracked view's row set comes from
/// `is_tracked` repos rather than relation flags, the attention column is
/// only meaningful for the active account. The repo-owner predicate matches
/// the Tracked view's `r.account_id = ?` filter so the badge can't over-count
/// cross-account relation rows.
///
/// ADR 0018, decision 5: archived rows do not contribute to attention totals.
/// `rel.archived_at IS NULL` gates the per-account scan so an archived PR
/// keeps `needs_attention = 1` on disk (the recompute path stays archive-
/// agnostic) but stops boosting the sidebar count chip.
pub fn count_sidebar_attention(
    conn: &Connection,
    account_id: i64,
) -> Result<SidebarAttentionCounts, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT
            SUM(CASE WHEN rel.is_authored = 1         THEN 1 ELSE 0 END) AS authored,
            SUM(CASE WHEN rel.is_review_requested = 1 THEN 1 ELSE 0 END) AS assigned,
            SUM(CASE WHEN rel.is_involved = 1         THEN 1 ELSE 0 END) AS watching,
            SUM(CASE
                    WHEN EXISTS (
                        SELECT 1 FROM pull_requests pr
                          JOIN repos r ON r.id = pr.repo_id
                         WHERE pr.id = rel.pull_request_id
                           AND r.is_tracked = 1
                           AND r.account_id = ?1
                    ) THEN 1 ELSE 0 END) AS tracked
           FROM pull_request_viewer_relations rel
          WHERE rel.account_id = ?1
            AND rel.needs_attention = 1
            AND rel.archived_at IS NULL",
    )?;
    let counts = stmt.query_row(params![account_id], |row| {
        Ok(SidebarAttentionCounts {
            authored: row.get::<_, Option<i64>>(0)?.unwrap_or(0),
            assigned: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
            watching: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            tracked: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
        })
    })?;
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh_db() -> Connection {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        conn
    }

    /// Seed one account / repo / PR / relation row. When
    /// `unresolved_involved_thread` is true the fixture also inserts one
    /// unresolved `review_threads` row with a `review_comments` row authored
    /// by the viewer - so the ADR-0016 query-time involvement check finds the
    /// viewer involved on an unresolved thread.
    fn seed_account_repo_pr(
        conn: &Connection,
        viewer_login: &str,
        author_login: &str,
        unresolved_involved_thread: bool,
        review_decision: Option<&str>,
    ) {
        conn.execute_batch(&format!(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', '{viewer_login}', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref, review_decision)
                VALUES (100, 10, 1, 't', 'open', 0, '{author_login}',
                        0, 0, 'main', 'feat',
                        {review_decision_sql});
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at)
                VALUES (1, 100, 0, 0, 0, 0);",
            review_decision_sql = match review_decision {
                Some(s) => format!("'{s}'"),
                None => "NULL".to_string(),
            }
        ))
        .unwrap();
        if unresolved_involved_thread {
            conn.execute_batch(&format!(
                "INSERT INTO review_threads
                    (id, pull_request_id, is_resolved, is_outdated, node_id)
                    VALUES (5001, 100, 0, 0, 'RT_seed');
                 INSERT INTO review_comments
                    (id, review_thread_id, author_login, body, created_at)
                    VALUES (6001, 5001, '{viewer_login}', 'note', 1);"
            ))
            .unwrap();
        }
    }

    fn read_needs_attention(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT needs_attention FROM pull_request_viewer_relations
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn signal_one_authored_with_unresolved_involved_threads() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "alice", true, None);
        recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert_eq!(read_needs_attention(&conn), 1);
    }

    #[test]
    fn signal_one_no_fire_when_not_author() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", true, None);
        recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert_eq!(read_needs_attention(&conn), 0);
    }

    #[test]
    fn signal_two_pending_requested_reviewer() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", false, None);
        conn.execute(
            "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                VALUES (100, 'alice', 'user')",
            [],
        )
        .unwrap();
        recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert_eq!(read_needs_attention(&conn), 1);
    }

    #[test]
    fn signal_two_no_fire_for_other_reviewers() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", false, None);
        conn.execute(
            "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                VALUES (100, 'carol', 'user')",
            [],
        )
        .unwrap();
        recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert_eq!(read_needs_attention(&conn), 0);
    }

    #[test]
    fn signal_three_unread_mentions() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", false, None);
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET mentioned_count_unread = 2
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();
        recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert_eq!(read_needs_attention(&conn), 1);
    }

    #[test]
    fn signal_four_changes_requested_on_authored_pr() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "alice", false, Some("CHANGES_REQUESTED"));
        recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert_eq!(read_needs_attention(&conn), 1);
    }

    #[test]
    fn signal_four_no_fire_when_not_author() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", false, Some("CHANGES_REQUESTED"));
        recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert_eq!(read_needs_attention(&conn), 0);
    }

    #[test]
    fn negative_no_signals_clears_flag() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", false, None);
        // Pre-set the flag so the recompute has to actively clear it.
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET needs_attention = 1
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();
        recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert_eq!(read_needs_attention(&conn), 0);
    }

    #[test]
    fn combined_signals_one_and_three_still_fire() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "alice", true, None);
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET mentioned_count_unread = 3
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();
        recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert_eq!(read_needs_attention(&conn), 1);
    }

    #[test]
    fn missing_relation_row_is_a_noop() {
        let conn = fresh_db();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 0, 'alice', 0, 0, 'main', 'feat');",
        )
        .unwrap();
        // No relations row for (1, 100) - the UPDATE should still succeed
        // and touch zero rows.
        let triggers = recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert!(
            triggers.is_empty(),
            "missing relation row must not emit triggers"
        );
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 0);
    }

    // ===== notification trigger emission (ADR 0017 decision 1, issue #192) =====
    //
    // The recompute helper returns triggers describing transitions observed
    // in the same call. The sync worker / commands dispatch them to the
    // notification sink after the transaction commits.

    #[test]
    fn returns_needs_attention_trigger_on_zero_to_one_flip() {
        let conn = fresh_db();
        // Author == viewer + unresolved involved thread fires signal 1, so a
        // baseline-zero row flips to 1 on this call.
        seed_account_repo_pr(&conn, "alice", "alice", true, None);
        let triggers = recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].kind, NotificationKind::NeedsAttention);
        assert_eq!(triggers[0].account_id, 1);
        assert_eq!(triggers[0].pull_request_id, 100);
    }

    #[test]
    fn returns_empty_vec_when_already_at_one() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "alice", true, None);
        // First call flips 0 -> 1 and emits the trigger.
        let _ = recompute_needs_attention(&conn, 1, 100, None).unwrap();
        // Second call is a steady-state 1 -> 1 transition; no trigger.
        let triggers = recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert!(triggers.is_empty());
        assert_eq!(read_needs_attention(&conn), 1);
    }

    #[test]
    fn returns_mention_trigger_when_count_increases() {
        // A jump from 0 to 2 fires exactly one Mention trigger, not two -
        // the trigger is about "a new unread mention landed since the last
        // recompute" rather than "one toast per mention" (ADR 0017 decision 1).
        // The caller (mimicking the sync worker's mention scan that ran just
        // before this recompute) passes the pre-scan baseline; the helper
        // compares the baseline to the post-UPDATE row value.
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", false, None);
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET mentioned_count_unread = 2
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();
        let triggers = recompute_needs_attention(&conn, 1, 100, Some(0)).unwrap();
        // Both kinds fire: the counter jump emits a Mention trigger, and the
        // attention column moves 0 -> 1 because mentioned_count_unread > 0.
        let mention_count = triggers
            .iter()
            .filter(|t| t.kind == NotificationKind::Mention)
            .count();
        assert_eq!(mention_count, 1, "single trigger per recompute call");
    }

    #[test]
    fn returns_both_triggers_when_both_transitions_happen() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", false, None);
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET mentioned_count_unread = 1
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();
        let triggers = recompute_needs_attention(&conn, 1, 100, Some(0)).unwrap();
        assert_eq!(triggers.len(), 2);
        assert!(triggers
            .iter()
            .any(|t| t.kind == NotificationKind::NeedsAttention));
        assert!(triggers.iter().any(|t| t.kind == NotificationKind::Mention));
    }

    #[test]
    fn returns_no_mention_trigger_when_baseline_matches_post_value() {
        // The mark-read / mark-unread paths pass `previous_mentioned_count`
        // equal to the post-UPDATE value (they don't bump the counter on the
        // way in). The helper must not surface a phantom Mention trigger.
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", false, None);
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET mentioned_count_unread = 0
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();
        let triggers = recompute_needs_attention(&conn, 1, 100, Some(0)).unwrap();
        assert!(
            triggers.iter().all(|t| t.kind != NotificationKind::Mention),
            "steady-state counter must not emit a Mention trigger"
        );
    }

    #[test]
    fn returns_empty_vec_on_one_to_zero_clear() {
        // Clearing attention is not a notification event - the in-app badge
        // turning off is signal enough. Only the rising edge fires a toast.
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", false, None);
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET needs_attention = 1
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();
        let triggers = recompute_needs_attention(&conn, 1, 100, None).unwrap();
        assert!(triggers.is_empty());
        assert_eq!(read_needs_attention(&conn), 0, "row cleared back to 0");
    }

    // ===== chip-count tests (M4-D) =====

    /// Seed a Watching-view fixture covering each chip predicate exactly once.
    /// PR 600 is drafts-only, 601 is ci-failing, 602 is stale, 603 has
    /// unresolved threads (seeded via `review_threads` since ADR 0016 retired
    /// the pre-aggregated `pr.threads_*` columns), 604 has needs_attention
    /// precomputed.
    fn seed_chip_count_fixture(conn: &Connection) {
        conn.execute_batch(
            r#"
            INSERT INTO accounts (id, label, host, login, created_at) VALUES
                (1, 'alice-acct', 'github.com', 'alice', 0);

            INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
                (10, 1, 'alice', 'web', 'public');

            INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref, ci_state) VALUES
                (600, 10, 1, 'd',  'open', 1, 'bob', 0, strftime('%s','now'), 'main', 'a', NULL),
                (601, 10, 2, 'ci', 'open', 0, 'bob', 0, strftime('%s','now'), 'main', 'b', 'FAILURE'),
                (602, 10, 3, 'st', 'open', 0, 'bob', 0, strftime('%s','now') - 800000, 'main', 'c', NULL),
                (603, 10, 4, 'th', 'open', 0, 'bob', 0, strftime('%s','now'), 'main', 'd', NULL),
                (604, 10, 5, 'a',  'open', 0, 'bob', 0, strftime('%s','now'), 'main', 'e', NULL);

            INSERT INTO review_threads (id, pull_request_id, is_resolved, is_outdated, node_id)
                VALUES (700, 603, 0, 0, 'RT_603');

            INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at, needs_attention) VALUES
                (1, 600, 0, 0, 1, 0, 0),
                (1, 601, 0, 0, 1, 0, 0),
                (1, 602, 0, 0, 1, 0, 0),
                (1, 603, 0, 0, 1, 0, 0),
                (1, 604, 0, 0, 1, 0, 1);
            "#,
        )
        .unwrap();
    }

    /// Shared fixture for the sidebar count helper: one account (alice), three
    /// repos (one tracked), four PRs covering each view-flag combo.
    /// `needs_attention` is toggled in each test as needed.
    fn seed_sidebar_fixture(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility, is_tracked) VALUES
                (10, 1, 'alice', 'web', 'public', 0),
                (20, 1, 'alice', 'api', 'public', 1);
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref) VALUES
                (100, 10, 1, 'a', 'open', 0, 'alice', 0, 1, 'main', 'feat'),
                (200, 10, 2, 'b', 'open', 0, 'bob',   0, 1, 'main', 'feat'),
                (300, 10, 3, 'c', 'open', 0, 'carol', 0, 1, 'main', 'feat'),
                (400, 20, 1, 'd', 'open', 0, 'dave',  0, 1, 'main', 'feat');
             -- PR 100: authored
             -- PR 200: assigned (review-requested)
             -- PR 300: watching (involved only)
             -- PR 400: in a tracked repo, no direct flags
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at) VALUES
                (1, 100, 1, 0, 0, 0),
                (1, 200, 0, 1, 0, 0),
                (1, 300, 0, 0, 1, 0),
                (1, 400, 0, 0, 0, 0);",
        )
        .unwrap();
    }

    #[test]
    fn list_filter_chip_counts_projects_one_per_chip() {
        let conn = fresh_db();
        seed_chip_count_fixture(&conn);

        let counts = list_filter_chip_counts(&conn, DashboardView::Watching, Some(1)).unwrap();
        assert_eq!(counts.drafts, 1, "PR 600 is the only draft");
        assert_eq!(counts.ci_failing, 1, "PR 601 is the only CI-failing");
        assert_eq!(counts.stale, 1, "PR 602 is the only stale");
        assert_eq!(
            counts.unresolved_threads, 1,
            "PR 603 is the only unresolved"
        );
        assert_eq!(counts.needs_attention, 1, "PR 604 is the only attention");
    }

    #[test]
    fn list_filter_chip_counts_returns_zeros_on_empty_view() {
        let conn = fresh_db();
        // Account exists but no PRs / relations.
        conn.execute(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0)",
            [],
        )
        .unwrap();
        let counts = list_filter_chip_counts(&conn, DashboardView::Authored, Some(1)).unwrap();
        assert_eq!(counts.needs_attention, 0);
        assert_eq!(counts.unresolved_threads, 0);
        assert_eq!(counts.ci_failing, 0);
        assert_eq!(counts.stale, 0);
        assert_eq!(counts.drafts, 0);
    }

    #[test]
    fn list_filter_chip_counts_per_chip_is_independent_of_other_chips() {
        // Build a PR that matches both `drafts` and `ci_failing`. Each chip's
        // count must include that PR independently, not as the AND-intersection.
        let conn = fresh_db();
        conn.execute_batch(
            r#"
            INSERT INTO accounts (id, label, host, login, created_at) VALUES
                (1, 'a', 'github.com', 'alice', 0);

            INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
                (10, 1, 'alice', 'web', 'public');

            INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref, ci_state) VALUES
                (700, 10, 1, 'd+ci',  'open', 1, 'bob', 0, strftime('%s','now'), 'main', 'a', 'FAILURE'),
                (701, 10, 2, 'ci',    'open', 0, 'bob', 0, strftime('%s','now'), 'main', 'b', 'FAILURE'),
                (702, 10, 3, 'draft', 'open', 1, 'bob', 0, strftime('%s','now'), 'main', 'c', NULL);

            INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at) VALUES
                (1, 700, 0, 0, 1, 0),
                (1, 701, 0, 0, 1, 0),
                (1, 702, 0, 0, 1, 0);
            "#,
        )
        .unwrap();

        let counts = list_filter_chip_counts(&conn, DashboardView::Watching, Some(1)).unwrap();
        assert_eq!(counts.drafts, 2, "PRs 700 + 702 are drafts");
        assert_eq!(counts.ci_failing, 2, "PRs 700 + 701 have failing CI");
    }

    #[test]
    fn list_filter_chip_counts_scopes_by_account() {
        // Two accounts, separate watching sets. Counts must only reflect the
        // requested account's PRs.
        let conn = fresh_db();
        conn.execute_batch(
            r#"
            INSERT INTO accounts (id, label, host, login, created_at) VALUES
                (1, 'a', 'github.com', 'alice', 0),
                (2, 'b', 'github.com', 'bob',   0);

            INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
                (10, 1, 'alice', 'web', 'public');

            INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref) VALUES
                (800, 10, 1, 'alice-pr', 'open', 1, 'x', 0, strftime('%s','now'), 'main', 'a'),
                (801, 10, 2, 'bob-pr',   'open', 1, 'y', 0, strftime('%s','now'), 'main', 'b');

            INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at) VALUES
                (1, 800, 0, 0, 1, 0),
                (2, 801, 0, 0, 1, 0);
            "#,
        )
        .unwrap();

        let alice = list_filter_chip_counts(&conn, DashboardView::Watching, Some(1)).unwrap();
        let bob = list_filter_chip_counts(&conn, DashboardView::Watching, Some(2)).unwrap();
        assert_eq!(alice.drafts, 1);
        assert_eq!(bob.drafts, 1);
    }

    #[test]
    fn list_filter_chip_counts_tracked_view_uses_repo_flag_not_relations() {
        // Tracked-view rows surface via `repos.is_tracked = 1`; the
        // needs_attention count comes from the LEFT JOIN to relations on the
        // active account.
        let conn = fresh_db();
        conn.execute_batch(
            r#"
            INSERT INTO accounts (id, label, host, login, created_at) VALUES
                (1, 'a', 'github.com', 'alice', 0);

            INSERT INTO repos (id, account_id, owner, name, visibility, is_tracked) VALUES
                (10, 1, 'alice', 'web', 'public', 1);

            INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref, ci_state) VALUES
                (900, 10, 1, 't', 'open', 1, 'x', 0, strftime('%s','now'), 'main', 'a', 'FAILURE');

            -- needs_attention precomputed on the relation row for alice.
            INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at, needs_attention) VALUES
                (1, 900, 0, 0, 0, 0, 1);
            "#,
        )
        .unwrap();

        let counts = list_filter_chip_counts(&conn, DashboardView::Tracked, Some(1)).unwrap();
        assert_eq!(counts.drafts, 1);
        assert_eq!(counts.ci_failing, 1);
        assert_eq!(counts.needs_attention, 1);
    }

    // ===== ADR 0016 union-mode chip counts (issue #171) =====
    //
    // Single-account chip counts (Some(id)) stay byte-identical to before
    // ADR 0016. Unified-mode counts (None) fan out across every tracked
    // account and dedupe by PR id so a PR matched by two accounts contributes
    // one to each chip it triggers, mirroring the dashboard query's union-path
    // `GROUP BY pr.id` row shape.

    /// Shared fixture: PR 100 sits in alice's repo. Alice authored it; bob
    /// is review-requested on the same PR. Both relation rows carry
    /// `needs_attention = 1` so the `NeedsAttention` chip predicate matches
    /// through both accounts - the dedupe test pins that the count says 1.
    fn seed_two_account_shared_pr_attention_fixture(conn: &Connection) {
        conn.execute_batch(
            r#"
            INSERT INTO accounts (id, label, host, login, created_at) VALUES
                (1, 'alice-acct', 'github.com', 'alice', 0),
                (2, 'bob-acct',   'github.com', 'bob',   0);

            INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
                (10, 1, 'alice', 'web', 'public');

            INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref) VALUES
                (100, 10, 1, 'shared', 'open', 0, 'someone-else',
                 0, strftime('%s','now'), 'main', 'feat-a');

            INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at, needs_attention) VALUES
                (1, 100, 1, 0, 0, 0, 1),
                (2, 100, 0, 1, 0, 0, 1);
            "#,
        )
        .unwrap();
    }

    #[test]
    fn list_filter_chip_counts_union_dedupes_needs_attention_across_two_accounts() {
        // PR 100 carries `needs_attention = 1` on both relation rows. The
        // unified count must say 1, not 2: the dashboard query's union path
        // GROUPs by `pr.id` so the chip count has to agree row-for-row.
        let conn = fresh_db();
        seed_two_account_shared_pr_attention_fixture(&conn);

        // Authored union path - admits PR 100 via alice's authored relation.
        let counts = list_filter_chip_counts(&conn, DashboardView::Authored, None).unwrap();
        assert_eq!(
            counts.needs_attention, 1,
            "PR matched via two accounts must contribute 1 to the chip count"
        );

        // Assigned union path - admits the same PR via bob's review-requested
        // relation. Same dedupe applies.
        let counts = list_filter_chip_counts(&conn, DashboardView::Assigned, None).unwrap();
        assert_eq!(counts.needs_attention, 1);
    }

    #[test]
    fn list_filter_chip_counts_single_account_path_unchanged_under_two_account_fixture() {
        // Regression guard: with the new Option<i64> signature, Some(id) must
        // count PRs in the active account's view exactly as before #171.
        // Under the two-account shared fixture, Authored from alice's POV
        // surfaces PR 100 once (her relation is `is_authored = 1`); Assigned
        // from alice's POV surfaces zero (her relation is not
        // review-requested). Both still carry `needs_attention = 1` on
        // alice's relation row so the chip matches in the Authored view.
        let conn = fresh_db();
        seed_two_account_shared_pr_attention_fixture(&conn);

        let alice_authored =
            list_filter_chip_counts(&conn, DashboardView::Authored, Some(1)).unwrap();
        assert_eq!(alice_authored.needs_attention, 1);

        let alice_assigned =
            list_filter_chip_counts(&conn, DashboardView::Assigned, Some(1)).unwrap();
        assert_eq!(
            alice_assigned.needs_attention, 0,
            "alice has no review-requested relation on PR 100"
        );

        let bob_authored =
            list_filter_chip_counts(&conn, DashboardView::Authored, Some(2)).unwrap();
        assert_eq!(
            bob_authored.needs_attention, 0,
            "bob has no authored relation on PR 100"
        );

        let bob_assigned =
            list_filter_chip_counts(&conn, DashboardView::Assigned, Some(2)).unwrap();
        assert_eq!(
            bob_assigned.needs_attention, 1,
            "bob's review-requested relation carries needs_attention = 1"
        );
    }

    #[test]
    fn list_filter_chip_counts_union_zeros_when_no_accounts_in_scope() {
        // Empty-in-scope guard: no accounts means no PRs admitted by any
        // view, so every chip count is zero. The PR table can be non-empty
        // (orphaned data); the WHERE clauses filter it out via the relation
        // (Authored / Assigned / Watching) or repo-owner (Tracked) joins.
        let conn = fresh_db();
        // Account-less PR + relation rows that point to a missing account.
        // The repos table requires an account_id FK; insert a placeholder so
        // we can attach PRs that nothing else references.
        conn.execute_batch(
            r#"
            -- Empty-in-scope: no rows in `accounts`. We still have a repo /
            -- PR in the table from a previous sync, but no relations and no
            -- repo owner means every view filter excludes it.
            "#,
        )
        .unwrap();

        for view in [
            DashboardView::Authored,
            DashboardView::Assigned,
            DashboardView::Watching,
            DashboardView::Tracked,
        ] {
            let counts = list_filter_chip_counts(&conn, view, None).unwrap();
            assert_eq!(counts.needs_attention, 0, "{view:?} empty-scope");
            assert_eq!(counts.unresolved_threads, 0, "{view:?} empty-scope");
            assert_eq!(counts.ci_failing, 0, "{view:?} empty-scope");
            assert_eq!(counts.stale, 0, "{view:?} empty-scope");
            assert_eq!(counts.drafts, 0, "{view:?} empty-scope");
        }
    }

    #[test]
    fn list_filter_chip_counts_union_drafts_dedupes_pr_with_relations_under_two_accounts() {
        // A draft PR matched by two relation rows must still count once.
        // The Drafts predicate references `pr.draft` (not `rel.*`) so the
        // LEFT JOIN multiplies rows; the COUNT DISTINCT collapses them.
        let conn = fresh_db();
        seed_two_account_shared_pr_attention_fixture(&conn);
        conn.execute("UPDATE pull_requests SET is_draft = 1 WHERE id = 100", [])
            .unwrap();

        let counts = list_filter_chip_counts(&conn, DashboardView::Authored, None).unwrap();
        assert_eq!(
            counts.drafts, 1,
            "draft PR matched via two accounts contributes 1"
        );
    }

    #[test]
    fn list_filter_chip_counts_union_tracked_view_surfaces_tracked_repo_pr_without_relations() {
        // Tracked view in unified mode shows PRs from any tracked repo on
        // any tracked account, even when the active user has no relation row
        // for the PR. Mirrors the dashboard query's Tracked union path.
        let conn = fresh_db();
        conn.execute_batch(
            r#"
            INSERT INTO accounts (id, label, host, login, created_at) VALUES
                (1, 'alice-acct', 'github.com', 'alice', 0),
                (2, 'bob-acct',   'github.com', 'bob',   0);

            INSERT INTO repos (id, account_id, owner, name, visibility, is_tracked) VALUES
                (10, 1, 'alice', 'web', 'public', 1),
                (20, 2, 'bob',   'cli', 'public', 1);

            INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref) VALUES
                (100, 10, 1, 'alice-tracked', 'open', 1, 'someone-else',
                 0, strftime('%s','now'), 'main', 'feat-a'),
                (200, 20, 1, 'bob-tracked',   'open', 1, 'someone-else',
                 0, strftime('%s','now'), 'main', 'feat-b');
            "#,
        )
        .unwrap();

        let counts = list_filter_chip_counts(&conn, DashboardView::Tracked, None).unwrap();
        assert_eq!(counts.drafts, 2, "both tracked PRs are drafts");
    }

    // ===== sidebar attention tests (M4-C) =====

    #[test]
    fn count_sidebar_attention_zero_when_no_rows_flagged() {
        let conn = fresh_db();
        seed_sidebar_fixture(&conn);
        let counts = count_sidebar_attention(&conn, 1).unwrap();
        assert_eq!(counts, SidebarAttentionCounts::default());
    }

    #[test]
    fn count_sidebar_attention_groups_by_view_flag() {
        let conn = fresh_db();
        seed_sidebar_fixture(&conn);
        // Flip every relation's needs_attention so each bucket sees one row.
        conn.execute(
            "UPDATE pull_request_viewer_relations SET needs_attention = 1
              WHERE account_id = 1",
            [],
        )
        .unwrap();
        let counts = count_sidebar_attention(&conn, 1).unwrap();
        // PR 100 fires Authored. PR 200 fires Assigned. PR 300 fires Watching.
        // PR 400 has no view flag (none of authored/assigned/involved) but
        // does sit in a tracked repo, so Tracked alone fires for it.
        // PRs 100/200/300 sit in repo 10 (not tracked) so Tracked counts 1.
        assert_eq!(counts.authored, 1);
        assert_eq!(counts.assigned, 1);
        assert_eq!(counts.watching, 1);
        assert_eq!(counts.tracked, 1);
    }

    /// A PR that fires both Authored and Watching contributes to both
    /// buckets so the chip never under-counts an active signal.
    #[test]
    fn count_sidebar_attention_overlapping_flags_double_count() {
        let conn = fresh_db();
        seed_sidebar_fixture(&conn);
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET is_involved = 1, needs_attention = 1
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();
        let counts = count_sidebar_attention(&conn, 1).unwrap();
        assert_eq!(counts.authored, 1);
        assert_eq!(counts.watching, 1);
        assert_eq!(counts.assigned, 0);
    }

    /// Only the active account's flagged rows are counted; another account's
    /// attention rows must not leak across.
    #[test]
    fn count_sidebar_attention_scopes_per_account() {
        let conn = fresh_db();
        seed_sidebar_fixture(&conn);
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (2, 'b', 'github.com', 'bob', 0);
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at, needs_attention)
                VALUES (2, 100, 1, 0, 0, 0, 1),
                       (2, 200, 1, 0, 0, 0, 1);",
        )
        .unwrap();
        let counts = count_sidebar_attention(&conn, 1).unwrap();
        // Alice's own relations all carry needs_attention = 0 by default.
        assert_eq!(counts, SidebarAttentionCounts::default());
        let bob_counts = count_sidebar_attention(&conn, 2).unwrap();
        assert_eq!(bob_counts.authored, 2);
    }

    #[test]
    fn count_sidebar_attention_tracked_requires_tracked_repo() {
        let conn = fresh_db();
        seed_sidebar_fixture(&conn);
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET needs_attention = 1
              WHERE account_id = 1 AND pull_request_id IN (100, 200, 300)",
            [],
        )
        .unwrap();
        // PRs 100/200/300 sit in repo 10 (not tracked). The Tracked bucket
        // must remain at zero because none of the attention-flagged PRs live
        // in a tracked repo.
        let counts = count_sidebar_attention(&conn, 1).unwrap();
        assert_eq!(counts.tracked, 0);
    }

    /// The Tracked bucket must mirror the dashboard Tracked view's
    /// `r.account_id = ?` predicate so a relation row owned by account 1
    /// on a tracked repo owned by account 2 doesn't over-count.
    #[test]
    fn count_sidebar_attention_tracked_requires_repo_owner_matches_active_account() {
        let conn = fresh_db();
        seed_sidebar_fixture(&conn);
        // Seed account 2 + a tracked repo it owns, plus a PR in that repo.
        // Then attach a relation row from account 1's perspective with
        // needs_attention = 1. The dashboard Tracked view scoped to account
        // 1 would NOT show this PR (repo owner is 2), so the count must
        // agree.
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (2, 'b', 'github.com', 'bob', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility, is_tracked)
                VALUES (30, 2, 'bob', 'cli', 'public', 1);
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (500, 30, 1, 'e', 'open', 0, 'bob', 0, 1, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at, needs_attention)
                VALUES (1, 500, 0, 0, 1, 0, 1);",
        )
        .unwrap();
        let counts = count_sidebar_attention(&conn, 1).unwrap();
        // Watching catches PR 500 (alice is involved). Tracked must not,
        // because repo 30 is owned by account 2.
        assert_eq!(counts.watching, 1);
        assert_eq!(counts.tracked, 0);
    }

    // ===== archive (M6) =====

    /// Seed an account, a repo, and a PR in the supplied `state`. The PR's
    /// `updated_at` is set to `now - days_inactive * 86400` so the sweep
    /// predicate (`updated_at < now - 30 days`) can be exercised across the
    /// 29 / 31 day boundary with a single helper. No relation row is created -
    /// each test attaches one (or none) according to the scenario.
    fn seed_pr_for_archive(conn: &Connection, pr_id: i64, state: &str, days_inactive: i64) {
        conn.execute_batch(&format!(
            "INSERT OR IGNORE INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT OR IGNORE INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES ({pr_id}, 10, {pr_id}, 't', '{state}', 0, 'bob',
                        0, strftime('%s','now','-{days_inactive} days'),
                        'main', 'feat');"
        ))
        .unwrap();
    }

    fn read_archived_at(conn: &Connection, account_id: i64, pr_id: i64) -> Option<i64> {
        conn.query_row(
            "SELECT archived_at FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    }

    #[test]
    fn mark_archived_upserts_when_relation_missing() {
        let conn = fresh_db();
        seed_pr_for_archive(&conn, 100, "open", 1);
        // No relation row for (1, 100) yet.
        mark_archived(&conn, 1, 100).unwrap();
        let archived_at = read_archived_at(&conn, 1, 100);
        assert!(archived_at.is_some(), "archived_at set on the new row");
        // Schema defaults on the freshly-inserted row.
        let (
            is_authored,
            is_review_requested,
            is_involved,
            mentioned_count_unread,
            needs_attention,
        ): (i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT is_authored, is_review_requested, is_involved,
                    mentioned_count_unread, needs_attention
               FROM pull_request_viewer_relations
              WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(is_authored, 0);
        assert_eq!(is_review_requested, 0);
        assert_eq!(is_involved, 0);
        assert_eq!(mentioned_count_unread, 0);
        assert_eq!(needs_attention, 0);
    }

    #[test]
    fn mark_archived_preserves_other_columns_on_existing_row() {
        let conn = fresh_db();
        seed_pr_for_archive(&conn, 100, "open", 1);
        // Seed an existing relation row with non-default triage state. The
        // archive UPSERT must touch `archived_at` only.
        conn.execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at, read_at, mentioned_count_unread,
                 needs_attention)
                VALUES (1, 100, 1, 0, 0, 12345, 99999, 3, 1)",
            [],
        )
        .unwrap();

        mark_archived(&conn, 1, 100).unwrap();

        let (is_authored, read_at, mentioned_count_unread, needs_attention, archived_at): (
            i64,
            Option<i64>,
            i64,
            i64,
            Option<i64>,
        ) = conn
            .query_row(
                "SELECT is_authored, read_at, mentioned_count_unread,
                    needs_attention, archived_at
               FROM pull_request_viewer_relations
              WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(is_authored, 1, "is_authored preserved");
        assert_eq!(read_at, Some(99999), "read_at preserved");
        assert_eq!(mentioned_count_unread, 3, "mention counter preserved");
        assert_eq!(needs_attention, 1, "needs_attention preserved");
        assert!(archived_at.is_some(), "archived_at set");
    }

    #[test]
    fn mark_unarchived_clears_archived_at() {
        let conn = fresh_db();
        seed_pr_for_archive(&conn, 100, "open", 1);
        mark_archived(&conn, 1, 100).unwrap();
        assert!(read_archived_at(&conn, 1, 100).is_some());

        mark_unarchived(&conn, 1, 100).unwrap();
        assert_eq!(
            read_archived_at(&conn, 1, 100),
            None,
            "archived_at cleared to NULL"
        );
    }

    #[test]
    fn mark_unarchived_upserts_when_relation_missing() {
        let conn = fresh_db();
        seed_pr_for_archive(&conn, 100, "open", 1);
        // No relation row; unarchive must still UPSERT (mirrors mark_archived)
        // so the same merged-row write path works whether the row exists yet.
        mark_unarchived(&conn, 1, 100).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(read_archived_at(&conn, 1, 100), None);
    }

    #[test]
    fn auto_archive_sweep_archives_closed_pr_immediately() {
        // Post-M6: the 30-day inactivity gate was dropped. Closed PRs are
        // archived on the next sweep regardless of `updated_at`.
        let conn = fresh_db();
        seed_pr_for_archive(&conn, 100, "closed", 1);
        conn.execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at)
                VALUES (1, 100, 0)",
            [],
        )
        .unwrap();

        let archived = auto_archive_sweep(&conn).unwrap();
        assert_eq!(archived, 1);
        assert!(read_archived_at(&conn, 1, 100).is_some());
    }

    #[test]
    fn auto_archive_sweep_archives_merged_pr_immediately() {
        let conn = fresh_db();
        seed_pr_for_archive(&conn, 100, "merged", 1);
        conn.execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at)
                VALUES (1, 100, 0)",
            [],
        )
        .unwrap();

        let archived = auto_archive_sweep(&conn).unwrap();
        assert_eq!(archived, 1);
        assert!(read_archived_at(&conn, 1, 100).is_some());
    }

    #[test]
    fn auto_archive_sweep_skips_open_pr_regardless_of_inactivity() {
        let conn = fresh_db();
        // 60 days inactive but still open - the sweep must not touch it.
        seed_pr_for_archive(&conn, 100, "open", 60);
        conn.execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at)
                VALUES (1, 100, 0)",
            [],
        )
        .unwrap();

        let archived = auto_archive_sweep(&conn).unwrap();
        assert_eq!(archived, 0);
        assert_eq!(read_archived_at(&conn, 1, 100), None);
    }

    #[test]
    fn auto_archive_sweep_is_idempotent() {
        let conn = fresh_db();
        seed_pr_for_archive(&conn, 100, "closed", 31);
        conn.execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at)
                VALUES (1, 100, 0)",
            [],
        )
        .unwrap();

        let first = auto_archive_sweep(&conn).unwrap();
        let first_archived_at = read_archived_at(&conn, 1, 100).expect("archived after first run");
        let second = auto_archive_sweep(&conn).unwrap();
        let second_archived_at = read_archived_at(&conn, 1, 100).expect("still archived");

        assert_eq!(first, 1, "first sweep archives the matching row");
        assert_eq!(
            second, 0,
            "second sweep is a no-op - predicate skips archived rows"
        );
        assert_eq!(
            first_archived_at, second_archived_at,
            "second sweep does not touch the existing archive timestamp"
        );
    }

    #[test]
    fn auto_archive_sweep_fans_across_all_accounts_for_one_pr() {
        // Two accounts both with relations to a closed-and-old PR. The sweep
        // is account-agnostic (single UPDATE across the table) so both rows
        // archive in one call.
        let conn = fresh_db();
        seed_pr_for_archive(&conn, 100, "closed", 45);
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (2, 'b', 'github.com', 'bob', 0);
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at)
                VALUES (1, 100, 0), (2, 100, 0);",
        )
        .unwrap();

        let archived = auto_archive_sweep(&conn).unwrap();
        assert_eq!(archived, 2);
        assert!(read_archived_at(&conn, 1, 100).is_some());
        assert!(read_archived_at(&conn, 2, 100).is_some());
    }

    // ===== ADR 0018 archive exclusion tests (issue #194) =====

    /// Sidebar attention counts exclude archived rows. A relation with
    /// `needs_attention = 1` AND `archived_at IS NOT NULL` does not boost the
    /// count chip - archived rows live in the Archive view, not the active
    /// queue.
    #[test]
    fn count_sidebar_attention_excludes_archived_rows() {
        let conn = fresh_db();
        seed_sidebar_fixture(&conn);
        // Flag every relation and archive PR 100 (alice's authored). The
        // sidebar count for Authored should drop to zero; Assigned and
        // Watching keep their non-archived rows.
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET needs_attention = 1
              WHERE account_id = 1",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET archived_at = strftime('%s','now')
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();

        let counts = count_sidebar_attention(&conn, 1).unwrap();
        assert_eq!(
            counts.authored, 0,
            "PR 100 carried the only Authored attention row; archive drops it"
        );
        assert_eq!(counts.assigned, 1, "Assigned bucket (PR 200) unaffected");
        assert_eq!(counts.watching, 1, "Watching bucket (PR 300) unaffected");
    }

    /// Chip counts exclude archived rows: a PR matching a chip predicate but
    /// archived on the active account does not contribute to the count.
    #[test]
    fn count_chip_excludes_archived_rows_single_account_scope() {
        let conn = fresh_db();
        seed_chip_count_fixture(&conn);
        // The fixture has PR 604 as the sole needs_attention row. Archive
        // alice's relation on it - the chip count should drop to zero.
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET archived_at = strftime('%s','now')
              WHERE account_id = 1 AND pull_request_id = 604",
            [],
        )
        .unwrap();

        let counts = list_filter_chip_counts(&conn, DashboardView::Watching, Some(1)).unwrap();
        assert_eq!(
            counts.needs_attention, 0,
            "archived row must not contribute to the chip count"
        );
        // Sanity: other chips reading non-relation columns are also gated by
        // the relation row - archiving alice's relation removes PR 604 from
        // the Watching FROM clause entirely.
        assert!(
            counts.drafts == 1 && counts.ci_failing == 1 && counts.stale == 1,
            "unrelated chip predicates still count their respective PRs"
        );
    }

    /// Chip counts under unified scope: same predicate applies to both the
    /// EXISTS view-filter and the LEFT JOIN. A PR with every relation
    /// archived drops out of the count entirely; a PR with a mix keeps
    /// counting through the unarchived relation.
    #[test]
    fn count_chip_excludes_archived_rows_unified_scope() {
        let conn = fresh_db();
        seed_two_account_shared_pr_attention_fixture(&conn);
        // Both relations carry `needs_attention = 1`. Archive both - the
        // PR should drop from every union view's chip count.
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET archived_at = strftime('%s','now')
              WHERE pull_request_id = 100",
            [],
        )
        .unwrap();

        for view in [DashboardView::Authored, DashboardView::Assigned] {
            let counts = list_filter_chip_counts(&conn, view, None).unwrap();
            assert_eq!(
                counts.needs_attention, 0,
                "{view:?} unified count excludes a fully-archived PR"
            );
        }
    }

    /// Chip counts under unified scope: a partial archive (one account
    /// archived, one not) keeps the PR in the count via the unarchived
    /// relation. Mirrors the dashboard query's union-mode shape.
    #[test]
    fn count_chip_keeps_partial_archive_in_unified_scope() {
        let conn = fresh_db();
        seed_two_account_shared_pr_attention_fixture(&conn);
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET archived_at = strftime('%s','now')
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();

        // Account 2's review-requested relation is unarchived and carries
        // needs_attention = 1; Assigned union should count the PR.
        let counts = list_filter_chip_counts(&conn, DashboardView::Assigned, None).unwrap();
        assert_eq!(
            counts.needs_attention, 1,
            "unarchived relation keeps the PR in the unified chip count"
        );
    }

    /// `list_filter_chip_counts` short-circuits to zeros for the Archive
    /// view. The W2 frontend hides the chip rail on archive, but the command
    /// shape stays uniform.
    #[test]
    fn list_filter_chip_counts_returns_zeros_for_archive_view() {
        let conn = fresh_db();
        seed_chip_count_fixture(&conn);
        let counts = list_filter_chip_counts(&conn, DashboardView::Archive, Some(1)).unwrap();
        assert_eq!(counts.needs_attention, 0);
        assert_eq!(counts.unresolved_threads, 0);
        assert_eq!(counts.ci_failing, 0);
        assert_eq!(counts.stale, 0);
        assert_eq!(counts.drafts, 0);
    }

    // ===== archive_retention_sweep (60-day hard delete) =====

    /// Seed an account + repo + PR + a single viewer relation with the
    /// requested `archived_at` offset (days into the past, or `None` for
    /// unarchived). Returns once the row is committed so the sweep can read
    /// against the freshly-seeded state.
    fn seed_pr_with_archive_age(conn: &Connection, pr_id: i64, archived_days_ago: Option<i64>) {
        conn.execute_batch(
            "INSERT OR IGNORE INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT OR IGNORE INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');",
        )
        .unwrap();
        let archived_sql = match archived_days_ago {
            Some(days) => format!("strftime('%s','now','-{days} days')"),
            None => "NULL".to_string(),
        };
        conn.execute_batch(&format!(
            "INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES ({pr_id}, 10, {pr_id}, 't', 'merged', 0, 'bob',
                        0, 0, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at, archived_at)
                VALUES (1, {pr_id}, 0, {archived_sql});"
        ))
        .unwrap();
    }

    fn pr_exists(conn: &Connection, pr_id: i64) -> bool {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pull_requests WHERE id = ?1",
                params![pr_id],
                |row| row.get(0),
            )
            .unwrap();
        count == 1
    }

    #[test]
    fn archive_retention_sweep_returns_zero_on_empty_db() {
        let conn = fresh_db();
        assert_eq!(archive_retention_sweep(&conn).unwrap(), 0);
    }

    #[test]
    fn archive_retention_sweep_deletes_pr_archived_past_60_days() {
        let conn = fresh_db();
        seed_pr_with_archive_age(&conn, 100, Some(61));
        let deleted = archive_retention_sweep(&conn).unwrap();
        assert_eq!(deleted, 1);
        assert!(!pr_exists(&conn, 100), "PR row removed");
        let rel_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pull_request_viewer_relations WHERE pull_request_id = 100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rel_count, 0, "FK cascade dropped the relation row");
    }

    #[test]
    fn archive_retention_sweep_skips_pr_archived_under_60_days() {
        let conn = fresh_db();
        seed_pr_with_archive_age(&conn, 100, Some(59));
        let deleted = archive_retention_sweep(&conn).unwrap();
        assert_eq!(deleted, 0);
        assert!(pr_exists(&conn, 100), "PR row retained");
    }

    #[test]
    fn archive_retention_sweep_skips_pr_with_unarchived_relation() {
        // A PR archived from one account but still active for another stays
        // alive - the cleanup waits until every relation has aged past the
        // 60-day mark.
        let conn = fresh_db();
        seed_pr_with_archive_age(&conn, 100, Some(90));
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (2, 'b', 'github.com', 'bob', 0);
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at, archived_at)
                VALUES (2, 100, 0, NULL);",
        )
        .unwrap();
        let deleted = archive_retention_sweep(&conn).unwrap();
        assert_eq!(deleted, 0);
        assert!(pr_exists(&conn, 100));
    }

    #[test]
    fn archive_retention_sweep_skips_pr_with_recently_archived_relation() {
        // Two relations, one archived 90 days ago, one archived 30 days ago.
        // The 30-day relation is too fresh; the sweep waits for it.
        let conn = fresh_db();
        seed_pr_with_archive_age(&conn, 100, Some(90));
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (2, 'b', 'github.com', 'bob', 0);
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at, archived_at)
                VALUES (2, 100, 0, strftime('%s','now','-30 days'));",
        )
        .unwrap();
        let deleted = archive_retention_sweep(&conn).unwrap();
        assert_eq!(deleted, 0);
        assert!(pr_exists(&conn, 100));
    }

    #[test]
    fn archive_retention_sweep_skips_pr_with_no_relations() {
        // Tracked-view-only PRs (no viewer relations) are out of scope. The
        // user never engaged with them; archive retention doesn't apply.
        let conn = fresh_db();
        conn.execute_batch(
            "INSERT OR IGNORE INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT OR IGNORE INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'merged', 0, 'bob',
                        0, 0, 'main', 'feat');",
        )
        .unwrap();
        let deleted = archive_retention_sweep(&conn).unwrap();
        assert_eq!(deleted, 0);
        assert!(pr_exists(&conn, 100));
    }

    // ===== mark_view_read (issue #336) =====

    /// Seed N Watching-view PRs for account 1, each unread (no `read_at`).
    /// Returns the PR ids inserted so the test can probe individual rows.
    fn seed_n_watching_prs(conn: &Connection, count: i64) -> Vec<i64> {
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'alice', 'web', 'public');",
        )
        .unwrap();
        let mut ids = Vec::new();
        for i in 0..count {
            let pr_id = 1000 + i;
            conn.execute_batch(&format!(
                "INSERT INTO pull_requests
                    (id, repo_id, number, title, state, is_draft, author_login,
                     created_at, updated_at, base_ref, head_ref)
                    VALUES ({pr_id}, 10, {pr_id}, 't', 'open', 0, 'bob',
                            0, 1000000, 'main', 'feat');
                 INSERT INTO pull_request_viewer_relations
                    (account_id, pull_request_id, is_authored, is_review_requested,
                     is_involved, relation_observed_at)
                    VALUES (1, {pr_id}, 0, 0, 1, 0);"
            ))
            .unwrap();
            ids.push(pr_id);
        }
        ids
    }

    fn count_unread_relations(conn: &Connection, account_id: i64) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND read_at IS NULL",
            params![account_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn mark_view_read_flips_every_pr_in_the_view() {
        let conn = fresh_db();
        let ids = seed_n_watching_prs(&conn, 4);
        assert_eq!(count_unread_relations(&conn, 1), 4, "fixture is unread");

        let marked = mark_view_read(&conn, DashboardView::Watching, Some(1), &[]).unwrap();
        assert_eq!(marked, 4, "every PR in the view contributes one");
        assert_eq!(
            count_unread_relations(&conn, 1),
            0,
            "no unread relations after the bulk flip"
        );

        // Per-row read fields match the per-row `mark_read` shape.
        for pr_id in ids {
            let (read_at, read_pr_updated_at, mentioned, watermark): (
                Option<i64>,
                Option<i64>,
                i64,
                Option<i64>,
            ) = conn
                .query_row(
                    "SELECT read_at, read_pr_updated_at,
                            mentioned_count_unread, mention_scan_watermark_at
                       FROM pull_request_viewer_relations
                      WHERE account_id = 1 AND pull_request_id = ?1",
                    params![pr_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
            assert!(read_at.is_some(), "read_at set for pr {pr_id}");
            assert_eq!(
                read_pr_updated_at,
                Some(1_000_000),
                "read_pr_updated_at snapshots pr.updated_at for pr {pr_id}"
            );
            assert_eq!(mentioned, 0, "mention counter cleared for pr {pr_id}");
            assert!(
                watermark.is_some(),
                "mention scan watermark advanced for pr {pr_id}"
            );
        }
    }

    #[test]
    fn mark_view_read_returns_zero_on_empty_view() {
        let conn = fresh_db();
        // Account exists but no PRs.
        conn.execute(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0)",
            [],
        )
        .unwrap();
        let marked = mark_view_read(&conn, DashboardView::Authored, Some(1), &[]).unwrap();
        assert_eq!(marked, 0);
    }

    #[test]
    fn mark_view_read_clears_attention_when_only_signal_was_mention() {
        let conn = fresh_db();
        let ids = seed_n_watching_prs(&conn, 1);
        let pr_id = ids[0];
        // Seed a mention counter + needs_attention precomputed off the counter.
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET mentioned_count_unread = 3, needs_attention = 1
              WHERE account_id = 1 AND pull_request_id = ?1",
            params![pr_id],
        )
        .unwrap();

        mark_view_read(&conn, DashboardView::Watching, Some(1), &[]).unwrap();

        let (mentioned, needs_attention): (i64, i64) = conn
            .query_row(
                "SELECT mentioned_count_unread, needs_attention
                   FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = ?1",
                params![pr_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(mentioned, 0);
        assert_eq!(
            needs_attention, 0,
            "read flip drops the only attention signal"
        );
    }

    #[test]
    fn mark_view_read_keeps_attention_when_other_signals_still_fire() {
        let conn = fresh_db();
        // Author == viewer + unresolved involved thread keeps signal 1 firing
        // even after the read flip (mentions go to zero, but threads stay).
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'alice', 'web', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 0, 'alice', 0, 1_000_000, 'main', 'feat');
             INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, node_id)
                VALUES (5001, 100, 0, 0, 'RT_seed');
             INSERT INTO review_comments
                (id, review_thread_id, author_login, body, created_at)
                VALUES (6001, 5001, 'alice', 'note', 1);
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at, mentioned_count_unread,
                 needs_attention)
                VALUES (1, 100, 1, 0, 0, 0, 4, 1);",
        )
        .unwrap();

        let marked = mark_view_read(&conn, DashboardView::Authored, Some(1), &[]).unwrap();
        assert_eq!(marked, 1);

        let needs_attention: i64 = conn
            .query_row(
                "SELECT needs_attention FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            needs_attention, 1,
            "thread-driven signal still fires after the mention flip"
        );
    }

    #[test]
    fn mark_view_read_scopes_to_active_view_only() {
        // Two PRs: one Authored, one Watching. mark_view_read on the Authored
        // view must only touch the authored relation; the watching one stays
        // unread.
        let conn = fresh_db();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'alice', 'web', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref) VALUES
                (100, 10, 1, 'authored', 'open', 0, 'alice', 0, 1, 'main', 'feat'),
                (200, 10, 2, 'watching', 'open', 0, 'bob',   0, 1, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at) VALUES
                (1, 100, 1, 0, 0, 0),
                (1, 200, 0, 0, 1, 0);",
        )
        .unwrap();

        let marked = mark_view_read(&conn, DashboardView::Authored, Some(1), &[]).unwrap();
        assert_eq!(marked, 1, "only the authored PR matched");

        let authored_read_at: Option<i64> = conn
            .query_row(
                "SELECT read_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let watching_read_at: Option<i64> = conn
            .query_row(
                "SELECT read_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 200",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(authored_read_at.is_some(), "authored PR is now read");
        assert!(
            watching_read_at.is_none(),
            "watching PR untouched - the view did not admit it"
        );
    }

    #[test]
    fn mark_view_read_respects_chip_filter() {
        // Seed two Watching PRs: PR 100 is a draft, PR 200 is not. With the
        // Drafts chip active, only PR 100 should be marked.
        let conn = fresh_db();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'alice', 'web', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref) VALUES
                (100, 10, 1, 'draft',    'open', 1, 'bob', 0, 1, 'main', 'feat'),
                (200, 10, 2, 'nondraft', 'open', 0, 'bob', 0, 1, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at) VALUES
                (1, 100, 0, 0, 1, 0),
                (1, 200, 0, 0, 1, 0);",
        )
        .unwrap();

        let marked =
            mark_view_read(&conn, DashboardView::Watching, Some(1), &[ChipKey::Drafts]).unwrap();
        assert_eq!(marked, 1, "only the draft PR is admitted by the chip");

        let draft_read_at: Option<i64> = conn
            .query_row(
                "SELECT read_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let nondraft_read_at: Option<i64> = conn
            .query_row(
                "SELECT read_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 200",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(draft_read_at.is_some(), "draft PR marked read");
        assert!(
            nondraft_read_at.is_none(),
            "non-draft PR stays unread - the chip filtered it out"
        );
    }

    #[test]
    fn mark_view_read_excludes_archived_rows() {
        // Default view (Watching) hides archived rows. mark_view_read must not
        // touch them either.
        let conn = fresh_db();
        let ids = seed_n_watching_prs(&conn, 2);
        // Archive the second PR's relation.
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET archived_at = strftime('%s','now')
              WHERE account_id = 1 AND pull_request_id = ?1",
            params![ids[1]],
        )
        .unwrap();

        let marked = mark_view_read(&conn, DashboardView::Watching, Some(1), &[]).unwrap();
        assert_eq!(marked, 1, "archived row excluded from the active view");

        let unarchived_read_at: Option<i64> = conn
            .query_row(
                "SELECT read_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = ?1",
                params![ids[0]],
                |row| row.get(0),
            )
            .unwrap();
        let archived_read_at: Option<i64> = conn
            .query_row(
                "SELECT read_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = ?1",
                params![ids[1]],
                |row| row.get(0),
            )
            .unwrap();
        assert!(unarchived_read_at.is_some());
        assert!(
            archived_read_at.is_none(),
            "archived row stays unread - the active view never showed it"
        );
    }

    #[test]
    fn mark_view_read_works_on_archive_view() {
        // Archive view: an archived PR should still be markable as read. The
        // user can hit the archive surface and clear unread dots on archived
        // rows.
        let conn = fresh_db();
        let ids = seed_n_watching_prs(&conn, 2);
        // Archive both PRs.
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET archived_at = strftime('%s','now')
              WHERE account_id = 1",
            [],
        )
        .unwrap();

        let marked = mark_view_read(&conn, DashboardView::Archive, Some(1), &[]).unwrap();
        assert_eq!(marked, 2, "both archived PRs flip to read");

        for pr_id in ids {
            let read_at: Option<i64> = conn
                .query_row(
                    "SELECT read_at FROM pull_request_viewer_relations
                      WHERE account_id = 1 AND pull_request_id = ?1",
                    params![pr_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(read_at.is_some(), "archived PR {pr_id} is now read");
        }
    }

    #[test]
    fn mark_view_read_unified_mode_fans_across_relation_owners() {
        // Two accounts share a PR via Authored (account 1) and Watching
        // (account 2) relations. Authored union mode admits the PR; both
        // relations should flip to read.
        let conn = fresh_db();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at) VALUES
                (1, 'a', 'github.com', 'alice', 0),
                (2, 'b', 'github.com', 'bob',   0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'alice', 'web', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 'shared', 'open', 0, 'someone-else',
                        0, 1_000_000, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at) VALUES
                (1, 100, 1, 0, 0, 0),
                (2, 100, 0, 0, 1, 0);",
        )
        .unwrap();

        let marked = mark_view_read(&conn, DashboardView::Authored, None, &[]).unwrap();
        assert_eq!(
            marked, 1,
            "one distinct PR (matched via alice's authored relation)"
        );

        let alice_read_at: Option<i64> = conn
            .query_row(
                "SELECT read_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let bob_read_at: Option<i64> = conn
            .query_row(
                "SELECT read_at FROM pull_request_viewer_relations
                  WHERE account_id = 2 AND pull_request_id = 100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(alice_read_at.is_some(), "alice's relation flipped");
        assert!(
            bob_read_at.is_some(),
            "unified mode fans the flip to bob's relation too"
        );
    }

    #[test]
    fn mark_view_read_does_not_upsert_tracked_view_pr_without_relation() {
        // Tracked view shows PRs from tracked repos even without a relation
        // row. mark_view_read must not UPSERT - the per-row mark_pr_read is
        // the right tool for that case.
        let conn = fresh_db();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility, is_tracked)
                VALUES (10, 1, 'alice', 'web', 'public', 1);
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 0, 'bob', 0, 1, 'main', 'feat');",
        )
        .unwrap();

        let marked = mark_view_read(&conn, DashboardView::Tracked, Some(1), &[]).unwrap();
        assert_eq!(marked, 0, "no relation rows to flip");

        let rel_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rel_count, 0, "mark_view_read must not UPSERT");
    }
}
