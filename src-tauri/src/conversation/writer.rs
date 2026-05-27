//! Shared upserts for conversation-depth tables.
//!
//! ADR 0029 made the sync worker the canonical writer for `review_comments`
//! and `issue_comments`; the per-row helpers below were previously private to
//! `conversation/commands.rs`. They're hoisted here so both the sync worker's
//! enrichment transaction and any remaining drawer-side write path call the
//! same SQL.
//!
//! Every helper takes a `&rusqlite::Transaction<'_>` so callers stay in
//! control of the surrounding atomicity boundary.

use rusqlite::params;

use crate::github::graphql::{IssueCommentNode, ReviewCommentNode};

/// Update `review_threads.diff_hunk` for the given thread. The
/// `COALESCE(?1, review_threads.diff_hunk)` form preserves any value already
/// on the row when the payload omits the field, so a paginated re-fetch that
/// drops `diffHunk` (or a legacy fixture that never carried it) can't blank an
/// existing hunk.
pub(crate) fn update_thread_diff_hunk(
    tx: &rusqlite::Transaction<'_>,
    thread_id: i64,
    diff_hunk: Option<&str>,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "UPDATE review_threads
            SET diff_hunk = COALESCE(?1, diff_hunk)
          WHERE id = ?2",
        params![diff_hunk, thread_id],
    )?;
    Ok(())
}

/// Upsert one review comment, keyed on `node_id`. Also primes the `users`
/// avatar cache from the comment's author. `COALESCE(excluded.url, ...)` and
/// the matching `body_html` clause preserve previously-persisted optional
/// fields when a later payload omits them - the sync worker's expanded
/// `PR_DETAIL_QUERY` carries every field, but defensive parity here is cheap.
pub(crate) fn upsert_review_comment(
    tx: &rusqlite::Transaction<'_>,
    thread_id: i64,
    comment: &ReviewCommentNode,
) -> Result<(), rusqlite::Error> {
    let author = comment
        .author
        .as_ref()
        .map(|a| a.login.as_str())
        .unwrap_or("");
    let created_at = rfc3339_to_unix(&comment.created_at).unwrap_or(0);
    if let Some(actor) = comment.author.as_ref() {
        upsert_user_avatar(tx, &actor.login, actor.avatar_url.as_deref(), created_at)?;
    }
    // The unique constraint on `node_id` is a partial index
    // (`WHERE node_id IS NOT NULL`); the conflict target needs the matching
    // predicate. Every caller writes a non-null `node_id`.
    tx.execute(
        "INSERT INTO review_comments
            (review_thread_id, author_login, body, created_at, node_id,
             database_id, line, side, url, body_html)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(node_id) WHERE node_id IS NOT NULL DO UPDATE SET
            review_thread_id = excluded.review_thread_id,
            author_login     = excluded.author_login,
            body             = excluded.body,
            created_at       = excluded.created_at,
            database_id      = excluded.database_id,
            line             = excluded.line,
            side             = excluded.side,
            url              = COALESCE(excluded.url, review_comments.url),
            body_html        = COALESCE(excluded.body_html, review_comments.body_html)",
        params![
            thread_id,
            author,
            comment.body,
            created_at,
            comment.id,
            comment.database_id,
            comment.line,
            comment.side,
            comment.url,
            comment.body_html,
        ],
    )?;
    Ok(())
}

/// Upsert one issue comment, keyed on `node_id`. Mirrors
/// [`upsert_review_comment`] for the PR-level conversation surface.
pub(crate) fn upsert_issue_comment(
    tx: &rusqlite::Transaction<'_>,
    pull_request_id: i64,
    comment: &IssueCommentNode,
) -> Result<(), rusqlite::Error> {
    let author = comment
        .author
        .as_ref()
        .map(|a| a.login.as_str())
        .unwrap_or("");
    let created_at = rfc3339_to_unix(&comment.created_at).unwrap_or(0);
    if let Some(actor) = comment.author.as_ref() {
        upsert_user_avatar(tx, &actor.login, actor.avatar_url.as_deref(), created_at)?;
    }
    tx.execute(
        "INSERT INTO issue_comments
            (pull_request_id, author_login, body, created_at, node_id,
             database_id, url, body_html)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(node_id) WHERE node_id IS NOT NULL DO UPDATE SET
            pull_request_id = excluded.pull_request_id,
            author_login    = excluded.author_login,
            body            = excluded.body,
            created_at      = excluded.created_at,
            database_id     = excluded.database_id,
            url             = COALESCE(excluded.url, issue_comments.url),
            body_html       = COALESCE(excluded.body_html, issue_comments.body_html)",
        params![
            pull_request_id,
            author,
            comment.body,
            created_at,
            comment.id,
            comment.database_id,
            comment.url,
            comment.body_html,
        ],
    )?;
    Ok(())
}

/// Mirror of the worker's user-cache upsert (ADR 0013). No-op when
/// `avatar_url` is `None` or empty - we never blank an existing row with a
/// NULL on a partial payload.
pub(crate) fn upsert_user_avatar(
    tx: &rusqlite::Transaction<'_>,
    login: &str,
    avatar_url: Option<&str>,
    last_seen_at: i64,
) -> Result<(), rusqlite::Error> {
    let Some(url) = avatar_url else {
        return Ok(());
    };
    if login.is_empty() || url.is_empty() {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO users (login, avatar_url, last_seen_at)
            VALUES (?1, ?2, ?3)
         ON CONFLICT(login) DO UPDATE SET
            avatar_url = excluded.avatar_url,
            last_seen_at = excluded.last_seen_at",
        params![login, url, last_seen_at],
    )?;
    Ok(())
}

fn rfc3339_to_unix(s: &str) -> Option<i64> {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    OffsetDateTime::parse(s, &Rfc3339)
        .ok()
        .map(|t| t.unix_timestamp())
}
