//! Per-PR review thread + review writes. Both follow the upsert + prune
//! pattern: every node_id in the latest payload becomes a row, any node_id
//! not present is deleted.
//!
//! ADR 0029: this module is also the canonical writer for `review_comments`.
//! The per-thread comments arriving in `PR_DETAIL_QUERY` are upserted via
//! `conversation::writer::upsert_review_comment` after the thread row exists,
//! and the head comment's `diff_hunk` is propagated onto `review_threads`.
//! Issue comments land via `write_issue_comments` from the same transaction.

use rusqlite::params;

use super::super::{rfc3339_to_unix, unix_now};
use crate::conversation::writer::{
    update_thread_diff_hunk, upsert_issue_comment, upsert_review_comment,
};

/// Upsert per-thread state AND every comment in each thread. Tracks
/// transitions on `is_resolved` so `resolved_at` is set when a thread becomes
/// resolved and cleared when it flips back. Prunes any prior thread for this
/// PR whose `node_id` is absent from the fetched set; cascading deletes on
/// `review_comments` follow. Comments themselves are upsert-only — a comment
/// deleted in-place on GitHub stays locally until its thread is pruned (the
/// payload caps at the first 100 comments per thread, so per-comment pruning
/// would wrongly drop reply pages beyond the cap).
pub(super) fn write_review_threads(
    tx: &rusqlite::Transaction<'_>,
    pr_id: i64,
    threads: &[crate::github::graphql::ReviewThread],
) -> Result<(), rusqlite::Error> {
    use std::collections::HashMap;

    // Snapshot the existing rows so we can detect resolve transitions
    // (set `resolved_at` only on the cycle the flag flips) and preserve
    // `created_at` once it's stamped.
    let mut existing: HashMap<String, ExistingThread> = HashMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT node_id, is_resolved, resolved_at, created_at
               FROM review_threads
              WHERE pull_request_id = ?1 AND node_id IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![pr_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                ExistingThread {
                    is_resolved: r.get::<_, i64>(1)? != 0,
                    resolved_at: r.get::<_, Option<i64>>(2)?,
                    created_at: r.get::<_, Option<i64>>(3)?,
                },
            ))
        })?;
        for row in rows {
            let (node_id, info) = row?;
            existing.insert(node_id, info);
        }
    }

    for thread in threads {
        let head = thread.comments.nodes.first();
        let head_created_at = head.and_then(|c| rfc3339_to_unix(&c.created_at));
        // `PullRequestReviewThread` has no `url` field on GitHub's GraphQL
        // schema (issue #115). The thread permalink is the head comment's
        // url; absent a head comment, leave the column NULL.
        let head_url = head.and_then(|c| c.url.as_deref());

        let prior = existing.remove(&thread.id);
        let created_at = prior
            .as_ref()
            .and_then(|p| p.created_at)
            .or(head_created_at);

        // Resolved-at follows the resolved flag transition: set on the cycle
        // it flips true, clear on the cycle it flips back. Preserve when the
        // state is unchanged.
        let resolved_at = match (prior.as_ref().map(|p| p.is_resolved), thread.is_resolved) {
            (Some(true), true) => prior.as_ref().and_then(|p| p.resolved_at),
            (Some(false), true) | (None, true) => Some(unix_now()),
            (_, false) => None,
        };

        // The reply count denormalises the post-head replies. `totalCount`
        // covers head + replies; one comment means zero replies.
        let reply_count = (thread.comments.total_count - 1).max(0);

        // ADR 0029: the `head_comment_*` denorm columns are gone; the
        // conversation read query derives the head from `review_comments`
        // directly. `last_reply_at` keeps the head's createdAt as its seed
        // because v1's stats math reads it for the threads-bar timestamp.
        tx.execute(
            "INSERT INTO review_threads
                (pull_request_id, node_id, is_resolved, is_outdated, path,
                 line, start_line, original_line, created_at, resolved_at,
                 last_reply_at, reply_count, url)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(node_id) WHERE node_id IS NOT NULL DO UPDATE SET
                pull_request_id = excluded.pull_request_id,
                is_resolved = excluded.is_resolved,
                is_outdated = excluded.is_outdated,
                path = excluded.path,
                line = excluded.line,
                start_line = excluded.start_line,
                original_line = excluded.original_line,
                created_at = COALESCE(review_threads.created_at, excluded.created_at),
                resolved_at = excluded.resolved_at,
                last_reply_at = excluded.last_reply_at,
                reply_count = excluded.reply_count,
                url = COALESCE(excluded.url, review_threads.url)",
            params![
                pr_id,
                thread.id,
                thread.is_resolved as i64,
                thread.is_outdated as i64,
                thread.path,
                thread.line,
                thread.start_line,
                thread.original_line,
                created_at,
                resolved_at,
                head_created_at,
                reply_count,
                head_url,
            ],
        )?;

        // Resolve the just-written thread's local rowid so `review_comments`
        // can FK-reference it. The thread row is guaranteed to exist at this
        // point (the upsert above either inserted or matched on `node_id`).
        let local_thread_id: i64 = tx.query_row(
            "SELECT id FROM review_threads WHERE node_id = ?1",
            params![thread.id],
            |r| r.get(0),
        )?;

        // Propagate the head comment's `diff_hunk` onto the thread row so the
        // conversation surface can render the diff context once per thread
        // (every comment in a thread shares the same hunk). The
        // `COALESCE(?, diff_hunk)` form keeps a previously-persisted value if
        // a later payload happens to omit the field.
        if let Some(head) = thread.comments.nodes.first() {
            update_thread_diff_hunk(tx, local_thread_id, head.diff_hunk.as_deref())?;
        }

        // Upsert every comment node arriving in the payload. The query caps
        // at `first: 100` per thread; threads with more comments paginate
        // their tail beyond the cap, matching the lazy hydrator's prior
        // coverage. See ADR 0029.
        for comment in &thread.comments.nodes {
            upsert_review_comment(tx, local_thread_id, comment)?;
        }
    }

    // Pruning: any thread row left in the snapshot wasn't present in the
    // latest fetch, so the thread has been removed on GitHub. Comments
    // cascade via the existing FK.
    //
    // ADR 0029 cap: the cycle's `pr_detail_extend_pages` follows up to 4
    // pages of `reviewThreads`, so PRs with >400 threads (vanishingly rare)
    // will see threads on pages 5+ pruned as "absent". Documented as a known
    // truncation, not a bug — the cap is well above v1 reality.
    for stale in existing.keys() {
        tx.execute(
            "DELETE FROM review_threads
              WHERE pull_request_id = ?1 AND node_id = ?2",
            params![pr_id, stale],
        )?;
    }

    Ok(())
}

