//! Format a [`NotificationTrigger`] into a user-facing
//! [`Notification`] by joining against the local cache for the PR title, repo
//! slug, number, and the conversation unit the trigger points at (ADR 0031).
//!
//! Title / body strings:
//!
//! * **Thread unit** -> title "Needs your attention", body "<owner>/<repo>
//!   #<number> - <path>:<line>" when the thread carries a file location, else
//!   the PR title.
//! * **General stream** -> title "Needs your attention", body "<owner>/<repo>
//!   #<number> - General discussion".
//! * **Review request** (role obligation) -> title "Review requested", body
//!   "You've been asked to review <owner>/<repo> #<number>".
//! * **Changes requested** (role obligation) -> title "Changes requested",
//!   body "Changes were requested on your <owner>/<repo> #<number>".
//!
//! `deep_link_url` is threaded from the trigger (the thread url, or the PR
//! conversation url for the general stream and the role kinds) into both the
//! snapshot and the click payload so the toast click reconciles the exact
//! unit / opens the PR.
//!
//! The formatter takes a `&Connection` so callers can run it outside the
//! recompute transaction without re-locking the DB. A formatter failure
//! produces `None`; the worker logs and skips dispatch rather than firing a
//! toast against missing data.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

use crate::notify::types::{
    Notification, NotificationSnapshot, NotificationTrigger, NotificationUnitKind,
};

/// Format a single trigger into a dispatchable notification. Returns `None`
/// when the PR or repo can't be resolved (typically a row deleted between the
/// recompute UPDATE and the post-commit dispatch).
pub fn format_trigger(conn: &Connection, trigger: &NotificationTrigger) -> Option<Notification> {
    let (owner, repo, number, title) = pr_lookup(conn, trigger.pull_request_id)?;
    let payload = json!({
        "account_id": trigger.account_id,
        "pull_request_id": trigger.pull_request_id,
        "unit_kind": trigger.unit_kind.as_storage(),
        "unit_ref": trigger.unit_ref,
        "deep_link_url": trigger.deep_link_url,
    });
    // Snapshot carried into the persistent inbox row (#378). Cloning is fine -
    // the dispatch path is per-trigger and the strings are short.
    let snapshot = NotificationSnapshot {
        kind: trigger.kind,
        account_id: trigger.account_id,
        pull_request_id: Some(trigger.pull_request_id),
        owner: owner.clone(),
        repo: repo.clone(),
        pr_number: number,
        pr_node_id: None,
        pr_title: title.clone(),
        unit_kind: Some(trigger.unit_kind),
        unit_ref: trigger.unit_ref.clone(),
        deep_link_url: trigger.deep_link_url.clone(),
    };

    let (title_copy, body_copy) = match trigger.unit_kind {
        NotificationUnitKind::Thread => {
            let unit_label = trigger
                .unit_ref
                .as_deref()
                .and_then(|node_id| thread_location_label(conn, node_id))
                .unwrap_or_else(|| title.clone());
            (
                "Needs your attention".to_string(),
                format!("{owner}/{repo} #{number} - {unit_label}"),
            )
        }
        NotificationUnitKind::General => (
            "Needs your attention".to_string(),
            format!("{owner}/{repo} #{number} - General discussion"),
        ),
        NotificationUnitKind::ReviewRequest => (
            "Review requested".to_string(),
            format!("You've been asked to review {owner}/{repo} #{number}"),
        ),
        NotificationUnitKind::ChangesRequested => (
            "Changes requested".to_string(),
            format!("Changes were requested on your {owner}/{repo} #{number}"),
        ),
    };

    Some(Notification {
        title: title_copy,
        body: body_copy,
        payload,
        snapshot: Some(snapshot),
    })
}

