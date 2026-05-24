//! Integration tests for the mention scan + needs_attention recompute.
//! Exercises [`super::scan_mentions_and_recompute_attention`] through the
//! enrichment write path so the per-cycle transaction guarantees hold.

use rusqlite::{params, Connection};
use std::sync::{Arc, Mutex};

use super::super::enrichment::write_pr_updates;
use crate::db::DbHandle;

fn seed_db_with_pr() -> (DbHandle, i64, i64) {
    let mut conn = Connection::open_in_memory().expect("in-memory db");
    crate::db::migrate::run(&mut conn).expect("migrations");
    conn.execute_batch(
        "INSERT INTO accounts (id, label, host, login, created_at)
            VALUES (1, 'a', 'github.com', 'me', 0);
         INSERT INTO repos (id, account_id, owner, name, visibility)
            VALUES (10, 1, 'owner', 'repo', 'public');
         INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref)
            VALUES (100, 10, 42, 'placeholder', 'open', 0, '', 0, 0, 'main', 'feat');",
    )
    .unwrap();
    (Arc::new(Mutex::new(conn)), 10, 100)
}

fn seed_relation(db: &DbHandle, account_id: i64, pr_id: i64) {
    db.lock()
        .unwrap()
        .execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at, mentioned_count_unread,
                 mention_scan_watermark_at, needs_attention)
                VALUES (?1, ?2, 0, 0, 0, 0, 0, 0, 0)",
            params![account_id, pr_id],
        )
        .unwrap();
}

fn read_mention_count(db: &DbHandle, account_id: i64, pr_id: i64) -> i64 {
    db.lock()
        .unwrap()
        .query_row(
            "SELECT mentioned_count_unread FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
}

fn read_watermark(db: &DbHandle, account_id: i64, pr_id: i64) -> i64 {
    db.lock()
        .unwrap()
        .query_row(
            "SELECT mention_scan_watermark_at FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
}

fn read_needs_attention(db: &DbHandle, account_id: i64, pr_id: i64) -> i64 {
    db.lock()
        .unwrap()
        .query_row(
            "SELECT needs_attention FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
}

// --- write_pr_updates scan integration tests ---

#[test]
fn mention_scan_counts_new_review_comment_mentions() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, node_id)
                VALUES (1001, 100, 0, 0, 'RT_m');
             INSERT INTO review_comments
                (id, review_thread_id, author_login, body, created_at)
                VALUES
                (2001, 1001, 'bob',   'hey @me what do you think', 10),
                (2002, 1001, 'carol', 'and @me again',             20);",
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(read_mention_count(&db, 1, pr_id), 2);
}

#[test]
fn mention_scan_counts_new_issue_comment_mentions() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
                VALUES
                (3001, 100, 'bob',   'looks good @me',             10),
                (3002, 100, 'carol', 'one more nit, @me, then go', 20);",
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(read_mention_count(&db, 1, pr_id), 2);
}

#[test]
fn mention_scan_ignores_viewers_own_comments() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
                VALUES
                (3001, 100, 'me',   'I am @me writing about myself', 10),
                (3002, 100, 'me',   'also @me here',                 20);",
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(
        read_mention_count(&db, 1, pr_id),
        0,
        "viewer's own comments must never increment the counter"
    );
}

#[test]
fn mention_scan_ignores_mentions_of_other_logins() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
                VALUES
                (3001, 100, 'bob',   '@alice please look',       10),
                (3002, 100, 'carol', '@dave can you take this?', 20);",
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(read_mention_count(&db, 1, pr_id), 0);
}

#[test]
fn mention_scan_word_boundary_rejects_subword_match() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
                VALUES
                (3001, 100, 'bob', 'pinging @me-bot for CI',     10),
                (3002, 100, 'bob', 'and @mester is on holiday',  20),
                (3003, 100, 'bob', 'true mention: @me now',      30);",
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(
        read_mention_count(&db, 1, pr_id),
        1,
        "only the bare @me row counts"
    );
}

#[test]
fn mention_scan_is_idempotent_across_cycles() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
                VALUES (3001, 100, 'bob', 'hi @me', 10);",
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
    let first = read_mention_count(&db, 1, pr_id);
    assert_eq!(first, 1);

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
    let second = read_mention_count(&db, 1, pr_id);
    assert_eq!(
        second, 1,
        "second cycle with no new comments must not re-count"
    );
}

#[test]
fn mention_scan_advances_watermark_even_without_new_mentions() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);

    // No comments at all.
    assert_eq!(read_watermark(&db, 1, pr_id), 0);

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    let watermark = read_watermark(&db, 1, pr_id);
    assert!(
        watermark > 0,
        "watermark must move forward every cycle (got {watermark})"
    );
}

