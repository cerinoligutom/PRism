//! Read-only SQL composition for the conversation Tauri commands.
//!
//! See `docs/contracts/conversation-depth.md` for the SQL shapes and the
//! conversation-stats definitions, and ADR 0010 for the storage decisions
//! these queries depend on.
//!
//! - [`list_pr_threads`] - join `review_threads` + `review_comments` (head)
//!   + `accounts` (for `is_you_in`).
//! - [`get_conversation_stats`] - the four-tile stats card math (oldest
//!   unresolved, avg time-to-response, resolution rate, comment-type
//!   breakdown).
//! - [`list_thread_comments`] / [`list_issue_comments`] / [`list_reviews`] -
//!   helpers the lazy hydrator uses to return the persisted state after a
//!   round-trip.

use rusqlite::{params, Connection, Row};

use crate::conversation::types::{
    CommentBreakdown, ConversationStats, IssueComment, PullRequestReview, PullRequestThread,
    ThreadComment, ThreadHeadComment, ThreadState,
};

/// List per-thread state for a PR, joined to the head-comment snapshot. The
/// `account_id` parameter resolves `is_you_in` via an `EXISTS` against
/// `review_comments` joined to `accounts`; `None` always returns `is_you_in =
/// false`.
pub fn list_pr_threads(
    conn: &Connection,
    pull_request_id: i64,
    account_id: Option<i64>,
) -> Result<Vec<PullRequestThread>, rusqlite::Error> {
    let sql = "
        SELECT
            t.id,
            t.node_id,
            t.pull_request_id,
            t.is_resolved,
            t.is_outdated,
            t.path,
            t.line,
            t.start_line,
            t.original_line,
            t.reply_count,
            t.created_at,
            t.resolved_at,
            t.last_reply_at,
            t.head_comment_author_login,
            t.head_comment_body_text,
            t.head_comment_created_at,
            CASE
                WHEN ?2 IS NULL THEN 0
                WHEN EXISTS (
                    SELECT 1 FROM review_comments c
                      JOIN accounts a ON a.login = c.author_login
                     WHERE c.review_thread_id = t.id
                       AND a.id = ?2
                ) THEN 1
                ELSE 0
            END AS is_you_in
        FROM review_threads t
        WHERE t.pull_request_id = ?1
        ORDER BY COALESCE(t.created_at, 0), t.id
    ";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![pull_request_id, account_id], project_thread_row)?;
    rows.collect::<Result<Vec<_>, _>>()
}

fn project_thread_row(row: &Row<'_>) -> Result<PullRequestThread, rusqlite::Error> {
    let id: i64 = row.get(0)?;
    // `node_id` is nullable in the schema (rows seeded pre-M3 carry NULL); the
    // DTO is `String` because every row written by the M3 sync path populates
    // it. Fall back to an empty string so legacy fixture rows still surface.
    let node_id: Option<String> = row.get(1)?;
    let pull_request_id: i64 = row.get(2)?;
    let is_resolved: i64 = row.get(3)?;
    let is_outdated: i64 = row.get(4)?;
    let path: Option<String> = row.get(5)?;
    let line: Option<i64> = row.get(6)?;
    let start_line: Option<i64> = row.get(7)?;
    let original_line: Option<i64> = row.get(8)?;
    let reply_count: i64 = row.get(9)?;
    let created_at: Option<i64> = row.get(10)?;
    let resolved_at: Option<i64> = row.get(11)?;
    let last_reply_at: Option<i64> = row.get(12)?;
    let head_author: Option<String> = row.get(13)?;
    let head_body: Option<String> = row.get(14)?;
    let head_created_at: Option<i64> = row.get(15)?;
    let is_you_in: i64 = row.get(16)?;

    let head_comment = match (head_author, head_body, head_created_at) {
        (Some(author_login), Some(body_text), Some(created_at)) => Some(ThreadHeadComment {
            author_login,
            body_text,
            created_at,
        }),
        _ => None,
    };

    let state = if is_outdated != 0 {
        ThreadState::Outdated
    } else if is_resolved != 0 {
        ThreadState::Resolved
    } else {
        ThreadState::Unresolved
    };

    Ok(PullRequestThread {
        id,
        node_id: node_id.unwrap_or_default(),
        pull_request_id,
        state,
        path,
        line,
        start_line,
        original_line,
        reply_count,
        head_comment,
        created_at,
        resolved_at,
        last_reply_at,
        is_you_in: is_you_in != 0,
    })
}

