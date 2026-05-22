//! Format a [`NotificationTrigger`] into a user-facing
//! [`Notification`] by joining against the local cache for the PR title, repo
//! slug, number, and (for mentions) the most recent unread comment excerpt.
//!
//! Title / body strings follow ADR 0017 decision 1:
//!
//! * [`NotificationKind::NeedsAttention`] -> title "Needs your attention",
//!   body "<owner>/<repo> #<number> - <pr_title>".
//! * [`NotificationKind::Mention`] -> title "New mention in <owner>/<repo>
//!   #<number>", body the latest unread mention excerpt (~80 chars, single
//!   line). Falls back to the PR title when no qualifying comment exists.
//!
//! The formatter takes a `&Connection` so callers can run it outside the
//! recompute transaction without re-locking the DB. A formatter failure
//! produces `None`; the worker logs and skips dispatch rather than firing a
//! toast against missing data.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

use crate::notify::types::{Notification, NotificationKind, NotificationTrigger};

const EXCERPT_CHAR_LIMIT: usize = 80;

/// Format a single trigger into a dispatchable notification. Returns `None`
/// when the PR or repo can't be resolved (typically a row deleted between the
/// recompute UPDATE and the post-commit dispatch).
pub fn format_trigger(conn: &Connection, trigger: &NotificationTrigger) -> Option<Notification> {
    let (owner, repo, number, title) = pr_lookup(conn, trigger.pull_request_id)?;
    let payload = json!({
        "account_id": trigger.account_id,
        "pull_request_id": trigger.pull_request_id,
    });
    let notification = match trigger.kind {
        NotificationKind::NeedsAttention => Notification {
            title: "Needs your attention".to_string(),
            body: format!("{owner}/{repo} #{number} - {title}"),
            payload,
        },
        NotificationKind::Mention => {
            // Latest qualifying comment body, trimmed and clipped.
            let excerpt =
                latest_mention_excerpt(conn, trigger.account_id, trigger.pull_request_id, &title);
            Notification {
                title: format!("New mention in {owner}/{repo} #{number}"),
                body: excerpt,
                payload,
            }
        }
    };
    Some(notification)
}

