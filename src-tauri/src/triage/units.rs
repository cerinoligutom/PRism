//! Per-conversation-unit helpers for the ADR 0031 attention model.
//!
//! A *conversation unit* is either a review thread (keyed on its GraphQL
//! `node_id`) or the PR's general comment stream (one dismissible unit per
//! PR). Two concerns live here:
//!
//! * **Seen-watermark writers** ([`advance_thread_seen`],
//!   [`advance_general_stream_seen`]) - MAX-only upserts that record an
//!   explicit "mark seen" act. They never move a watermark backwards, so a
//!   later cycle that re-reads an older `seen_at` can't un-clear a unit.
//! * **The per-row "needs me" predicate** ([`row_unit_needs_me_predicate`]) -
//!   the same host-aware unit shape the roll-up's (A)/(B) branches use
//!   ([`crate::triage::query::needs_attention_case_expr`]), but gated to one
//!   unit and correlated to a `notifications` row `n`. The derived inbox uses
//!   it so a live row is unread iff *its own* unit still needs me, not the
//!   whole PR roll-up (ADR 0031 inbox decision: per-row, not roll-up).
//!
//! Keeping the predicate as a string fragment - rather than a second copy of
//! the EXISTS shape - means the inbox and the roll-up cannot drift apart on
//! what "this unit needs me" means.

use rusqlite::{params, Connection};

/// Advance the per-thread explicit seen watermark to at least `seen_at`.
///
/// MAX-only: an INSERT that conflicts on the `(account_id,
/// review_thread_node_id)` key keeps whichever `seen_at` is larger, so a
/// re-mark with a stale clock (or a re-run over the same node) never moves the
/// watermark backwards. Keyed on the GraphQL `node_id` so it survives a
/// transient delete+re-add of the `review_threads` row during a paginated
/// fetch (ADR 0031 schema note).
pub fn advance_thread_seen(
    conn: &Connection,
    account_id: i64,
    review_thread_node_id: &str,
    seen_at: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO thread_read_state (account_id, review_thread_node_id, seen_at)
            VALUES (?1, ?2, ?3)
         ON CONFLICT(account_id, review_thread_node_id) DO UPDATE SET
            seen_at = MAX(seen_at, excluded.seen_at)",
        params![account_id, review_thread_node_id, seen_at],
    )?;
    Ok(())
}

/// Advance the per-PR general-stream seen watermark to at least `seen_at`.
///
/// MAX-only via `COALESCE(...)` so a NULL column (never marked) or a smaller
/// stored value both yield `seen_at`, while a larger stored value is kept.
/// No-op when the relation row is missing (the conversation surface marks
/// units seen against rows the viewer already has a relation to; a Team-view
/// PR with no relation row has no general-stream watermark to advance).
pub fn advance_general_stream_seen(
    conn: &Connection,
    account_id: i64,
    pull_request_id: i64,
    seen_at: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET general_stream_seen_at = MAX(COALESCE(general_stream_seen_at, 0), ?3)
          WHERE account_id = ?1 AND pull_request_id = ?2",
        params![account_id, pull_request_id, seen_at],
    )?;
    Ok(())
}

/// Advance the per-PR reviews-stream seen watermark to at least `seen_at`
/// (ADR 0033). Peer to [`advance_general_stream_seen`] for the reviews unit:
/// the reviews unit (branch E) clears when this advances past the newest
/// mentioning review. A mention-only unit, so there is no "my own comment"
/// component - the watermark is `reviews_seen_at` alone.
///
/// MAX-only via `COALESCE(...)` so a NULL column (never marked) or a smaller
/// stored value both yield `seen_at`, while a larger stored value is kept.
/// No-op when the relation row is missing, matching
/// [`advance_general_stream_seen`].
pub fn advance_reviews_seen(
    conn: &Connection,
    account_id: i64,
    pull_request_id: i64,
    seen_at: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET reviews_seen_at = MAX(COALESCE(reviews_seen_at, 0), ?3)
          WHERE account_id = ?1 AND pull_request_id = ?2",
        params![account_id, pull_request_id, seen_at],
    )?;
    Ok(())
}

/// Advance the PR-level read watermark so the dashboard's `unread` derivation
/// (`read_at IS NULL OR pull_requests.updated_at > read_pr_updated_at`) clears
/// when the viewer opens the PR. This is the "I have opened the latest of this
/// PR" axis (the bold-title signal), distinct from the per-unit "needs me"
/// attention dot: opening a PR clears both - the units via the seen watermarks
/// above, the unread via this. `read_pr_updated_at` snapshots the PR's current
/// `updated_at` so a later update re-flags the row unread.
///
/// UPDATE-only (no UPSERT) so it is a no-op on a missing relation row, matching
/// [`advance_general_stream_seen`]: opening a Team-view PR the viewer has no
/// relation to must not manufacture one.
pub fn advance_read_watermark(
    conn: &Connection,
    account_id: i64,
    pull_request_id: i64,
    read_at: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET read_at = ?3,
                read_pr_updated_at = (
                    SELECT updated_at FROM pull_requests WHERE id = ?2
                )
          WHERE account_id = ?1 AND pull_request_id = ?2",
        params![account_id, pull_request_id, read_at],
    )?;
    Ok(())
}

