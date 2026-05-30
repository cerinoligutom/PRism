//! Mention scan + `needs_attention` roll-up recompute (ADR 0031; supersedes
//! the ADR 0015 four-signal composite).
//!
//! Runs inside the same DB transaction as `enrichment::write_pr_updates` so the
//! recompute sees the freshest threads, comments, requested reviewers, and
//! review decision. The scan sets the per-comment `mentions_viewer` bit the
//! roll-up folds into involvement (and still bumps the legacy
//! `mentioned_count_unread` counter the existing dispatch trigger reads).
//! Returns the (possibly empty) [`NotificationTrigger`]s for the ADR 0017
//! transitions observed in this cycle - the caller dispatches after commit.

use rusqlite::params;

use crate::github::AccountId;
use crate::notify::{NotificationKind, NotificationTrigger};

use super::unix_now;

mod mentions;
#[cfg(test)]
mod tests;

use mentions::mentions_viewer;

/// Count new `@<viewer-login>` mentions across the PR's comment bodies since
/// the per-(account, PR) watermark, set `mentions_viewer = 1` on each matched
/// comment, bump the legacy unread counter by that count, advance the watermark
/// to now, then recompute the `needs_attention` roll-up. See ADR 0031.
///
/// Watermark advance runs unconditionally so a cycle with zero new comments
/// still moves the cursor forward and the next scan starts from now.
///
/// Host isolation (issue #169): GitHub logins are unique per host, not
/// globally. Two PRism accounts can share the same login on different hosts
/// (e.g. `ada` on github.com and `ada` on github.acme.corp) but they are
/// different identities. The scan + recompute therefore matches on the
/// viewer's `(login, host)` pair, derived from `accounts WHERE id = ?1` and
/// the PR's owning host from `repos -> accounts`. A relation row whose viewer
/// host differs from the PR's host is treated as a no-op so cross-host login
/// collisions never inflate counters or flip `needs_attention`.
pub(super) fn scan_mentions_and_recompute_attention(
    tx: &rusqlite::Transaction<'_>,
    account_id: AccountId,
    pr_id: i64,
) -> Result<Vec<NotificationTrigger>, rusqlite::Error> {
    let account_id = account_id as i64;

    // Viewer (login, host). The relation row may not exist on this account
    // (Team-view path where the active account has no discovered relation to
    // the PR); in that case the UPDATE matches zero rows and the scan is a
    // clean no-op.
    let viewer: Option<(String, String)> = tx
        .query_row(
            "SELECT login, host FROM accounts WHERE id = ?1",
            params![account_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .ok();
    let Some((viewer_login, viewer_host)) = viewer else {
        return Ok(Vec::new());
    };

    // PR's owning host: the host of the account that owns the repo. Used to
    // confirm the viewer's identity lives on this PR's host before counting
    // mentions or matching the PR author / requested reviewer. A missing PR
    // row reads the same as "no relation" - clean no-op.
    let pr_owner_host: Option<String> = tx
        .query_row(
            "SELECT acc.host
               FROM pull_requests pr
               JOIN repos r ON r.id = pr.repo_id
               JOIN accounts acc ON acc.id = r.account_id
              WHERE pr.id = ?1",
            params![pr_id],
            |r| r.get::<_, String>(0),
        )
        .ok();
    let Some(pr_owner_host) = pr_owner_host else {
        return Ok(Vec::new());
    };
    if viewer_host != pr_owner_host {
        return Ok(Vec::new());
    }

    // Snapshot the row before the scan + recompute so we can spot the two
    // ADR 0017 transitions (0 -> 1 on `needs_attention`, strict increase on
    // `mentioned_count_unread`). The mention counter snapshot has to come
    // _before_ the UPDATE below bumps it; the attention snapshot can come
    // either side of that bump since the recompute UPDATE that follows is
    // the only thing that writes `needs_attention`.
    let before: Option<(i64, i64)> = tx
        .query_row(
            "SELECT needs_attention, mentioned_count_unread
               FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok();

    // Read the prior watermark. NULL or missing relation row reads as 0 so the
    // first cycle counts every comment newer than the epoch.
    let watermark: i64 = tx
        .query_row(
            "SELECT COALESCE(mention_scan_watermark_at, 0)
               FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0);

    // Pull (id, body) from review + issue comments newer than the watermark
    // and not authored by the viewer. Scan in Rust (word-boundary aware) rather
    // than via SQLite REGEXP so the worker doesn't need to register a custom
    // SQL function. Bodies are bounded by the per-PR comment volume on the
    // GitHub side; for v1 sizes a memory pass is cheap.
    //
    // A match does two things: it bumps the legacy `mentioned_count_unread`
    // counter (still read by the existing dispatch trigger; the next slice,
    // #433, retires it) and sets the persisted per-comment `mentions_viewer`
    // bit that the ADR-0031 roll-up folds into involvement. The bit is set,
    // never cleared (idempotent); re-running over an already-flagged comment is
    // a no-op write. Collecting the matched ids first keeps the borrow of the
    // prepared statement from overlapping the UPDATE.
    let mut new_mentions: i64 = 0;
    let mut matched_review_ids: Vec<i64> = Vec::new();
    let mut matched_issue_ids: Vec<i64> = Vec::new();
    {
        let mut review_stmt = tx.prepare(
            "SELECT c.id, c.body
               FROM review_comments c
               JOIN review_threads t ON t.id = c.review_thread_id
              WHERE t.pull_request_id = ?1
                AND c.author_login != ?2
                AND c.created_at > ?3",
        )?;
        let matches = review_stmt.query_map(params![pr_id, viewer_login, watermark], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for matched in matches {
            let (id, body) = matched?;
            if mentions_viewer(&body, &viewer_login) {
                new_mentions += 1;
                matched_review_ids.push(id);
            }
        }
    }
    {
        let mut issue_stmt = tx.prepare(
            "SELECT ic.id, ic.body
               FROM issue_comments ic
              WHERE ic.pull_request_id = ?1
                AND ic.author_login != ?2
                AND ic.created_at > ?3",
        )?;
        let matches = issue_stmt.query_map(params![pr_id, viewer_login, watermark], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for matched in matches {
            let (id, body) = matched?;
            if mentions_viewer(&body, &viewer_login) {
                new_mentions += 1;
                matched_issue_ids.push(id);
            }
        }
    }

    // Set the per-comment mention bit on each matched row. ADR 0031: a mention
    // is one reason a unit involves the viewer, so the roll-up reads this bit
    // rather than the relation-level counter.
    for id in &matched_review_ids {
        tx.execute(
            "UPDATE review_comments SET mentions_viewer = 1 WHERE id = ?1",
            params![id],
        )?;
    }
    for id in &matched_issue_ids {
        tx.execute(
            "UPDATE issue_comments SET mentions_viewer = 1 WHERE id = ?1",
            params![id],
        )?;
    }

    // Bump counter and advance watermark. Watermark moves forward on every
    // cycle (idempotency cursor) so re-runs without new comments stay flat.
    let now = unix_now();
    tx.execute(
        "UPDATE pull_request_viewer_relations
            SET mentioned_count_unread = mentioned_count_unread + ?1,
                mention_scan_watermark_at = ?2
          WHERE account_id = ?3 AND pull_request_id = ?4",
        params![new_mentions, now, account_id, pr_id],
    )?;

    // Composite recompute via the shared host-aware, row-correlated builder
    // in `triage::query` so this per-cycle path and the command paths run one
    // formula (ADR 0031). The WHERE scopes the UPDATE to this single
    // `(account_id, pull_request_id)` pair; the builder resolves the viewer's
    // `(login, host)` from `accounts viewer ON viewer.id = rel.account_id` and
    // matches it against the PR's owning host, so the early-exit above and the
    // formula agree on host isolation.
    tx.execute(
        &format!(
            "UPDATE pull_request_viewer_relations AS rel
                SET needs_attention = ({case_expr})
              WHERE rel.account_id = ?1 AND rel.pull_request_id = ?2",
            case_expr = crate::triage::query::needs_attention_case_expr(),
        ),
        params![account_id, pr_id],
    )?;

    // Compare to the pre-write snapshot. A missing relation row before the
    // write means the recompute UPDATE matched zero rows; no trigger fires.
    let Some((before_attention, before_mentions)) = before else {
        return Ok(Vec::new());
    };
    let after: Option<(i64, i64)> = tx
        .query_row(
            "SELECT needs_attention, mentioned_count_unread
               FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok();
    let Some((after_attention, after_mentions)) = after else {
        return Ok(Vec::new());
    };

    let mut triggers = Vec::new();
    if before_attention == 0 && after_attention == 1 {
        triggers.push(NotificationTrigger {
            account_id,
            pull_request_id: pr_id,
            kind: NotificationKind::NeedsAttention,
        });
    }
    if after_mentions > before_mentions {
        triggers.push(NotificationTrigger {
            account_id,
            pull_request_id: pr_id,
            kind: NotificationKind::Mention,
        });
    }
    Ok(triggers)
}
