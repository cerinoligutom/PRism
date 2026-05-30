//! Edge-with-re-arm dispatch detection (ADR 0031), per-PR dedup.
//!
//! For a PR that currently needs the viewer (`needs_attention = 1`), find the
//! newest other-authored comment that satisfies a unit-needs-me predicate
//! across ALL of the PR's conversation units, and emit one trigger iff that
//! timestamp is strictly newer than the per-PR `last_emitted_activity_at`
//! watermark. Reading a unit advances its engagement watermark, so the PR
//! stops needing the viewer and nothing fires; a genuinely-new later reply is
//! both newer than the engagement mark (re-lights) and newer than
//! `last_emitted_activity_at` (re-fires exactly once).
//!
//! Per-PR coarseness is deliberate (ADR 0031 dispatch dedup grain): two units
//! crossing in the same cycle produce ONE trigger, tagged with the unit
//! holding the newest crossing activity.

use rusqlite::{params, OptionalExtension};

use crate::notify::{NotificationKind, NotificationTrigger, NotificationUnitKind};

/// One qualifying unit's newest crossing activity, gathered by the query
/// below. The caller picks the row with the largest `newest_activity_at` as
/// the unit to tag the (single) trigger with.
struct UnitCrossing {
    unit_kind: NotificationUnitKind,
    /// Thread `node_id` for a thread unit; `None` for the general stream.
    unit_ref: Option<String>,
    /// Deep link: the thread url, or the PR conversation url for the general
    /// stream.
    deep_link_url: Option<String>,
    newest_activity_at: i64,
}

/// Compute the per-PR re-arm trigger after the recompute has run.
///
/// Returns at most one [`NotificationTrigger`] and, when it does, the value
/// the caller must advance `last_emitted_activity_at` to (MAX-only). Returns
/// `(None, None)` when the PR doesn't need the viewer, no unit crossing beats
/// the watermark, or the relation row is missing.
///
/// `account_id` / `pr_id` scope the lookup; `viewer_login` is the
/// host-resolved viewer identity the caller already derived for the scan.
pub(super) fn rearm_dispatch(
    tx: &rusqlite::Transaction<'_>,
    account_id: i64,
    pr_id: i64,
    viewer_login: &str,
) -> Result<(Option<NotificationTrigger>, Option<i64>), rusqlite::Error> {
    // Only PRs that currently need the viewer can emit. A missing relation row
    // reads as no dispatch.
    let needs_attention: Option<i64> = tx
        .query_row(
            "SELECT needs_attention FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    if needs_attention != Some(1) {
        return Ok((None, None));
    }

    let last_emitted: i64 = tx
        .query_row(
            "SELECT COALESCE(last_emitted_activity_at, 0)
               FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);

    let crossings = gather_unit_crossings(tx, account_id, pr_id, viewer_login)?;

    // The newest crossing across all units. `gather_unit_crossings` already
    // orders by activity DESC so the head is the newest; tagging the trigger
    // with it is the documented per-PR coarseness (two units crossing in one
    // cycle => one trigger tagged with the newest unit).
    let Some(newest) = crossings.into_iter().next() else {
        return Ok((None, None));
    };

    if newest.newest_activity_at <= last_emitted {
        return Ok((None, None));
    }

    let trigger = NotificationTrigger {
        account_id,
        pull_request_id: pr_id,
        kind: NotificationKind::NeedsAttention,
        unit_kind: newest.unit_kind,
        unit_ref: newest.unit_ref,
        deep_link_url: newest.deep_link_url,
        newest_activity_at: newest.newest_activity_at,
    };
    Ok((Some(trigger), Some(newest.newest_activity_at)))
}