/// Build a `path:line` label for a thread unit from `review_threads`. Returns
/// `None` when the thread row can't be resolved or carries no `path`; the
/// caller falls back to the PR title. `line` prefers the current `line`, then
/// `original_line` (an outdated thread keeps only the latter).
fn thread_location_label(conn: &Connection, node_id: &str) -> Option<String> {
    let (path, line): (Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT path, COALESCE(line, original_line)
               FROM review_threads
              WHERE node_id = ?1",
            params![node_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                ))
            },
        )
        .optional()
        .ok()
        .flatten()?;
    let path = path?;
    match line {
        Some(n) => Some(format!("{path}:{n}")),
        None => Some(path),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::types::NotificationKind;
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
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES ({pr_id}, 10, {number}, '{title}', 'open', 0, 'bob',
                        0, 0, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at,
                 mention_scan_watermark_at, mentioned_count_unread)
                VALUES (1, {pr_id}, 0, 100, 0);"
        ))
        .unwrap();
    }

    fn thread_trigger(node_id: &str, url: Option<&str>) -> NotificationTrigger {
        NotificationTrigger {
            account_id: 1,
            pull_request_id: 100,
            kind: NotificationKind::NeedsAttention,
            unit_kind: NotificationUnitKind::Thread,
            unit_ref: Some(node_id.to_string()),
            deep_link_url: url.map(str::to_string),
            newest_activity_at: Some(200),
        }
    }

    fn general_trigger(url: Option<&str>) -> NotificationTrigger {
        NotificationTrigger {
            account_id: 1,
            pull_request_id: 100,
            kind: NotificationKind::NeedsAttention,
            unit_kind: NotificationUnitKind::General,
            unit_ref: None,
            deep_link_url: url.map(str::to_string),
            newest_activity_at: Some(200),
        }
    }

    fn role_trigger(unit_kind: NotificationUnitKind, url: Option<&str>) -> NotificationTrigger {
        NotificationTrigger {
            account_id: 1,
            pull_request_id: 100,
            kind: NotificationKind::NeedsAttention,
            unit_kind,
            unit_ref: None,
            deep_link_url: url.map(str::to_string),
            newest_activity_at: None,
        }
    }

    #[test]
    fn thread_body_uses_path_and_line() {
        let conn = fresh_db();
        seed_pr(&conn, "owner", "web", 100, 42, "Add a thing");
        conn.execute(
            "INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, node_id, path, line)
                VALUES (200, 100, 0, 0, 'RT_1', 'src/lib.rs', 42)",
            [],
        )
        .unwrap();
        let n = format_trigger(&conn, &thread_trigger("RT_1", Some("https://x/t"))).expect("fmt");
        assert_eq!(n.title, "Needs your attention");
        assert_eq!(n.body, "owner/web #42 - src/lib.rs:42");
        assert_eq!(n.payload["unit_kind"], "thread");
        assert_eq!(n.payload["unit_ref"], "RT_1");
        assert_eq!(n.payload["deep_link_url"], "https://x/t");
    }

    #[test]
    fn thread_body_falls_back_to_original_line_then_pr_title() {
        let conn = fresh_db();
        seed_pr(&conn, "owner", "web", 100, 42, "Add a thing");
        // No current `line`, only `original_line` (outdated thread keeps it).
        conn.execute(
            "INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, node_id, path, original_line)
                VALUES (200, 100, 0, 1, 'RT_o', 'src/old.rs', 7)",
            [],
        )
        .unwrap();
        let n = format_trigger(&conn, &thread_trigger("RT_o", None)).expect("fmt");
        assert_eq!(n.body, "owner/web #42 - src/old.rs:7");

        // A thread with no path falls back to the PR title.
        conn.execute(
            "INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, node_id)
                VALUES (201, 100, 0, 0, 'RT_np')",
            [],
        )
        .unwrap();
        let n = format_trigger(&conn, &thread_trigger("RT_np", None)).expect("fmt");
        assert_eq!(n.body, "owner/web #42 - Add a thing");
    }

    #[test]
    fn general_stream_body_is_general_discussion() {
        let conn = fresh_db();
        seed_pr(&conn, "owner", "web", 100, 42, "Add a thing");
        let url = "https://github.com/owner/web/pull/42";
        let n = format_trigger(&conn, &general_trigger(Some(url))).expect("fmt");
        assert_eq!(n.title, "Needs your attention");
        assert_eq!(n.body, "owner/web #42 - General discussion");
        assert_eq!(n.payload["unit_kind"], "general");
        assert!(n.payload["unit_ref"].is_null());
        assert_eq!(n.payload["deep_link_url"], url);
    }

    #[test]
    fn snapshot_carries_unit_fields() {
        let conn = fresh_db();
        seed_pr(&conn, "owner", "web", 100, 42, "Add a thing");
        conn.execute(
            "INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, node_id, path, line)
                VALUES (200, 100, 0, 0, 'RT_1', 'src/lib.rs', 42)",
            [],
        )
        .unwrap();
        let n = format_trigger(&conn, &thread_trigger("RT_1", Some("https://x/t"))).expect("fmt");
        let snap = n.snapshot.expect("snapshot");
        assert_eq!(snap.unit_kind, Some(NotificationUnitKind::Thread));
        assert_eq!(snap.unit_ref.as_deref(), Some("RT_1"));
        assert_eq!(snap.deep_link_url.as_deref(), Some("https://x/t"));
    }

    #[test]
    fn review_request_copy_is_review_requested() {
        let conn = fresh_db();
        seed_pr(&conn, "owner", "web", 100, 42, "Add a thing");
        let url = "https://github.com/owner/web/pull/42";
        let n = format_trigger(
            &conn,
            &role_trigger(NotificationUnitKind::ReviewRequest, Some(url)),
        )
        .expect("fmt");
        assert_eq!(n.title, "Review requested");
        assert_eq!(n.body, "You've been asked to review owner/web #42");
        assert_eq!(n.payload["unit_kind"], "review_request");
        assert!(n.payload["unit_ref"].is_null());
        assert_eq!(n.payload["deep_link_url"], url);
        let snap = n.snapshot.expect("snapshot");
        assert_eq!(snap.unit_kind, Some(NotificationUnitKind::ReviewRequest));
        assert_eq!(snap.unit_ref, None);
    }

    #[test]
    fn changes_requested_copy_is_changes_requested() {
        let conn = fresh_db();
        seed_pr(&conn, "owner", "web", 100, 42, "Add a thing");
        let url = "https://github.com/owner/web/pull/42";
        let n = format_trigger(
            &conn,
            &role_trigger(NotificationUnitKind::ChangesRequested, Some(url)),
        )
        .expect("fmt");
        assert_eq!(n.title, "Changes requested");
        assert_eq!(n.body, "Changes were requested on your owner/web #42");
        assert_eq!(n.payload["unit_kind"], "changes_requested");
    }

    #[test]
    fn returns_none_when_pr_missing() {
        let conn = fresh_db();
        // No PR rows seeded.
        assert!(format_trigger(&conn, &general_trigger(None)).is_none());
    }
}