/// Compute the four-tile conversation stats for a PR. All math runs at read
/// time; the worker doesn't pre-aggregate these.
pub fn get_conversation_stats(
    conn: &Connection,
    pull_request_id: i64,
) -> Result<ConversationStats, rusqlite::Error> {
    let counts = thread_counts(conn, pull_request_id)?;
    let oldest_unresolved_at = oldest_unresolved(conn, pull_request_id)?;
    let avg_response_seconds = avg_time_to_response(conn, pull_request_id)?;
    let resolution_rate = compute_resolution_rate(counts.resolved, counts.total, counts.outdated);
    let breakdown = comment_breakdown(conn, pull_request_id)?;

    Ok(ConversationStats {
        threads_total: counts.total,
        threads_unresolved: counts.unresolved,
        threads_resolved: counts.resolved,
        threads_outdated: counts.outdated,
        oldest_unresolved_at,
        avg_response_seconds,
        resolution_rate,
        comment_breakdown: breakdown,
    })
}

/// Thread-state counts split out so the resolution-rate math can reuse them.
#[derive(Debug, Clone, Copy)]
struct ThreadCounts {
    total: i64,
    unresolved: i64,
    resolved: i64,
    outdated: i64,
}

fn thread_counts(conn: &Connection, pull_request_id: i64) -> Result<ThreadCounts, rusqlite::Error> {
    // Single aggregation: total, unresolved, resolved, outdated. `unresolved`
    // AND `resolved` are both strict-active (exclude outdated) so the three
    // visible buckets — unresolved, resolved, outdated — are disjoint over the
    // active set (`total - outdated`). A thread that's both resolved AND
    // outdated counts only in `outdated`; this matches the threads list which
    // hides such threads behind the "Show N outdated" toggle and prevents the
    // resolution rate from overshooting 100%.
    let (total, unresolved, resolved, outdated): (i64, i64, i64, i64) = conn.query_row(
        "SELECT
             COUNT(*),
             SUM(CASE WHEN is_resolved = 0 AND is_outdated = 0 THEN 1 ELSE 0 END),
             SUM(CASE WHEN is_resolved = 1 AND is_outdated = 0 THEN 1 ELSE 0 END),
             SUM(CASE WHEN is_outdated = 1 THEN 1 ELSE 0 END)
           FROM review_threads
          WHERE pull_request_id = ?1",
        params![pull_request_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                row.get::<_, Option<i64>>(3)?.unwrap_or(0),
            ))
        },
    )?;
    Ok(ThreadCounts {
        total,
        unresolved,
        resolved,
        outdated,
    })
}

fn oldest_unresolved(
    conn: &Connection,
    pull_request_id: i64,
) -> Result<Option<i64>, rusqlite::Error> {
    conn.query_row(
        "SELECT MIN(created_at)
           FROM review_threads
          WHERE pull_request_id = ?1
            AND is_resolved = 0
            AND is_outdated = 0
            AND created_at IS NOT NULL",
        params![pull_request_id],
        |row| row.get::<_, Option<i64>>(0),
    )
}

/// Average gap between consecutive `review_comments.created_at` within each
/// thread, computed via LAG over thread-partitioned comments. `None` when no
/// thread has a reply yet.
fn avg_time_to_response(
    conn: &Connection,
    pull_request_id: i64,
) -> Result<Option<i64>, rusqlite::Error> {
    let avg: Option<f64> = conn.query_row(
        "WITH gaps AS (
             SELECT
                 c.created_at -
                     LAG(c.created_at) OVER (
                         PARTITION BY c.review_thread_id
                         ORDER BY c.created_at
                     ) AS gap_seconds
               FROM review_comments c
               JOIN review_threads t ON t.id = c.review_thread_id
              WHERE t.pull_request_id = ?1
         )
         SELECT AVG(gap_seconds) FROM gaps WHERE gap_seconds IS NOT NULL",
        params![pull_request_id],
        |row| row.get::<_, Option<f64>>(0),
    )?;
    Ok(avg.map(|v| v.round() as i64))
}