/// Gather one [`UnitCrossing`] per qualifying conversation unit, newest
/// crossing first. A unit qualifies when it needs the viewer under the same
/// per-unit predicate the roll-up's (A)/(B) branches use; `newest_activity_at`
/// is the unit's max other-authored comment `created_at` strictly newer than
/// the unit's engagement watermark. Host gating is handled by the caller's
/// early-exit (the scan returns before calling here when the viewer host
/// differs from the PR host), so this query keys on `viewer_login` directly.
fn gather_unit_crossings(
    tx: &rusqlite::Transaction<'_>,
    account_id: i64,
    pr_id: i64,
    viewer_login: &str,
) -> Result<Vec<UnitCrossing>, rusqlite::Error> {
    let mut out: Vec<UnitCrossing> = Vec::new();

    // Threads: per thread the viewer is involved in, the newest other-authored
    // comment past MAX(seen-watermark, my-latest-comment).
    {
        let mut stmt = tx.prepare(
            "SELECT t.node_id, t.url, MAX(c.created_at) AS newest
               FROM review_threads t
               JOIN review_comments c ON c.review_thread_id = t.id
              WHERE t.pull_request_id = ?1
                AND t.node_id IS NOT NULL
                AND c.author_login <> ?2
                AND (
                    EXISTS (
                        SELECT 1 FROM pull_requests pr
                         WHERE pr.id = t.pull_request_id
                           AND pr.author_login = ?2
                    )
                    OR EXISTS (
                        SELECT 1 FROM review_comments ic
                         WHERE ic.review_thread_id = t.id
                           AND ic.author_login = ?2
                    )
                    OR EXISTS (
                        SELECT 1 FROM review_comments ic
                         WHERE ic.review_thread_id = t.id
                           AND ic.mentions_viewer = 1
                    )
                )
                AND c.created_at > (
                    SELECT MAX(w) FROM (
                        SELECT COALESCE((
                            SELECT trs.seen_at FROM thread_read_state trs
                             WHERE trs.account_id = ?3
                               AND trs.review_thread_node_id = t.node_id
                        ), 0) AS w
                        UNION ALL
                        SELECT COALESCE((
                            SELECT MAX(mc.created_at) FROM review_comments mc
                             WHERE mc.review_thread_id = t.id
                               AND mc.author_login = ?2
                        ), 0) AS w
                    )
                )
              GROUP BY t.id",
        )?;
        let rows = stmt.query_map(params![pr_id, viewer_login, account_id], |row| {
            Ok(UnitCrossing {
                unit_kind: NotificationUnitKind::Thread,
                unit_ref: row.get::<_, Option<String>>(0)?,
                deep_link_url: row.get::<_, Option<String>>(1)?,
                newest_activity_at: row.get::<_, i64>(2)?,
            })
        })?;
        for r in rows {
            out.push(r?);
        }
    }

    // General stream: the newest other-authored issue_comment past
    // MAX(general_stream_seen_at, my-latest-issue-comment).
    {
        let general: Option<i64> = tx
            .query_row(
                "SELECT MAX(ic.created_at)
                   FROM issue_comments ic
                  WHERE ic.pull_request_id = ?1
                    AND ic.author_login <> ?2
                    AND (
                        EXISTS (
                            SELECT 1 FROM pull_requests pr
                             WHERE pr.id = ic.pull_request_id
                               AND pr.author_login = ?2
                        )
                        OR EXISTS (
                            SELECT 1 FROM issue_comments mine
                             WHERE mine.pull_request_id = ?1
                               AND mine.author_login = ?2
                        )
                        OR EXISTS (
                            SELECT 1 FROM issue_comments m
                             WHERE m.pull_request_id = ?1
                               AND m.mentions_viewer = 1
                        )
                    )
                    AND ic.created_at > (
                        SELECT MAX(w) FROM (
                            SELECT COALESCE((
                                SELECT rel.general_stream_seen_at
                                  FROM pull_request_viewer_relations rel
                                 WHERE rel.account_id = ?3
                                   AND rel.pull_request_id = ?1
                            ), 0) AS w
                            UNION ALL
                            SELECT COALESCE((
                                SELECT MAX(mic.created_at) FROM issue_comments mic
                                 WHERE mic.pull_request_id = ?1
                                   AND mic.author_login = ?2
                            ), 0) AS w
                        )
                    )",
                params![pr_id, viewer_login, account_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten();
        if let Some(newest) = general {
            out.push(UnitCrossing {
                unit_kind: NotificationUnitKind::General,
                unit_ref: None,
                deep_link_url: pr_conversation_url(tx, pr_id)?,
                newest_activity_at: newest,
            });
        }
    }

    out.sort_by_key(|c| std::cmp::Reverse(c.newest_activity_at));
    Ok(out)
}

/// The PR's conversation deep link for the general stream toast. Derived from
/// the repo slug + PR number so it points at the GitHub conversation tab.
/// Returns `None` when the PR row can't be resolved.
fn pr_conversation_url(
    tx: &rusqlite::Transaction<'_>,
    pr_id: i64,
) -> Result<Option<String>, rusqlite::Error> {
    tx.query_row(
        "SELECT r.owner, r.name, pr.number
           FROM pull_requests pr
           JOIN repos r ON r.id = pr.repo_id
          WHERE pr.id = ?1",
        params![pr_id],
        |row| {
            Ok(format!(
                "https://github.com/{}/{}/pull/{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )
    .optional()
}