/// The per-row "does this notification's own unit still need me" predicate, as
/// a `CASE WHEN ... THEN 1 ELSE 0 END` SQL expression correlated to a
/// `notifications n` row. Resolves `n.unit_kind` / `n.unit_ref` against the
/// same per-unit watermark shape the roll-up uses, host-gated.
///
/// Result `1` (unread) iff ANY of:
///
/// - `n.unit_kind = 'thread'`: the review thread whose `node_id = n.unit_ref`
///   on `n.pull_request_id` is involved-and-lit for `n.account_id` - I authored
///   the PR, OR I have a comment in the thread, OR a comment has
///   `mentions_viewer = 1`; AND an other-authored comment in that thread is
///   newer than `MAX(thread_read_state.seen_at, my latest comment.created_at)`.
/// - `n.unit_kind = 'general'`: the same shape over `n.pull_request_id`'s
///   `issue_comments`, watermark `MAX(rel.general_stream_seen_at, my latest
///   issue_comment.created_at)`.
/// - `n.unit_kind = 'review_request'`: the viewer is in `requested_reviewers`
///   for `n.pull_request_id` AND the request is newer than the open watermark
///   (`requested_at > read_at`, host-gated, roll-up branch C). ADR 0033: the
///   obligation clears on PR open (read_at advances past it) or from GitHub
///   state (submitting drops the viewer); a fresh request re-arms it.
/// - `n.unit_kind = 'changes_requested'`: the PR has `review_decision =
///   'CHANGES_REQUESTED'` and `author_login = viewer.login`, and the blocking
///   review's `submitted_at` is newer than the open watermark (host-gated,
///   roll-up branch D). ADR 0033: clears on open, or when the decision flips.
/// - `n.unit_kind = 'review'`: a formal review by someone else whose body
///   `@`-mentions the viewer, newer than `rel.reviews_seen_at` (host-gated,
///   roll-up branch E). Clears when the reviews stream is marked seen.
///
/// `0` otherwise (the unit settled, the obligation cleared, or the kind/ref
/// don't resolve). A row with `unit_kind IS NULL` is NOT handled here - the
/// caller falls back to `read_at` for legacy rows. Host isolation matches the
/// roll-up: every branch joins the PR's owning host (`repos -> accounts`) and
/// requires it to equal the viewer's host before a login-string match counts
/// (issue #169, ADR 0031).
///
/// References only `n.account_id`, `n.pull_request_id`, `n.unit_kind`, and
/// `n.unit_ref`, so a caller can embed it under any `notifications n` scope.
pub(crate) fn row_unit_needs_me_predicate() -> &'static str {
    "CASE WHEN (
        (n.unit_kind = 'thread' AND EXISTS (
            SELECT 1
              FROM review_threads t
              JOIN pull_requests pr ON pr.id = t.pull_request_id
              JOIN accounts viewer ON viewer.id = n.account_id
              JOIN repos r ON r.id = pr.repo_id
              JOIN accounts pr_host_acc ON pr_host_acc.id = r.account_id
             WHERE t.pull_request_id = n.pull_request_id
               AND t.node_id = n.unit_ref
               AND pr_host_acc.host = viewer.host
               AND (
                   pr.author_login = viewer.login
                   OR EXISTS (
                       SELECT 1 FROM review_comments c
                        WHERE c.review_thread_id = t.id
                          AND c.author_login = viewer.login
                   )
                   OR EXISTS (
                       SELECT 1 FROM review_comments c
                        WHERE c.review_thread_id = t.id
                          AND c.mentions_viewer = 1
                   )
               )
               AND EXISTS (
                   SELECT 1 FROM review_comments c
                    WHERE c.review_thread_id = t.id
                      AND c.author_login <> viewer.login
                      AND c.created_at > (
                          SELECT MAX(w) FROM (
                              SELECT COALESCE((
                                  SELECT trs.seen_at FROM thread_read_state trs
                                   WHERE trs.account_id = n.account_id
                                     AND trs.review_thread_node_id = t.node_id
                              ), 0) AS w
                              UNION ALL
                              SELECT COALESCE((
                                  SELECT MAX(mc.created_at) FROM review_comments mc
                                   WHERE mc.review_thread_id = t.id
                                     AND mc.author_login = viewer.login
                              ), 0) AS w
                          )
                      )
               )
        ))
        OR (n.unit_kind = 'general' AND EXISTS (
            SELECT 1
              FROM pull_requests pr
              JOIN accounts viewer ON viewer.id = n.account_id
              JOIN repos r ON r.id = pr.repo_id
              JOIN accounts pr_host_acc ON pr_host_acc.id = r.account_id
              JOIN pull_request_viewer_relations rel
                ON rel.account_id = n.account_id
               AND rel.pull_request_id = pr.id
             WHERE pr.id = n.pull_request_id
               AND pr_host_acc.host = viewer.host
               AND (
                   pr.author_login = viewer.login
                   OR EXISTS (
                       SELECT 1 FROM issue_comments ic
                        WHERE ic.pull_request_id = pr.id
                          AND ic.author_login = viewer.login
                   )
                   OR EXISTS (
                       SELECT 1 FROM issue_comments ic
                        WHERE ic.pull_request_id = pr.id
                          AND ic.mentions_viewer = 1
                   )
               )
               AND EXISTS (
                   SELECT 1 FROM issue_comments ic
                    WHERE ic.pull_request_id = pr.id
                      AND ic.author_login <> viewer.login
                      AND ic.created_at > (
                          SELECT MAX(w) FROM (
                              SELECT COALESCE(rel.general_stream_seen_at, 0) AS w
                              UNION ALL
                              SELECT COALESCE((
                                  SELECT MAX(mic.created_at) FROM issue_comments mic
                                   WHERE mic.pull_request_id = pr.id
                                     AND mic.author_login = viewer.login
                              ), 0) AS w
                          )
                      )
               )
        ))
        OR (n.unit_kind = 'review_request' AND EXISTS (
            SELECT 1
              FROM requested_reviewers rr
              JOIN pull_requests pr ON pr.id = rr.pull_request_id
              JOIN accounts viewer ON viewer.id = n.account_id
              JOIN repos r ON r.id = pr.repo_id
              JOIN accounts pr_host_acc ON pr_host_acc.id = r.account_id
              JOIN pull_request_viewer_relations rel
                ON rel.account_id = n.account_id AND rel.pull_request_id = pr.id
             WHERE rr.pull_request_id = n.pull_request_id
               AND rr.login = viewer.login
               AND pr_host_acc.host = viewer.host
               AND COALESCE(rr.requested_at, 0) > COALESCE(rel.read_at, 0)
        ))
        OR (n.unit_kind = 'changes_requested' AND EXISTS (
            SELECT 1
              FROM pull_requests pr
              JOIN accounts viewer ON viewer.id = n.account_id
              JOIN repos r ON r.id = pr.repo_id
              JOIN accounts pr_host_acc ON pr_host_acc.id = r.account_id
              JOIN pull_request_viewer_relations rel
                ON rel.account_id = n.account_id AND rel.pull_request_id = pr.id
             WHERE pr.id = n.pull_request_id
               AND pr.author_login = viewer.login
               AND pr_host_acc.host = viewer.host
               AND pr.review_decision = 'CHANGES_REQUESTED'
               AND COALESCE((
                   SELECT MAX(rv.submitted_at) FROM reviews rv
                    WHERE rv.pull_request_id = pr.id
                      AND rv.state = 'CHANGES_REQUESTED'
               ), 0) > COALESCE(rel.read_at, 0)
        ))
        OR (n.unit_kind = 'review' AND EXISTS (
            SELECT 1
              FROM reviews rv
              JOIN pull_requests pr ON pr.id = rv.pull_request_id
              JOIN accounts viewer ON viewer.id = n.account_id
              JOIN repos r ON r.id = pr.repo_id
              JOIN accounts pr_host_acc ON pr_host_acc.id = r.account_id
              JOIN pull_request_viewer_relations rel
                ON rel.account_id = n.account_id AND rel.pull_request_id = pr.id
             WHERE rv.pull_request_id = n.pull_request_id
               AND pr_host_acc.host = viewer.host
               AND rv.reviewer_login <> viewer.login
               AND rv.mentions_viewer = 1
               AND COALESCE(rv.submitted_at, 0) > COALESCE(rel.reviews_seen_at, 0)
        ))
    ) THEN 1 ELSE 0 END"
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> Connection {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        conn
    }

    fn read_thread_seen(conn: &Connection, account_id: i64, node_id: &str) -> Option<i64> {
        conn.query_row(
            "SELECT seen_at FROM thread_read_state
              WHERE account_id = ?1 AND review_thread_node_id = ?2",
            params![account_id, node_id],
            |r| r.get::<_, i64>(0),
        )
        .ok()
    }

    #[test]
    fn advance_thread_seen_inserts_then_keeps_the_max() {
        let conn = fresh();
        conn.execute(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0)",
            [],
        )
        .unwrap();

        advance_thread_seen(&conn, 1, "RT_1", 100).unwrap();
        assert_eq!(read_thread_seen(&conn, 1, "RT_1"), Some(100));

        // A later mark advances.
        advance_thread_seen(&conn, 1, "RT_1", 200).unwrap();
        assert_eq!(read_thread_seen(&conn, 1, "RT_1"), Some(200));

        // A stale mark is ignored (MAX-only).
        advance_thread_seen(&conn, 1, "RT_1", 50).unwrap();
        assert_eq!(read_thread_seen(&conn, 1, "RT_1"), Some(200));
    }

    #[test]
    fn advance_thread_seen_survives_node_delete_and_readd() {
        // node_id keying: the watermark lives on the node, not the
        // review_threads row id. Deleting and re-adding the row (a paginated
        // fetch can do this) leaves the seen watermark intact and still
        // applicable.
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'o', 'r', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 'me', 0, 0, 'main', 'feat');
             INSERT INTO review_threads (id, pull_request_id, is_resolved, node_id)
                VALUES (500, 100, 0, 'RT_keep');",
        )
        .unwrap();

        advance_thread_seen(&conn, 1, "RT_keep", 150).unwrap();

        // Delete + re-add the node under a new row id.
        conn.execute("DELETE FROM review_threads WHERE id = 500", [])
            .unwrap();
        conn.execute(
            "INSERT INTO review_threads (id, pull_request_id, is_resolved, node_id)
                VALUES (501, 100, 0, 'RT_keep')",
            [],
        )
        .unwrap();

        assert_eq!(
            read_thread_seen(&conn, 1, "RT_keep"),
            Some(150),
            "seen watermark keyed on node_id survives a row delete+re-add"
        );
    }

    #[test]
    fn advance_general_stream_seen_is_max_only_and_skips_missing_row() {
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'o', 'r', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 'me', 0, 0, 'main', 'feat');",
        )
        .unwrap();

        // No relation row yet: the UPDATE is a clean no-op.
        advance_general_stream_seen(&conn, 1, 100, 100).unwrap();

        conn.execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at)
                VALUES (1, 100, 0)",
            [],
        )
        .unwrap();

        advance_general_stream_seen(&conn, 1, 100, 100).unwrap();
        let read = |conn: &Connection| -> Option<i64> {
            conn.query_row(
                "SELECT general_stream_seen_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |r| r.get::<_, Option<i64>>(0),
            )
            .unwrap()
        };
        assert_eq!(read(&conn), Some(100));

        advance_general_stream_seen(&conn, 1, 100, 250).unwrap();
        assert_eq!(read(&conn), Some(250));

        advance_general_stream_seen(&conn, 1, 100, 90).unwrap();
        assert_eq!(read(&conn), Some(250), "MAX-only: stale mark ignored");
    }

    #[test]
    fn advance_reviews_seen_is_max_only_and_skips_missing_row() {
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'o', 'r', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 'me', 0, 0, 'main', 'feat');",
        )
        .unwrap();

        // No relation row yet: the UPDATE is a clean no-op.
        advance_reviews_seen(&conn, 1, 100, 100).unwrap();

        conn.execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at)
                VALUES (1, 100, 0)",
            [],
        )
        .unwrap();

        let read = |conn: &Connection| -> Option<i64> {
            conn.query_row(
                "SELECT reviews_seen_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |r| r.get::<_, Option<i64>>(0),
            )
            .unwrap()
        };

        advance_reviews_seen(&conn, 1, 100, 100).unwrap();
        assert_eq!(read(&conn), Some(100));

        advance_reviews_seen(&conn, 1, 100, 250).unwrap();
        assert_eq!(read(&conn), Some(250));

        advance_reviews_seen(&conn, 1, 100, 90).unwrap();
        assert_eq!(read(&conn), Some(250), "MAX-only: stale mark ignored");
    }

    #[test]
    fn advance_read_watermark_sets_read_at_snapshots_updated_at_and_skips_missing_row() {
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'o', 'r', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 'me', 0, 500, 'main', 'feat');",
        )
        .unwrap();

        // No relation row yet: the UPDATE is a clean no-op and manufactures
        // nothing (matches the general-stream / Team-view PR semantic).
        advance_read_watermark(&conn, 1, 100, 999).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "no relation row is manufactured on a missing row");

        conn.execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at)
                VALUES (1, 100, 0)",
            [],
        )
        .unwrap();

        advance_read_watermark(&conn, 1, 100, 999).unwrap();
        let (read_at, read_pr_updated_at): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT read_at, read_pr_updated_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(read_at, Some(999), "read_at set to the passed clock");
        assert_eq!(
            read_pr_updated_at,
            Some(500),
            "read_pr_updated_at snapshots the PR's updated_at so a later update re-flags unread",
        );
    }
}