fn comment_breakdown(
    conn: &Connection,
    pull_request_id: i64,
) -> Result<CommentBreakdown, rusqlite::Error> {
    // `review` sums `reply_count + 1` across the PR's threads. The sync cycle
    // writes `reply_count = comments.totalCount - 1` for every thread, so the
    // sum recovers the cycle-accurate review-comment total without depending
    // on the lazy hydrator having populated `review_comments`. Pre-fix this
    // counted rows in `review_comments` directly, which read zero on PRs that
    // had never been drawer-opened.
    let (review, issue, summary): (i64, i64, i64) = conn.query_row(
        "SELECT
             (SELECT COALESCE(SUM(reply_count + 1), 0)
                FROM review_threads
               WHERE pull_request_id = ?1),
             (SELECT COALESCE(issue_comments_count, 0)
                FROM pull_requests WHERE id = ?1),
             (SELECT COUNT(*) FROM reviews
               WHERE pull_request_id = ?1
                 AND body IS NOT NULL
                 AND body <> '')",
        params![pull_request_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok(CommentBreakdown {
        review,
        issue,
        summary,
        total: review + issue + summary,
    })
}

/// `resolved / (total - outdated)`. Zero when the denominator is zero.
fn compute_resolution_rate(resolved: i64, total: i64, outdated: i64) -> f64 {
    let denom = total - outdated;
    if denom <= 0 {
        0.0
    } else {
        (resolved as f64) / (denom as f64)
    }
}

/// Every thread comment for a PR, ordered by `created_at`. Returned alongside
/// the hydrated DTO so the frontend renders without a second round-trip.
pub fn list_thread_comments(
    conn: &Connection,
    pull_request_id: i64,
) -> Result<Vec<ThreadComment>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.review_thread_id, c.author_login, c.body, c.created_at, c.line, c.side
           FROM review_comments c
           JOIN review_threads t ON t.id = c.review_thread_id
          WHERE t.pull_request_id = ?1
          ORDER BY c.review_thread_id, c.created_at, c.id",
    )?;
    let rows = stmt.query_map(params![pull_request_id], |row| {
        Ok(ThreadComment {
            id: row.get(0)?,
            thread_id: row.get(1)?,
            author_login: row.get(2)?,
            body: row.get(3)?,
            created_at: row.get(4)?,
            line: row.get(5)?,
            side: row.get(6)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
}

pub fn list_issue_comments(
    conn: &Connection,
    pull_request_id: i64,
) -> Result<Vec<IssueComment>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, author_login, body, created_at
           FROM issue_comments
          WHERE pull_request_id = ?1
          ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map(params![pull_request_id], |row| {
        Ok(IssueComment {
            id: row.get(0)?,
            author_login: row.get(1)?,
            body: row.get(2)?,
            created_at: row.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
}

pub fn list_reviews(
    conn: &Connection,
    pull_request_id: i64,
) -> Result<Vec<PullRequestReview>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, node_id, reviewer_login, state, body, submitted_at
           FROM reviews
          WHERE pull_request_id = ?1
          ORDER BY COALESCE(submitted_at, 0), id",
    )?;
    let rows = stmt.query_map(params![pull_request_id], |row| {
        let node_id: Option<String> = row.get(1)?;
        Ok(PullRequestReview {
            id: row.get(0)?,
            node_id: node_id.unwrap_or_default(),
            author_login: row.get(2)?,
            state: row.get(3)?,
            body: row.get(4)?,
            submitted_at: row.get(5)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_rate_zero_when_no_active_threads() {
        assert_eq!(compute_resolution_rate(0, 0, 0), 0.0);
        assert_eq!(compute_resolution_rate(0, 3, 3), 0.0);
    }

    #[test]
    fn resolution_rate_handles_typical_case() {
        // 2 resolved / (5 total - 1 outdated) = 2 / 4 = 0.5
        assert_eq!(compute_resolution_rate(2, 5, 1), 0.5);
    }

    #[test]
    fn resolution_rate_full() {
        // 4 resolved / (4 total - 0 outdated) = 1.0
        assert_eq!(compute_resolution_rate(4, 4, 0), 1.0);
    }
}