#[test]
fn mention_scan_only_counts_comments_after_watermark() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);

    // Pin the watermark forward of the older comment so only the newer
    // one is counted on this cycle.
    db.lock()
        .unwrap()
        .execute(
            "UPDATE pull_request_viewer_relations
                SET mention_scan_watermark_at = 15
              WHERE account_id = 1 AND pull_request_id = ?1",
            params![pr_id],
        )
        .unwrap();
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
                VALUES
                (3001, 100, 'bob', 'older @me before watermark', 10),
                (3002, 100, 'bob', 'newer @me after  watermark', 20);",
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(
        read_mention_count(&db, 1, pr_id),
        1,
        "only the post-watermark comment should count"
    );
}

// --- needs_attention recompute tests (four signals, ADR 0015) ---

#[test]
fn needs_attention_stays_zero_when_no_signal_fires() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(read_needs_attention(&db, 1, pr_id), 0);
}

#[test]
fn needs_attention_fires_on_unresolved_thread_for_pr_author() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);

    // Make 'me' the PR author and add an unresolved + involved thread.
    db.lock()
        .unwrap()
        .execute_batch(
            "UPDATE pull_requests SET author_login = 'me' WHERE id = 100;
             INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, node_id)
                VALUES (1001, 100, 0, 0, 'RT_n');
             INSERT INTO review_comments
                (id, review_thread_id, author_login, body, created_at)
                VALUES (2001, 1001, 'me', 'reply', 10);",
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(read_needs_attention(&db, 1, pr_id), 1);
}

#[test]
fn needs_attention_fires_when_viewer_is_requested_reviewer() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);

    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                VALUES (100, 'me', 'user');",
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(read_needs_attention(&db, 1, pr_id), 1);
}

#[test]
fn needs_attention_fires_on_unread_mention() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);

    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
                VALUES (3001, 100, 'bob', 'hi @me', 10);",
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(read_needs_attention(&db, 1, pr_id), 1);
}

#[test]
fn needs_attention_fires_on_changes_requested_for_pr_author() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);

    db.lock()
        .unwrap()
        .execute(
            "UPDATE pull_requests
                SET author_login = 'me',
                    review_decision = 'CHANGES_REQUESTED'
              WHERE id = ?1",
            params![pr_id],
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(read_needs_attention(&db, 1, pr_id), 1);
}

#[test]
fn needs_attention_does_not_fire_on_changes_requested_for_other_author() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);

    // PR author is somebody else; CHANGES_REQUESTED on a PR you didn't
    // author isn't a signal for you.
    db.lock()
        .unwrap()
        .execute(
            "UPDATE pull_requests
                SET author_login = 'someone-else',
                    review_decision = 'CHANGES_REQUESTED'
              WHERE id = ?1",
            params![pr_id],
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(read_needs_attention(&db, 1, pr_id), 0);
}

#[test]
fn needs_attention_clears_when_signal_disappears() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);

    // Cycle 1: fire on requested-reviewer signal.
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                VALUES (100, 'me', 'user');",
        )
        .unwrap();
    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
    assert_eq!(read_needs_attention(&db, 1, pr_id), 1);

    // Cycle 2: reviewer request rescinded.
    db.lock()
        .unwrap()
        .execute(
            "DELETE FROM requested_reviewers WHERE pull_request_id = ?1",
            params![pr_id],
        )
        .unwrap();
    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
    assert_eq!(
        read_needs_attention(&db, 1, pr_id),
        0,
        "removing the only signal must clear needs_attention"
    );
}

#[test]
fn sync_cycle_flips_needs_attention_via_new_mention() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    seed_relation(&db, 1, pr_id);

    // Cycle 1: no comments, no signals — flag stays 0.
    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
    assert_eq!(read_needs_attention(&db, 1, pr_id), 0);
    let watermark_after_first = read_watermark(&db, 1, pr_id);
    assert!(watermark_after_first > 0);

    // A new mention lands after the first cycle (created_at > watermark).
    db.lock()
        .unwrap()
        .execute(
            "INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
                VALUES (3001, ?1, 'bob', 'heads up @me', ?2)",
            params![pr_id, watermark_after_first + 60],
        )
        .unwrap();

    // Cycle 2 picks it up and flips the composite.
    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
    assert_eq!(read_mention_count(&db, 1, pr_id), 1);
    assert_eq!(read_needs_attention(&db, 1, pr_id), 1);
}

#[test]
fn mention_scan_is_a_noop_when_relation_row_missing() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    // Deliberately no `seed_relation` — Team-view path where this account
    // has no discovered relation to the PR.
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
                VALUES (3001, 100, 'bob', 'hi @me', 10);",
        )
        .unwrap();

    // Should not error even with no relation row to update.
    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    let count: i64 = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM pull_request_viewer_relations
              WHERE account_id = 1 AND pull_request_id = ?1",
            params![pr_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "missing relation row must remain missing");
}

// --- cross-host (login collision) isolation tests (issue #169) ---
//
// Two accounts share login `me` on different hosts. The PR is owned by
// account 1 (github.com). Without the host-aware joins the recompute
// would flag account 2 as the PR author / requested reviewer / etc.
// purely because the login string matches, even though account 2 lives
// on a different host and isn't the same identity.

