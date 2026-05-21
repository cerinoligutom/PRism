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

use rusqlite::{params, Connection, OptionalExtension};

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
            (account_id, pull_request_id, last_seen_at,
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

/// Recompute the `pull_request_viewer_relations.needs_attention` boolean for
/// one `(account_id, pull_request_id)` pair using the four ADR-0015 signals:
///
/// 1. Viewer authored the PR AND `threads_unresolved_involved > 0`.
/// 2. Viewer is in `requested_reviewers` for the PR (presence implies pending;
///    the table never stores submitted reviews - those flow through
///    `reviews`).
/// 3. `mentioned_count_unread > 0` for the (account, PR) pair.
/// 4. Viewer authored the PR AND `review_decision = 'CHANGES_REQUESTED'`.
///
/// The UPDATE is a no-op when the relation row doesn't exist for the pair
/// (Team-view PRs never get a row - see contract). Callers that need the row
/// present should UPSERT first.
pub fn recompute_needs_attention(
    conn: &Connection,
    account_id: i64,
    pull_request_id: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE pull_request_viewer_relations AS rel
            SET needs_attention = (
                SELECT CASE WHEN
                    EXISTS (
                        SELECT 1
                          FROM pull_requests pr
                          JOIN accounts a ON a.id = rel.account_id
                         WHERE pr.id = rel.pull_request_id
                           AND pr.author_login = a.login
                           AND pr.threads_unresolved_involved > 0
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
    Ok(())
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

    fn seed_account_repo_pr(
        conn: &Connection,
        viewer_login: &str,
        author_login: &str,
        threads_unresolved_involved: i64,
        review_decision: Option<&str>,
    ) {
        conn.execute_batch(&format!(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', '{viewer_login}', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, draft, author_login,
                 created_at, updated_at, base_ref, head_ref,
                 threads_unresolved_involved, review_decision)
                VALUES (100, 10, 1, 't', 'open', 0, '{author_login}',
                        0, 0, 'main', 'feat', {threads_unresolved_involved},
                        {review_decision_sql});
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, last_seen_at)
                VALUES (1, 100, 0, 0, 0, 0);",
            review_decision_sql = match review_decision {
                Some(s) => format!("'{s}'"),
                None => "NULL".to_string(),
            }
        ))
        .unwrap();
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
        seed_account_repo_pr(&conn, "alice", "alice", 1, None);
        recompute_needs_attention(&conn, 1, 100).unwrap();
        assert_eq!(read_needs_attention(&conn), 1);
    }

    #[test]
    fn signal_one_no_fire_when_not_author() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", 1, None);
        recompute_needs_attention(&conn, 1, 100).unwrap();
        assert_eq!(read_needs_attention(&conn), 0);
    }

    #[test]
    fn signal_two_pending_requested_reviewer() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", 0, None);
        conn.execute(
            "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                VALUES (100, 'alice', 'user')",
            [],
        )
        .unwrap();
        recompute_needs_attention(&conn, 1, 100).unwrap();
        assert_eq!(read_needs_attention(&conn), 1);
    }

    #[test]
    fn signal_two_no_fire_for_other_reviewers() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", 0, None);
        conn.execute(
            "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                VALUES (100, 'carol', 'user')",
            [],
        )
        .unwrap();
        recompute_needs_attention(&conn, 1, 100).unwrap();
        assert_eq!(read_needs_attention(&conn), 0);
    }

    #[test]
    fn signal_three_unread_mentions() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", 0, None);
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET mentioned_count_unread = 2
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();
        recompute_needs_attention(&conn, 1, 100).unwrap();
        assert_eq!(read_needs_attention(&conn), 1);
    }

    #[test]
    fn signal_four_changes_requested_on_authored_pr() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "alice", 0, Some("CHANGES_REQUESTED"));
        recompute_needs_attention(&conn, 1, 100).unwrap();
        assert_eq!(read_needs_attention(&conn), 1);
    }

    #[test]
    fn signal_four_no_fire_when_not_author() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", 0, Some("CHANGES_REQUESTED"));
        recompute_needs_attention(&conn, 1, 100).unwrap();
        assert_eq!(read_needs_attention(&conn), 0);
    }

    #[test]
    fn negative_no_signals_clears_flag() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "bob", 0, None);
        // Pre-set the flag so the recompute has to actively clear it.
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET needs_attention = 1
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();
        recompute_needs_attention(&conn, 1, 100).unwrap();
        assert_eq!(read_needs_attention(&conn), 0);
    }

    #[test]
    fn combined_signals_one_and_three_still_fire() {
        let conn = fresh_db();
        seed_account_repo_pr(&conn, "alice", "alice", 1, None);
        conn.execute(
            "UPDATE pull_request_viewer_relations
                SET mentioned_count_unread = 3
              WHERE account_id = 1 AND pull_request_id = 100",
            [],
        )
        .unwrap();
        recompute_needs_attention(&conn, 1, 100).unwrap();
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
                (id, repo_id, number, title, state, draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 0, 'alice', 0, 0, 'main', 'feat');",
        )
        .unwrap();
        // No relations row for (1, 100) - the UPDATE should still succeed
        // and touch zero rows.
        recompute_needs_attention(&conn, 1, 100).unwrap();
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
}