#[derive(Debug)]
struct ExistingThread {
    is_resolved: bool,
    resolved_at: Option<i64>,
    created_at: Option<i64>,
}

/// Upsert every PR-level issue comment from the cycle payload. Upsert-only,
/// for the same reason `write_review_threads` doesn't prune review_comments:
/// the payload caps at the first 100 issue comments, so per-row pruning would
/// wrongly drop comments on later pages.
pub(super) fn write_issue_comments(
    tx: &rusqlite::Transaction<'_>,
    pr_id: i64,
    comments: &[crate::github::graphql::IssueCommentNode],
) -> Result<(), rusqlite::Error> {
    for comment in comments {
        upsert_issue_comment(tx, pr_id, comment)?;
    }
    Ok(())
}

/// Upsert submitted reviews and prune any prior row whose `node_id` is absent
/// from the fetched set.
pub(super) fn write_reviews(
    tx: &rusqlite::Transaction<'_>,
    pr_id: i64,
    reviews: &[crate::github::graphql::PullRequestReviewNode],
) -> Result<(), rusqlite::Error> {
    use std::collections::HashSet;

    let mut existing: HashSet<String> = HashSet::new();
    {
        let mut stmt = tx.prepare(
            "SELECT node_id FROM reviews
              WHERE pull_request_id = ?1 AND node_id IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![pr_id], |r| r.get::<_, String>(0))?;
        for row in rows {
            existing.insert(row?);
        }
    }

    for review in reviews {
        let author = review
            .author
            .as_ref()
            .map(|a| a.login.as_str())
            .unwrap_or("");
        let submitted_at = review.submitted_at.as_deref().and_then(rfc3339_to_unix);

        // Same partial-index conflict target shape as review_threads.
        // `body_html` is COALESCEd so a payload that omits the field doesn't
        // blank a previously-populated row (ADR 0014, issue #138). The same
        // protection applies for `body` already today.
        tx.execute(
            "INSERT INTO reviews
                (pull_request_id, node_id, reviewer_login, state, submitted_at,
                 body, body_html, url)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(node_id) WHERE node_id IS NOT NULL DO UPDATE SET
                pull_request_id = excluded.pull_request_id,
                reviewer_login = excluded.reviewer_login,
                state = excluded.state,
                submitted_at = excluded.submitted_at,
                body = excluded.body,
                body_html = COALESCE(excluded.body_html, reviews.body_html),
                url = COALESCE(excluded.url, reviews.url)",
            params![
                pr_id,
                review.id,
                author,
                review.state,
                submitted_at,
                review.body,
                review.body_html,
                review.url,
            ],
        )?;

        existing.remove(&review.id);
    }

    // Pruning: any review row whose node_id wasn't in the latest fetch is
    // gone upstream; drop it locally.
    for stale in &existing {
        tx.execute(
            "DELETE FROM reviews
              WHERE pull_request_id = ?1 AND node_id = ?2",
            params![pr_id, stale],
        )?;
    }

    Ok(())
}