/// Seed a fixture where the PR is owned by account 1 (github.com, login
/// `me`) and a second account on a different host shares the same login.
/// Both accounts get a relation row to the same PR so the scan + recompute
/// can run for either.
fn seed_db_with_cross_host_login_collision() -> (DbHandle, i64, i64) {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (2, 'ghe', 'github.acme.corp', 'me', 0);",
        )
        .unwrap();
    seed_relation(&db, 1, pr_id);
    seed_relation(&db, 2, pr_id);
    (db, repo_id, pr_id)
}

#[test]
fn needs_attention_does_not_fire_cross_host_for_pr_author_match() {
    let (db, repo_id, pr_id) = seed_db_with_cross_host_login_collision();

    // PR sits on github.com (account 1's host); author_login matches both
    // accounts' login string but the identity is only account 1's. Seed
    // an unresolved + involved thread via a `me`-authored comment so the
    // query-time involvement test would otherwise mark the thread as
    // involved for account 2 too (the EXISTS join is login-only). The
    // host-aware guard around signal #1 must reject the match.
    db.lock()
        .unwrap()
        .execute_batch(
            "UPDATE pull_requests SET author_login = 'me' WHERE id = 100;
             INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, node_id)
                VALUES (1001, 100, 0, 0, 'RT_x');
             INSERT INTO review_comments
                (id, review_thread_id, author_login, body, created_at)
                VALUES (2001, 1001, 'me', 'reply', 10);",
        )
        .unwrap();

    write_pr_updates(&db, 2, repo_id, pr_id, None, None).unwrap();

    assert_eq!(
        read_needs_attention(&db, 2, pr_id),
        0,
        "account 2 lives on a different host, so the login-only author \
         match must not flag its needs_attention"
    );
}

#[test]
fn needs_attention_does_not_fire_cross_host_for_requested_reviewer_match() {
    let (db, repo_id, pr_id) = seed_db_with_cross_host_login_collision();

    // Requested reviewer `me` on a github.com PR refers to the github.com
    // user, not account 2's ghe identity.
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                VALUES (100, 'me', 'user');",
        )
        .unwrap();

    write_pr_updates(&db, 2, repo_id, pr_id, None, None).unwrap();

    assert_eq!(
        read_needs_attention(&db, 2, pr_id),
        0,
        "the requested reviewer is on the PR's host; cross-host login \
         match must not flag account 2"
    );
}

#[test]
fn needs_attention_does_not_fire_cross_host_for_changes_requested() {
    let (db, repo_id, pr_id) = seed_db_with_cross_host_login_collision();

    db.lock()
        .unwrap()
        .execute(
            "UPDATE pull_requests
                SET author_login = 'me',
                    review_decision = 'CHANGES_REQUESTED'
              WHERE id = ?1",
            params![pr_id],
        )
        .unwrap();

    write_pr_updates(&db, 2, repo_id, pr_id, None, None).unwrap();

    assert_eq!(
        read_needs_attention(&db, 2, pr_id),
        0,
        "CHANGES_REQUESTED on a github.com PR doesn't make account 2 \
         (different host) the author"
    );
}

#[test]
fn needs_attention_still_fires_same_host_for_pr_author_match() {
    // Regression guard: the host-aware join must not break the matching
    // account's recompute. Same fixture, but check the account that IS
    // the PR author still gets needs_attention=1.
    let (db, repo_id, pr_id) = seed_db_with_cross_host_login_collision();

    db.lock()
        .unwrap()
        .execute_batch(
            "UPDATE pull_requests SET author_login = 'me' WHERE id = 100;
             INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, node_id)
                VALUES (1001, 100, 0, 0, 'RT_y');
             INSERT INTO review_comments
                (id, review_thread_id, author_login, body, created_at)
                VALUES (2001, 1001, 'me', 'reply', 10);",
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(
        read_needs_attention(&db, 1, pr_id),
        1,
        "account 1 IS the PR author (same host, same login) - must flag"
    );
}

#[test]
fn mention_scan_does_not_increment_cross_host_relation_row() {
    // The same `@me` mention applies to whichever identity matches the
    // PR's host. Account 2 (different host) must not see its mention
    // count climb when only the literal `@me` token matches its login
    // string.
    let (db, repo_id, pr_id) = seed_db_with_cross_host_login_collision();
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
                VALUES (3001, 100, 'bob', 'ping @me', 10);",
        )
        .unwrap();

    write_pr_updates(&db, 2, repo_id, pr_id, None, None).unwrap();

    assert_eq!(
        read_mention_count(&db, 2, pr_id),
        0,
        "cross-host account must not see the github.com mention"
    );
}

#[test]
fn mention_scan_still_increments_same_host_relation_row() {
    // Regression guard for the same fixture: the host-matching account
    // still gets the mention counted.
    let (db, repo_id, pr_id) = seed_db_with_cross_host_login_collision();
    db.lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
                VALUES (3001, 100, 'bob', 'ping @me', 10);",
        )
        .unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

    assert_eq!(read_mention_count(&db, 1, pr_id), 1);
}