fn pr_lookup(conn: &Connection, pr_id: i64) -> Option<(String, String, i64, String)> {
    conn.query_row(
        "SELECT r.owner, r.name, pr.number, pr.title
           FROM pull_requests pr
           JOIN repos r ON r.id = pr.repo_id
          WHERE pr.id = ?1",
        params![pr_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// Best-effort excerpt from the most recent review comment on the PR whose
/// `created_at` sits past the relation row's `mention_scan_watermark_at`. The
/// scan + recompute pair advances the watermark _before_ the recompute runs,
/// so by the time the formatter looks the new comment has the freshest
/// timestamp at or after the watermark. Falls back to the PR title when no
/// qualifying comment is found (e.g. the trigger fired from a mark-read
/// recompute, where the counter rose via a sync write that committed
/// elsewhere). Issue comments aren't queried here in v1 - the contract notes
/// the review-comments table holds the bulk of @-mentions during reviews; an
/// open follow-up extends to issue_comments once the body shape is unified.
fn latest_mention_excerpt(
    conn: &Connection,
    account_id: i64,
    pr_id: i64,
    fallback_title: &str,
) -> String {
    let body: Option<String> = conn
        .query_row(
            "SELECT c.body
               FROM review_comments c
               JOIN review_threads t ON t.id = c.review_thread_id
              WHERE t.pull_request_id = ?1
                AND c.created_at >= COALESCE(
                    (SELECT mention_scan_watermark_at
                       FROM pull_request_viewer_relations
                      WHERE account_id = ?2 AND pull_request_id = ?1), 0)
              ORDER BY c.created_at DESC
              LIMIT 1",
            params![pr_id, account_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten();
    body.map(|raw| excerpt_single_line(&raw))
        .unwrap_or_else(|| excerpt_single_line(fallback_title))
}

/// Collapse a multi-line comment body into a single line and clip to
/// [`EXCERPT_CHAR_LIMIT`] chars. Replaces every run of whitespace with a
/// single space so the toast row stays one line tall; appends an ellipsis
/// when the source overflowed.
fn excerpt_single_line(input: &str) -> String {
    let collapsed: String = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= EXCERPT_CHAR_LIMIT {
        return collapsed;
    }
    let mut clipped: String = collapsed.chars().take(EXCERPT_CHAR_LIMIT).collect();
    clipped.push('\u{2026}');
    clipped
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

    fn seed_pr(conn: &Connection, owner: &str, repo: &str, pr_id: i64, number: i64, title: &str) {
        conn.execute_batch(&format!(
            "INSERT OR IGNORE INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT OR IGNORE INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, '{owner}', '{repo}', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES ({pr_id}, 10, {number}, '{title}', 'open', 0, 'bob',
                        0, 0, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, last_seen_at,
                 mention_scan_watermark_at, mentioned_count_unread)
                VALUES (1, {pr_id}, 0, 100, 0);"
        ))
        .unwrap();
    }

    #[test]
    fn formats_needs_attention_with_pr_title_and_repo_slug() {
        let conn = fresh_db();
        seed_pr(&conn, "owner", "web", 100, 42, "Add a thing");
        let trigger = NotificationTrigger {
            account_id: 1,
            pull_request_id: 100,
            kind: NotificationKind::NeedsAttention,
        };
        let n = format_trigger(&conn, &trigger).expect("formatted");
        assert_eq!(n.title, "Needs your attention");
        assert_eq!(n.body, "owner/web #42 - Add a thing");
        assert_eq!(n.payload["account_id"], 1);
        assert_eq!(n.payload["pull_request_id"], 100);
    }

    #[test]
    fn formats_mention_title_with_repo_and_number() {
        let conn = fresh_db();
        seed_pr(&conn, "owner", "web", 100, 42, "Add a thing");
        let trigger = NotificationTrigger {
            account_id: 1,
            pull_request_id: 100,
            kind: NotificationKind::Mention,
        };
        let n = format_trigger(&conn, &trigger).expect("formatted");
        assert_eq!(n.title, "New mention in owner/web #42");
    }

    #[test]
    fn mention_body_pulls_latest_review_comment_excerpt() {
        let conn = fresh_db();
        seed_pr(&conn, "owner", "web", 100, 42, "Add a thing");
        conn.execute_batch(
            "INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, node_id)
                VALUES (200, 100, 0, 0, 'RT_1');
             INSERT INTO review_comments
                (id, review_thread_id, author_login, body, created_at)
                VALUES (300, 200, 'bob', 'older comment', 90),
                       (301, 200, 'bob', 'hey @alice can you look at this?', 200);",
        )
        .unwrap();
        let trigger = NotificationTrigger {
            account_id: 1,
            pull_request_id: 100,
            kind: NotificationKind::Mention,
        };
        let n = format_trigger(&conn, &trigger).expect("formatted");
        assert_eq!(n.body, "hey @alice can you look at this?");
    }

    #[test]
    fn mention_body_falls_back_to_pr_title_when_no_comment() {
        let conn = fresh_db();
        seed_pr(&conn, "owner", "web", 100, 42, "Add a thing");
        let trigger = NotificationTrigger {
            account_id: 1,
            pull_request_id: 100,
            kind: NotificationKind::Mention,
        };
        let n = format_trigger(&conn, &trigger).expect("formatted");
        assert_eq!(n.body, "Add a thing");
    }

    #[test]
    fn excerpt_collapses_whitespace_and_clips_to_eighty_chars() {
        let long = "alpha\n\nbeta\tgamma".to_string() + &"x".repeat(200);
        let trimmed = excerpt_single_line(&long);
        let char_count = trimmed.chars().count();
        assert_eq!(char_count, EXCERPT_CHAR_LIMIT + 1, "limit + ellipsis");
        assert!(trimmed.ends_with('\u{2026}'));
        assert!(trimmed.starts_with("alpha beta gamma"));
    }

    #[test]
    fn returns_none_when_pr_missing() {
        let conn = fresh_db();
        // No PR rows seeded.
        let trigger = NotificationTrigger {
            account_id: 1,
            pull_request_id: 100,
            kind: NotificationKind::NeedsAttention,
        };
        assert!(format_trigger(&conn, &trigger).is_none());
    }
}
