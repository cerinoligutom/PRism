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
        newest_activity_at: Some(newest.newest_activity_at),
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

    // Reviews stream (ADR 0033): the newest other-authored formal review whose
    // body @-mentions the viewer, past `reviews_seen_at`. A mention-only unit,
    // so the watermark is `reviews_seen_at` alone (no "my own comment"
    // component like the thread / general units have). Deep-links to the PR
    // conversation url, peer to the general stream.
    {
        let review: Option<i64> = tx
            .query_row(
                "SELECT MAX(rv.submitted_at)
                   FROM reviews rv
                  WHERE rv.pull_request_id = ?1
                    AND rv.reviewer_login <> ?2
                    AND rv.mentions_viewer = 1
                    AND COALESCE(rv.submitted_at, 0) > COALESCE((
                        SELECT rel.reviews_seen_at
                          FROM pull_request_viewer_relations rel
                         WHERE rel.account_id = ?3
                           AND rel.pull_request_id = ?1
                    ), 0)",
                params![pr_id, viewer_login, account_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten();
        if let Some(newest) = review {
            out.push(UnitCrossing {
                unit_kind: NotificationUnitKind::Review,
                unit_ref: None,
                deep_link_url: pr_conversation_url(tx, pr_id)?,
                newest_activity_at: newest,
            });
        }
    }

    out.sort_by_key(|c| std::cmp::Reverse(c.newest_activity_at));
    Ok(out)
}

/// Compute the per-PR role-obligation dispatch after the recompute has run
/// (ADR 0031 amendment, issue #450). Separate dedup from the conversation
/// `last_emitted_activity_at`: a PR may legitimately emit both a conversation
/// trigger and a role trigger in the same cycle.
///
/// The current role *signature* uses the SAME host-gated logic as roll-up
/// branches C/D: `'changes_requested'` when the viewer authored the PR and its
/// `review_decision = 'CHANGES_REQUESTED'`, else `'review_request'` when the
/// viewer is in `requested_reviewers`, else `None`. Author + requested are
/// mutually exclusive in practice; if both somehow held, `'changes_requested'`
/// wins (checked first).
///
/// Returns at most one [`NotificationTrigger`] and, when the marker must move,
/// the value the caller writes into `last_emitted_role`:
///
/// - signature is `Some(sig)` AND `sig != last_emitted_role` -> emit one role
///   trigger for that kind; the caller sets `last_emitted_role = sig`.
/// - signature is `None` -> re-arm: the caller sets `last_emitted_role = NULL`.
/// - signature is `Some(sig)` AND `sig == last_emitted_role` -> no trigger, no
///   write (still in the same obligation episode).
///
/// The returned `Option<&'static str>` is the value to persist; `None` from
/// this position is "no write needed" only in the unchanged-signature case. The
/// caller distinguishes write-NULL (re-arm) from no-write via the
/// [`RoleDispatch`] return.
pub(super) fn role_dispatch(
    tx: &rusqlite::Transaction<'_>,
    account_id: i64,
    pr_id: i64,
    viewer_login: &str,
) -> Result<RoleDispatch, rusqlite::Error> {
    // A role obligation needs a relation row to dedup against (and to mark
    // unread per-row later). A missing row - the Team/Tracked-view path where
    // this account has no discovered relation to the PR - is a clean no-op,
    // mirroring the conversation re-arm's relation-row guard. The marker UPDATE
    // would match zero rows anyway; short-circuiting also avoids toasting an
    // obligation the viewer has no relation surface for.
    let last_emitted_role: Option<Option<String>> = tx
        .query_row(
            "SELECT last_emitted_role FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?;
    let Some(last_emitted_role) = last_emitted_role else {
        return Ok(RoleDispatch::none());
    };

    let signature = current_role_signature(tx, pr_id, viewer_login)?;

    match signature {
        None => {
            // Obligation cleared (or never held): re-arm. Writing NULL is
            // idempotent, but only act when the marker isn't already NULL so a
            // quiet PR doesn't churn the column every cycle.
            if last_emitted_role.is_some() {
                Ok(RoleDispatch {
                    trigger: None,
                    set_marker: Some(None),
                })
            } else {
                Ok(RoleDispatch::none())
            }
        }
        Some(kind) => {
            let sig = role_kind_storage(kind);
            if last_emitted_role.as_deref() == Some(sig) {
                // Same obligation still held; already toasted.
                return Ok(RoleDispatch::none());
            }
            let trigger = NotificationTrigger {
                account_id,
                pull_request_id: pr_id,
                kind: NotificationKind::NeedsAttention,
                unit_kind: kind,
                unit_ref: None,
                deep_link_url: pr_conversation_url(tx, pr_id)?,
                newest_activity_at: None,
            };
            Ok(RoleDispatch {
                trigger: Some(trigger),
                set_marker: Some(Some(sig.to_string())),
            })
        }
    }
}

/// Outcome of [`role_dispatch`]. `trigger` is the (optional) role toast to
/// dispatch after commit; `set_marker` is `None` for "leave `last_emitted_role`
/// untouched", `Some(None)` for "write NULL (re-arm)", and `Some(Some(sig))`
/// for "write the new signature".
pub(super) struct RoleDispatch {
    pub(super) trigger: Option<NotificationTrigger>,
    pub(super) set_marker: Option<Option<String>>,
}

impl RoleDispatch {
    fn none() -> Self {
        Self {
            trigger: None,
            set_marker: None,
        }
    }
}

/// Storage string for a role [`NotificationUnitKind`], matching the
/// `last_emitted_role` column values and the `notifications.unit_kind` form.
fn role_kind_storage(kind: NotificationUnitKind) -> &'static str {
    match kind {
        NotificationUnitKind::ChangesRequested => "changes_requested",
        NotificationUnitKind::ReviewRequest => "review_request",
        // The conversation kinds never reach here; keep the match total.
        NotificationUnitKind::Thread
        | NotificationUnitKind::General
        | NotificationUnitKind::Review => "review_request",
    }
}

/// The viewer's current role obligation on the PR, host-gated exactly as
/// roll-up branches C (requested reviewer) and D (CHANGES_REQUESTED on the
/// viewer's authored PR). Returns the role kind, or `None` when neither holds.
/// `'changes_requested'` wins when both somehow apply (checked first).
fn current_role_signature(
    tx: &rusqlite::Transaction<'_>,
    pr_id: i64,
    viewer_login: &str,
) -> Result<Option<NotificationUnitKind>, rusqlite::Error> {
    // (D) CHANGES_REQUESTED on the viewer's authored PR, host-gated. The caller
    // already early-exits on a host mismatch (the scan returns before reaching
    // here when the viewer host differs from the PR host), so a direct
    // author_login match is host-correct.
    let changes_requested: bool = tx.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM pull_requests pr
              WHERE pr.id = ?1
                AND pr.author_login = ?2
                AND pr.review_decision = 'CHANGES_REQUESTED'
         )",
        params![pr_id, viewer_login],
        |r| r.get::<_, i64>(0),
    )? == 1;
    if changes_requested {
        return Ok(Some(NotificationUnitKind::ChangesRequested));
    }

    // (C) requested reviewer, host-gated by the same early-exit.
    let review_requested: bool = tx.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM requested_reviewers rr
              WHERE rr.pull_request_id = ?1
                AND rr.login = ?2
         )",
        params![pr_id, viewer_login],
        |r| r.get::<_, i64>(0),
    )? == 1;
    if review_requested {
        return Ok(Some(NotificationUnitKind::ReviewRequest));
    }

    Ok(None)
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
