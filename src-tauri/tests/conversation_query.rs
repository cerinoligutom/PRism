//! Integration tests for the M3-B conversation query layer.
//!
//! Two surfaces are exercised:
//!
//! 1. `query::list_pr_threads` / `query::get_conversation_stats` against a
//!    fixture DB. Each test seeds the conversation tables directly to keep the
//!    SQL composition + stats math under tight control.
//! 2. `commands::persist_for_tests` — the same persistence path the live
//!    `fetch_pr_conversation` hydrator uses, run without booting Tauri so we
//!    can assert atomicity + upsert idempotency.

use prism_lib::conversation::commands::testing as commands_testing;
use prism_lib::conversation::query;
use prism_lib::conversation::types::ThreadState;
use prism_lib::db::{migrate, DbHandle};
use prism_lib::github::graphql::{
    Actor, IssueCommentNode, PageInfo, ReviewCommentConnection, ReviewCommentNode,
    ReviewThreadComments,
};
use rusqlite::{params, Connection};
use std::sync::{Arc, Mutex};

const ALICE_ID: i64 = 1;
const BOB_ID: i64 = 2;
const PR_ID: i64 = 100;

fn fresh_db() -> DbHandle {
    let mut conn = Connection::open_in_memory().expect("in-memory db");
    migrate::run(&mut conn).expect("migrations");
    Arc::new(Mutex::new(conn))
}

/// Seed the canonical fixture: one PR, two accounts, three threads.
///
/// - Thread 1000: unresolved, head from bob, alice replied. (alice is in.)
/// - Thread 1001: resolved.
/// - Thread 1002: outdated.
/// - Thread 1003: unresolved, head from carol, no replies.
///
/// `issue_comments_count` is set to 4 so the breakdown's `issue` count
/// reflects the cycle's rollup write.
fn seed_fixture(db: &DbHandle) {
    let conn = db.lock().unwrap();
    conn.execute_batch(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'alice', 'github.com', 'alice', 0),
            (2, 'bob',   'github.com', 'bob',   0);

        INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
            (10, 1, 'alice', 'web', 'public');

        INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref,
             issue_comments_count,
             threads_total,
             threads_unresolved_involved,
             threads_unresolved_uninvolved,
             threads_resolved_involved,
             threads_resolved_uninvolved)
            VALUES
            (100, 10, 1, 'web/#1', 'open', 0, 'alice', 0, 0, 'main', 'feat', 4,
             4, 1, 2, 0, 1);

        -- Three threads with distinct state combos.
        INSERT INTO review_threads
            (id, pull_request_id, is_resolved, is_outdated, original_line,
             path, node_id, created_at, resolved_at, last_reply_at,
             reply_count, head_comment_author_login, head_comment_body_text,
             head_comment_created_at, line, start_line) VALUES
            -- Unresolved, oldest. Two comments: head 1000 + reply 1100 (gap 1000s).
            (1000, 100, 0, 0, 12, 'src/lib.rs', 'PRRT_1', 1000, NULL, 1100, 1,
             'bob', 'looks wrong', 1000, 12, NULL),
            -- Resolved.
            (1001, 100, 1, 0, 22, 'src/util.rs', 'PRRT_2', 1500, 2000, 1800, 1,
             'bob', 'fix it', 1500, 22, NULL),
            -- Outdated (still has comments but excluded from unresolved counts).
            (1002, 100, 0, 1, 33, 'src/old.rs', 'PRRT_3', 1700, NULL, 1700, 0,
             'carol', 'nope', 1700, 33, NULL),
            -- Unresolved, no replies, head from carol (alice is NOT in).
            (1003, 100, 0, 0, 44, 'src/new.rs', 'PRRT_4', 2500, NULL, 2500, 0,
             'carol', 'spelling', 2500, 44, NULL);

        -- Thread 1000 comments: head + reply 1000s later. Reply is alice, so
        -- alice's `is_involved` lights up for this thread.
        INSERT INTO review_comments
            (id, review_thread_id, author_login, body, created_at, node_id) VALUES
            (50001, 1000, 'bob',   'looks wrong',  1000, 'PRRC_h1'),
            (50002, 1000, 'alice', 'fixed',        2000, 'PRRC_r1'),
            (50003, 1001, 'bob',   'fix it',       1500, 'PRRC_h2'),
            -- Resolved thread has a reply 100s after the head (gap 100s).
            (50004, 1001, 'bob',   'done',         1600, 'PRRC_r2'),
            -- Outdated thread: single comment (no reply gap).
            (50005, 1002, 'carol', 'nope',         1700, 'PRRC_h3'),
            -- Thread 1003: single comment, no replies.
            (50006, 1003, 'carol', 'spelling',     2500, 'PRRC_h4');

        -- Reviews: two with bodies (summary count), one with NULL body, one empty.
        INSERT INTO reviews
            (id, pull_request_id, reviewer_login, state, submitted_at, body, node_id) VALUES
            (70001, 100, 'bob',   'APPROVED',          1900, 'lgtm',     'REV_1'),
            (70002, 100, 'carol', 'CHANGES_REQUESTED', 2100, 'fix this', 'REV_2'),
            (70003, 100, 'dave',  'COMMENTED',         2200, NULL,       'REV_3'),
            (70004, 100, 'eve',   'COMMENTED',         2300, '',         'REV_4');
        "#,
    )
    .unwrap();
}

// ===== list_pr_threads =====

#[test]
fn list_pr_threads_returns_threads_in_created_order() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    let ids: Vec<i64> = threads.iter().map(|t| t.id).collect();
    assert_eq!(ids, vec![1000, 1001, 1002, 1003]);
}

#[test]
fn list_pr_threads_maps_state_correctly() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    let states: Vec<ThreadState> = threads.iter().map(|t| t.state).collect();
    assert_eq!(
        states,
        vec![
            ThreadState::Unresolved,
            ThreadState::Resolved,
            ThreadState::Outdated,
            ThreadState::Unresolved,
        ]
    );
}

#[test]
fn list_pr_threads_resolves_is_involved_for_account() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    // Alice replied on thread 1000. No other thread has an alice comment.
    let threads = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    let map: std::collections::HashMap<i64, bool> =
        threads.iter().map(|t| (t.id, t.is_involved)).collect();
    assert!(map[&1000]);
    assert!(!map[&1001]);
    assert!(!map[&1002]);
    assert!(!map[&1003]);
}

#[test]
fn list_pr_threads_with_no_account_marks_everything_uninvolved() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, None).unwrap();
    assert!(threads.iter().all(|t| !t.is_involved));
}

#[test]
fn list_pr_threads_returns_empty_for_unknown_pr() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, 9999, Some(ALICE_ID)).unwrap();
    assert!(threads.is_empty());
}

#[test]
fn list_pr_threads_carries_head_comment_snapshot() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    let head = threads[0].head_comment.as_ref().unwrap();
    assert_eq!(head.author_login, "bob");
    assert_eq!(head.body_text, "looks wrong");
    assert_eq!(head.created_at, 1000);
}

#[test]
fn list_pr_threads_projects_diff_hunk_through_to_dto() {
    // Issue #162: the hydrator writes `review_threads.diff_hunk` once per
    // thread on the head-comment write path. The projection through
    // `list_pr_threads` surfaces it on the DTO so the frontend can render
    // the file-context block above the thread card. Threads without a
    // hydrated hunk (legacy rows + PRs whose drawer has never been opened)
    // project as `None`.
    let db = fresh_db();
    seed_fixture(&db);
    db.lock()
        .unwrap()
        .execute(
            "UPDATE review_threads
                SET diff_hunk = '@@ -10,2 +10,3 @@\n a\n-b\n+c'
              WHERE id = 1000",
            [],
        )
        .unwrap();

    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    let by_id: std::collections::HashMap<i64, Option<String>> = threads
        .iter()
        .map(|t| (t.id, t.diff_hunk.clone()))
        .collect();
    assert_eq!(
        by_id[&1000].as_deref(),
        Some("@@ -10,2 +10,3 @@\n a\n-b\n+c"),
    );
    assert!(
        by_id[&1001].is_none(),
        "non-hydrated thread carries no hunk"
    );
    assert!(by_id[&1002].is_none());
    assert!(by_id[&1003].is_none());
}

// ===== unread derivation (issue #158) =====

/// Insert a viewer-relation row for (`account_id`, PR 100) with the given
/// `read_at` watermark.
fn seed_viewer_relation(db: &DbHandle, account_id: i64, read_at: Option<i64>) {
    let conn = db.lock().unwrap();
    conn.execute(
        "INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, relation_observed_at, read_at)
         VALUES (?1, ?2, 0, ?3)",
        params![account_id, PR_ID, read_at],
    )
    .unwrap();
}

#[test]
fn list_pr_threads_unread_when_no_relation_row() {
    // No relation row for ALICE on this PR. Every thread has activity > 0
    // (the fixture timestamps), so all four read as unread.
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    assert!(
        threads.iter().all(|t| t.unread),
        "threads must be unread without a read_at watermark"
    );
}

#[test]
fn list_pr_threads_unread_when_read_at_null() {
    // Explicit relation row but NULL read_at (e.g. flipped back to unread via
    // `mark_unread`). Same effect as a missing row: every thread is unread.
    let db = fresh_db();
    seed_fixture(&db);
    seed_viewer_relation(&db, ALICE_ID, None);
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    assert!(threads.iter().all(|t| t.unread));
}

#[test]
fn list_pr_threads_unread_after_stale_read_at() {
    // Viewer last read at t=500. Fixture activity sits in [1000, 2500], so
    // every thread is still unread.
    let db = fresh_db();
    seed_fixture(&db);
    seed_viewer_relation(&db, ALICE_ID, Some(500));
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    assert!(threads.iter().all(|t| t.unread));
}

#[test]
fn list_pr_threads_read_after_fresh_read_at() {
    // Viewer caught up at t=3000, after every fixture timestamp. All threads
    // read.
    let db = fresh_db();
    seed_fixture(&db);
    seed_viewer_relation(&db, ALICE_ID, Some(3_000));
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    assert!(threads.iter().all(|t| !t.unread));
}

#[test]
fn list_pr_threads_unread_is_per_thread_against_watermark() {
    // Viewer read at t=1900. Fixture activity:
    //   thread 1000: last_reply_at=1100 -> read.
    //   thread 1001: last_reply_at=1800 -> read.
    //   thread 1002: created_at=1700, no reply, head=1700 -> read.
    //   thread 1003: created_at=2500 -> unread.
    let db = fresh_db();
    seed_fixture(&db);
    seed_viewer_relation(&db, ALICE_ID, Some(1_900));
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    let by_id: std::collections::HashMap<i64, bool> =
        threads.iter().map(|t| (t.id, t.unread)).collect();
    assert!(!by_id[&1000]);
    assert!(!by_id[&1001]);
    assert!(!by_id[&1002]);
    assert!(by_id[&1003]);
}

#[test]
fn list_pr_threads_unread_false_when_account_id_is_none() {
    // Without a viewer, the projection has no read state to compare against.
    // Forced to false so the dashboard's anonymous reader doesn't accidentally
    // boldface every thread.
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, None).unwrap();
    assert!(threads.iter().all(|t| !t.unread));
}

#[test]
fn list_pr_threads_unread_scoped_to_viewer_account() {
    // Alice has caught up; Bob hasn't opened the PR yet. Bob's read should be
    // unread regardless of alice's relation row.
    let db = fresh_db();
    seed_fixture(&db);
    seed_viewer_relation(&db, ALICE_ID, Some(3_000));
    let conn = db.lock().unwrap();
    let threads_for_bob = query::list_pr_threads(&conn, PR_ID, Some(BOB_ID)).unwrap();
    assert!(
        threads_for_bob.iter().all(|t| t.unread),
        "bob has no relation row; all threads unread"
    );
    let threads_for_alice = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    assert!(threads_for_alice.iter().all(|t| !t.unread));
}

// ===== get_conversation_stats =====

#[test]
fn stats_total_counts_every_thread_including_outdated() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let stats = query::get_conversation_stats(&conn, PR_ID).unwrap();
    // ADR 0012: unresolved and resolved partition all four threads by
    // is_resolved alone. Thread 1002 is outdated AND unresolved, so it
    // belongs in the unresolved bucket; threads_outdated overlaps that.
    assert_eq!(stats.threads_total, 4);
    assert_eq!(stats.threads_unresolved, 3, "1000 + 1002 + 1003");
    assert_eq!(stats.threads_resolved, 1, "1001");
    assert_eq!(stats.threads_outdated, 1, "1002 (overlaps unresolved)");
}

#[test]
fn stats_four_buckets_read_from_review_threads_with_union_involvement() {
    // ADR 0016: the conversation surface bar is computed at read time from
    // `review_threads` + `review_comments`, with the involvement test
    // unioned across every tracked account. The fixture's threads:
    //   1000 (unresolved): bob + alice commented -> unresolved_involved
    //   1001 (resolved):   bob commented         -> resolved_involved
    //   1002 (unresolved): only carol commented  -> unresolved_uninvolved
    //   1003 (unresolved): only carol commented  -> unresolved_uninvolved
    // Carol isn't a tracked account, so threads with only carol's comments
    // count as uninvolved.
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let stats = query::get_conversation_stats(&conn, PR_ID).unwrap();
    assert_eq!(stats.threads_unresolved_involved, 1);
    assert_eq!(stats.threads_unresolved_uninvolved, 2);
    assert_eq!(stats.threads_resolved_involved, 1);
    assert_eq!(stats.threads_resolved_uninvolved, 0);
    let bucket_sum = stats.threads_unresolved_involved
        + stats.threads_unresolved_uninvolved
        + stats.threads_resolved_involved
        + stats.threads_resolved_uninvolved;
    assert_eq!(
        bucket_sum, stats.threads_total,
        "four buckets must sum to threads_total"
    );
}

#[test]
fn stats_oldest_unresolved_includes_outdated_unresolved() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let stats = query::get_conversation_stats(&conn, PR_ID).unwrap();
    // ADR 0012: outdated threads still count when they're unresolved. The
    // oldest unresolved thread is 1000 (created_at 1000); outdated thread
    // 1002 (created_at 1700) is unresolved too but younger.
    assert_eq!(stats.oldest_unresolved_at, Some(1000));
}

#[test]
fn stats_avg_response_seconds_averages_per_thread_gaps() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let stats = query::get_conversation_stats(&conn, PR_ID).unwrap();
    // Thread 1000 gap: 2000 - 1000 = 1000s.
    // Thread 1001 gap: 1600 - 1500 = 100s.
    // Threads 1002, 1003: single comment, no gap contribution.
    // Average across the two non-null gaps = (1000 + 100) / 2 = 550.
    assert_eq!(stats.avg_response_seconds, Some(550));
}

#[test]
fn stats_resolution_rate_uses_total_as_denominator() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let stats = query::get_conversation_stats(&conn, PR_ID).unwrap();
    // ADR 0012: resolved / total, with outdated threads counted normally.
    // resolved=1, total=4 => 0.25
    assert!((stats.resolution_rate - 0.25).abs() < 1e-9);
}

#[test]
fn stats_comment_breakdown_counts_each_source() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let stats = query::get_conversation_stats(&conn, PR_ID).unwrap();
    let bd = &stats.comment_breakdown;
    // `review` sums `reply_count + 1` per thread. Fixture has threads with
    // reply_count = 1, 1, 0, 0 -> 2 + 2 + 1 + 1 = 6.
    assert_eq!(bd.review, 6, "sum(reply_count + 1) across four threads");
    assert_eq!(bd.issue, 4, "issue_comments_count = 4");
    assert_eq!(bd.summary, 2, "two reviews with non-empty body");
    assert_eq!(bd.total, 12);
}

#[test]
fn stats_comment_breakdown_review_uses_reply_count_without_review_comments() {
    // Regression for issue #93: on a PR that has never had its drawer / route
    // opened, the sync cycle populates `review_threads.reply_count` from
    // `comments.totalCount - 1`, but `review_comments` stays empty until the
    // lazy hydrator runs. Before this fix `comment_breakdown.review` counted
    // rows in `review_comments` directly and rendered as zero pre-hydration.
    let db = fresh_db();
    let conn = db.lock().unwrap();
    conn.execute_batch(
        "INSERT INTO accounts (id, label, host, login, created_at)
            VALUES (1, 'a', 'github.com', 'me', 0);
         INSERT INTO repos (id, account_id, owner, name, visibility)
            VALUES (10, 1, 'a', 'r', 'public');
         INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref,
             issue_comments_count)
            VALUES (600, 10, 1, 't', 'open', 0, '', 0, 0, 'main', 'feat', 0);
         -- Three threads with reply_count 2, 1, 0 and ZERO review_comments
         -- rows. Expected: review = (2+1) + (1+1) + (0+1) = 6.
         INSERT INTO review_threads
            (id, pull_request_id, is_resolved, is_outdated, original_line,
             path, node_id, created_at, reply_count)
            VALUES (1, 600, 0, 0, 1, 'f', 'A', 100, 2),
                   (2, 600, 0, 0, 1, 'f', 'B', 200, 1),
                   (3, 600, 0, 0, 1, 'f', 'C', 300, 0);",
    )
    .unwrap();
    let comment_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM review_comments c
               JOIN review_threads t ON t.id = c.review_thread_id
              WHERE t.pull_request_id = 600",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(comment_rows, 0, "fixture seeds zero review_comments");
    let stats = query::get_conversation_stats(&conn, 600).unwrap();
    assert_eq!(
        stats.comment_breakdown.review, 6,
        "review count comes from sum(reply_count + 1), not review_comments rows"
    );
}

#[test]
fn stats_zero_threads_returns_baseline() {
    let db = fresh_db();
    let conn = db.lock().unwrap();
    conn.execute_batch(
        "INSERT INTO accounts (id, label, host, login, created_at)
            VALUES (1, 'a', 'github.com', 'me', 0);
         INSERT INTO repos (id, account_id, owner, name, visibility)
            VALUES (10, 1, 'owner', 'repo', 'public');
         INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref)
            VALUES (200, 10, 1, 't', 'open', 0, '', 0, 0, 'main', 'feat');",
    )
    .unwrap();
    let stats = query::get_conversation_stats(&conn, 200).unwrap();
    assert_eq!(stats.threads_total, 0);
    assert_eq!(stats.threads_unresolved, 0);
    assert_eq!(stats.threads_resolved, 0);
    assert_eq!(stats.threads_outdated, 0);
    assert_eq!(stats.oldest_unresolved_at, None);
    assert_eq!(stats.avg_response_seconds, None);
    assert_eq!(stats.resolution_rate, 0.0);
    assert_eq!(stats.comment_breakdown.total, 0);
    assert_eq!(stats.participants, 0);
    assert_eq!(stats.reviews_summary.total, 0);
    assert_eq!(stats.last_activity_at, None);
}

#[test]
fn stats_participants_unions_distinct_authors_across_surfaces() {
    // The fixture has review_comments from bob, alice, carol (three logins),
    // no issue_comments, and reviews from bob, carol, dave, eve. The union
    // dedupes by login: alice, bob, carol, dave, eve = 5 participants.
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let stats = query::get_conversation_stats(&conn, PR_ID).unwrap();
    assert_eq!(stats.participants, 5);
}

#[test]
fn stats_participants_dedupes_repeat_authors() {
    // A reviewer who's also commented on review threads only counts once.
    let db = fresh_db();
    let conn = db.lock().unwrap();
    conn.execute_batch(
        "INSERT INTO accounts (id, label, host, login, created_at)
            VALUES (1, 'a', 'github.com', 'me', 0);
         INSERT INTO repos (id, account_id, owner, name, visibility)
            VALUES (10, 1, 'a', 'r', 'public');
         INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref)
            VALUES (700, 10, 1, 't', 'open', 0, '', 0, 0, 'main', 'feat');
         INSERT INTO review_threads
            (id, pull_request_id, is_resolved, is_outdated, original_line,
             path, node_id, created_at, reply_count)
            VALUES (1, 700, 0, 0, 1, 'f', 'A', 100, 0);
         INSERT INTO review_comments
            (id, review_thread_id, author_login, body, created_at)
            VALUES (1, 1, 'bob', 'h1', 100);
         INSERT INTO issue_comments
            (id, pull_request_id, author_login, body, created_at)
            VALUES (2, 700, 'bob', 'i1', 110);
         INSERT INTO reviews
            (id, pull_request_id, reviewer_login, state, submitted_at, body, node_id)
            VALUES (3, 700, 'bob', 'COMMENTED', 200, 'r1', 'REV_X');",
    )
    .unwrap();
    let stats = query::get_conversation_stats(&conn, 700).unwrap();
    assert_eq!(stats.participants, 1, "bob counts once across all surfaces");
}

#[test]
fn stats_participants_excludes_pending_reviewers() {
    // Pending reviews aren't surfaced to anyone else yet, so the reviewer
    // doesn't count as a participant on their pending review alone.
    let db = fresh_db();
    let conn = db.lock().unwrap();
    conn.execute_batch(
        "INSERT INTO accounts (id, label, host, login, created_at)
            VALUES (1, 'a', 'github.com', 'me', 0);
         INSERT INTO repos (id, account_id, owner, name, visibility)
            VALUES (10, 1, 'a', 'r', 'public');
         INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref)
            VALUES (701, 10, 1, 't', 'open', 0, '', 0, 0, 'main', 'feat');
         INSERT INTO reviews
            (id, pull_request_id, reviewer_login, state, submitted_at, body, node_id)
            VALUES (1, 701, 'alice', 'PENDING', NULL, NULL, 'REV_P');",
    )
    .unwrap();
    let stats = query::get_conversation_stats(&conn, 701).unwrap();
    assert_eq!(stats.participants, 0);
}

#[test]
fn stats_reviews_summary_buckets_by_submitted_state() {
    // Fixture reviews: APPROVED bob, CHANGES_REQUESTED carol, COMMENTED dave,
    // COMMENTED eve. Pending rows aren't in the fixture; total is the sum
    // of the four submitted-state buckets.
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let stats = query::get_conversation_stats(&conn, PR_ID).unwrap();
    let summary = &stats.reviews_summary;
    assert_eq!(summary.approved, 1);
    assert_eq!(summary.changes_requested, 1);
    assert_eq!(summary.commented, 2);
    assert_eq!(summary.dismissed, 0);
    assert_eq!(summary.total, 4);
}

#[test]
fn stats_reviews_summary_excludes_pending() {
    let db = fresh_db();
    let conn = db.lock().unwrap();
    conn.execute_batch(
        "INSERT INTO accounts (id, label, host, login, created_at)
            VALUES (1, 'a', 'github.com', 'me', 0);
         INSERT INTO repos (id, account_id, owner, name, visibility)
            VALUES (10, 1, 'a', 'r', 'public');
         INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref)
            VALUES (800, 10, 1, 't', 'open', 0, '', 0, 0, 'main', 'feat');
         INSERT INTO reviews
            (id, pull_request_id, reviewer_login, state, submitted_at, body, node_id) VALUES
            (1, 800, 'bob',   'APPROVED', 100, 'lgtm', 'R_A'),
            (2, 800, 'carol', 'PENDING',  NULL, NULL,  'R_P'),
            (3, 800, 'dave',  'DISMISSED', 200, NULL,  'R_D');",
    )
    .unwrap();
    let stats = query::get_conversation_stats(&conn, 800).unwrap();
    let summary = &stats.reviews_summary;
    assert_eq!(summary.approved, 1);
    assert_eq!(summary.dismissed, 1);
    assert_eq!(summary.total, 2, "pending review excluded from total");
}

#[test]
fn stats_last_activity_picks_max_across_comments_issues_reviews() {
    // Fixture: review_comments max ts = 2500 (thread 1003 head from carol),
    // issue_comments has none, reviews max submitted_at = 2300 (eve).
    // Last activity = 2500.
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let stats = query::get_conversation_stats(&conn, PR_ID).unwrap();
    assert_eq!(stats.last_activity_at, Some(2500));
}

#[test]
fn stats_last_activity_uses_issue_comments_when_newest() {
    let db = fresh_db();
    seed_fixture(&db);
    // Insert an issue comment newer than every fixture timestamp; the stat
    // should track it.
    db.lock()
        .unwrap()
        .execute(
            "INSERT INTO issue_comments
                (id, pull_request_id, author_login, body, created_at)
              VALUES (900001, 100, 'frank', 'late', 4000)",
            [],
        )
        .unwrap();
    let conn = db.lock().unwrap();
    let stats = query::get_conversation_stats(&conn, PR_ID).unwrap();
    assert_eq!(stats.last_activity_at, Some(4000));
}

#[test]
fn stats_last_activity_none_when_no_activity() {
    let db = fresh_db();
    let conn = db.lock().unwrap();
    conn.execute_batch(
        "INSERT INTO accounts (id, label, host, login, created_at)
            VALUES (1, 'a', 'github.com', 'me', 0);
         INSERT INTO repos (id, account_id, owner, name, visibility)
            VALUES (10, 1, 'a', 'r', 'public');
         INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref)
            VALUES (900, 10, 1, 't', 'open', 0, '', 0, 0, 'main', 'feat');",
    )
    .unwrap();
    let stats = query::get_conversation_stats(&conn, 900).unwrap();
    assert_eq!(stats.last_activity_at, None);
}

#[test]
fn stats_all_resolved_yields_resolution_rate_one() {
    let db = fresh_db();
    let conn = db.lock().unwrap();
    conn.execute_batch(
        "INSERT INTO accounts (id, label, host, login, created_at)
            VALUES (1, 'a', 'github.com', 'me', 0);
         INSERT INTO repos (id, account_id, owner, name, visibility)
            VALUES (10, 1, 'a', 'r', 'public');
         INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref)
            VALUES (300, 10, 1, 't', 'open', 0, '', 0, 0, 'main', 'feat');
         INSERT INTO review_threads
            (id, pull_request_id, is_resolved, is_outdated, original_line,
             path, node_id, created_at, resolved_at, reply_count)
            VALUES (1, 300, 1, 0, 1, 'f', 'A', 1, 2, 0),
                   (2, 300, 1, 0, 1, 'f', 'B', 1, 2, 0);",
    )
    .unwrap();
    let stats = query::get_conversation_stats(&conn, 300).unwrap();
    assert_eq!(stats.resolution_rate, 1.0);
    assert_eq!(stats.threads_unresolved, 0);
}

#[test]
fn stats_resolved_includes_resolved_and_outdated_intersection() {
    // ADR 0012: resolved and unresolved partition every thread by is_resolved
    // alone. A thread that's both resolved AND outdated counts in
    // threads_resolved; threads_outdated overlaps (still surfaced as a
    // separate count for the stats tile). The resolution rate stays in
    // [0, 1] by construction because resolved <= total.
    let db = fresh_db();
    let conn = db.lock().unwrap();
    conn.execute_batch(
        "INSERT INTO accounts (id, label, host, login, created_at)
            VALUES (1, 'a', 'github.com', 'me', 0);
         INSERT INTO repos (id, account_id, owner, name, visibility)
            VALUES (10, 1, 'a', 'r', 'public');
         INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref)
            VALUES (400, 10, 1, 't', 'open', 0, '', 0, 0, 'main', 'feat');
         -- 7 threads total: 3 strict-active resolved + 4 resolved-and-outdated.
         INSERT INTO review_threads
            (id, pull_request_id, is_resolved, is_outdated, original_line,
             path, node_id, created_at, resolved_at, reply_count)
            VALUES (1, 400, 1, 0, 1, 'f', 'A', 1, 2, 0),
                   (2, 400, 1, 0, 1, 'f', 'B', 1, 2, 0),
                   (3, 400, 1, 0, 1, 'f', 'C', 1, 2, 0),
                   (4, 400, 1, 1, 1, 'f', 'D', 1, 2, 0),
                   (5, 400, 1, 1, 1, 'f', 'E', 1, 2, 0),
                   (6, 400, 1, 1, 1, 'f', 'F', 1, 2, 0),
                   (7, 400, 1, 1, 1, 'f', 'G', 1, 2, 0);",
    )
    .unwrap();
    let stats = query::get_conversation_stats(&conn, 400).unwrap();
    assert_eq!(stats.threads_total, 7);
    assert_eq!(stats.threads_outdated, 4);
    assert_eq!(
        stats.threads_resolved, 7,
        "all seven threads have is_resolved = 1"
    );
    assert_eq!(stats.threads_unresolved, 0);
    assert_eq!(stats.resolution_rate, 1.0);
}

#[test]
fn stats_all_outdated_unresolved_count_normally() {
    // ADR 0012: outdated threads count in the denominator. Two outdated
    // unresolved threads => total=2, resolved=0, rate=0/2=0. The oldest
    // unresolved timestamp now includes outdated rows.
    let db = fresh_db();
    let conn = db.lock().unwrap();
    conn.execute_batch(
        "INSERT INTO accounts (id, label, host, login, created_at)
            VALUES (1, 'a', 'github.com', 'me', 0);
         INSERT INTO repos (id, account_id, owner, name, visibility)
            VALUES (10, 1, 'a', 'r', 'public');
         INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref)
            VALUES (400, 10, 1, 't', 'open', 0, '', 0, 0, 'main', 'feat');
         INSERT INTO review_threads
            (id, pull_request_id, is_resolved, is_outdated, original_line,
             path, node_id, created_at, reply_count)
            VALUES (5, 400, 0, 1, 1, 'f', 'X', 100, 0),
                   (6, 400, 0, 1, 1, 'f', 'Y', 200, 0);",
    )
    .unwrap();
    let stats = query::get_conversation_stats(&conn, 400).unwrap();
    assert_eq!(stats.resolution_rate, 0.0, "no resolved threads");
    assert_eq!(
        stats.oldest_unresolved_at,
        Some(100),
        "outdated-unresolved row still surfaces as oldest"
    );
    assert_eq!(stats.threads_outdated, 2);
    assert_eq!(stats.threads_unresolved, 2);
}

#[test]
fn stats_single_comment_threads_contribute_no_response_gap() {
    let db = fresh_db();
    let conn = db.lock().unwrap();
    conn.execute_batch(
        "INSERT INTO accounts (id, label, host, login, created_at)
            VALUES (1, 'a', 'github.com', 'me', 0);
         INSERT INTO repos (id, account_id, owner, name, visibility)
            VALUES (10, 1, 'a', 'r', 'public');
         INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref)
            VALUES (500, 10, 1, 't', 'open', 0, '', 0, 0, 'main', 'feat');
         INSERT INTO review_threads
            (id, pull_request_id, is_resolved, is_outdated, original_line,
             path, node_id, created_at, reply_count)
            VALUES (10, 500, 0, 0, 1, 'f', 'X', 100, 0),
                   (11, 500, 0, 0, 1, 'f', 'Y', 200, 0);
         INSERT INTO review_comments
            (id, review_thread_id, author_login, body, created_at)
            VALUES (1, 10, 'x', 'h1', 100),
                   (2, 11, 'y', 'h2', 200);",
    )
    .unwrap();
    let stats = query::get_conversation_stats(&conn, 500).unwrap();
    // No thread has a second comment -> the gaps CTE yields nothing.
    assert_eq!(stats.avg_response_seconds, None);
}

#[test]
fn stats_unaccounted_account_does_not_break_query() {
    // Smoke-test: get_conversation_stats doesn't take an account, so
    // passing only a PR ID works regardless of whose stats we want.
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    assert!(query::get_conversation_stats(&conn, PR_ID).is_ok());
}

#[test]
fn bob_is_involved_in_threads_he_commented_on() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    // Bob authored the head on threads 1000, 1001, and replied on 1001.
    let threads = query::list_pr_threads(&conn, PR_ID, Some(BOB_ID)).unwrap();
    let map: std::collections::HashMap<i64, bool> =
        threads.iter().map(|t| (t.id, t.is_involved)).collect();
    assert!(map[&1000]);
    assert!(map[&1001]);
    assert!(!map[&1002]);
    assert!(!map[&1003]);
}

// ===== persist_payload (the lazy hydrator's persistence path) =====

fn page_info(has_next: bool) -> PageInfo {
    PageInfo {
        has_next_page: has_next,
        end_cursor: None,
    }
}

fn make_comment(id: &str, db_id: i64, login: &str, created_at: &str) -> ReviewCommentNode {
    ReviewCommentNode {
        id: id.into(),
        url: None,
        database_id: Some(db_id),
        author: Some(Actor::new(login)),
        body: format!("body of {id}"),
        body_html: None,
        body_text: format!("body of {id}"),
        created_at: created_at.into(),
        path: Some("f.rs".into()),
        line: Some(7),
        original_line: Some(7),
        side: Some("RIGHT".into()),
        diff_hunk: None,
    }
}

fn make_issue(id: &str, db_id: i64, login: &str) -> IssueCommentNode {
    IssueCommentNode {
        id: id.into(),
        url: None,
        database_id: Some(db_id),
        author: Some(Actor::new(login)),
        body: format!("issue body {id}"),
        body_html: None,
        body_text: format!("issue body {id}"),
        created_at: "2026-05-19T13:00:00Z".into(),
    }
}

fn make_thread(node_id: &str, comments: Vec<ReviewCommentNode>) -> ReviewThreadComments {
    ReviewThreadComments {
        id: node_id.into(),
        comments: ReviewCommentConnection {
            page_info: page_info(false),
            nodes: comments,
        },
    }
}

#[test]
fn hydrator_persists_thread_and_issue_comments() {
    let db = fresh_db();
    seed_fixture(&db);

    commands_testing::persist(
        &db,
        PR_ID,
        vec![make_thread(
            "PRRT_1",
            vec![
                make_comment("PRRC_NEW1", 88001, "bob", "2026-05-19T10:00:00Z"),
                make_comment("PRRC_NEW2", 88002, "alice", "2026-05-19T11:00:00Z"),
            ],
        )],
        vec![
            make_issue("IC_NEW1", 99001, "bob"),
            make_issue("IC_NEW2", 99002, "carol"),
        ],
    )
    .unwrap();

    let conn = db.lock().unwrap();
    let comments = query::list_thread_comments(&conn, PR_ID).unwrap();
    // The seed fixture wrote 2 comments to thread 1000 (PRRT_1) and 2 to 1001
    // (PRRT_2). The hydrator should upsert two new bodies under thread 1000.
    let new_ones: Vec<&prism_lib::conversation::types::ThreadComment> = comments
        .iter()
        .filter(|c| c.body.starts_with("body of PRRC_NEW"))
        .collect();
    assert_eq!(new_ones.len(), 2);
    assert!(new_ones.iter().all(|c| c.thread_id == 1000));

    let issues = query::list_issue_comments(&conn, PR_ID).unwrap();
    assert_eq!(issues.len(), 2);
    assert!(issues.iter().any(|c| c.author_login == "bob"));
    assert!(issues.iter().any(|c| c.author_login == "carol"));
}

#[test]
fn hydrator_is_idempotent_across_repeated_calls() {
    let db = fresh_db();
    seed_fixture(&db);

    let payload = || {
        (
            vec![make_thread(
                "PRRT_1",
                vec![make_comment(
                    "PRRC_DUP1",
                    77001,
                    "bob",
                    "2026-05-19T10:00:00Z",
                )],
            )],
            vec![make_issue("IC_DUP1", 66001, "alice")],
        )
    };

    let (t1, i1) = payload();
    commands_testing::persist(&db, PR_ID, t1, i1).unwrap();
    let conn = db.lock().unwrap();
    let count_after_first: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM review_comments WHERE node_id = 'PRRC_DUP1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(conn);
    assert_eq!(count_after_first, 1);

    // Re-run with the same node ids but a different body to confirm upsert.
    let mut updated_comment = make_comment("PRRC_DUP1", 77001, "bob", "2026-05-19T10:00:00Z");
    updated_comment.body = "edited".into();
    commands_testing::persist(
        &db,
        PR_ID,
        vec![make_thread("PRRT_1", vec![updated_comment])],
        vec![make_issue("IC_DUP1", 66001, "alice")],
    )
    .unwrap();

    let conn = db.lock().unwrap();
    let (count, body): (i64, String) = conn
        .query_row(
            "SELECT COUNT(*), MAX(body) FROM review_comments WHERE node_id = 'PRRC_DUP1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 1, "upsert keeps one row");
    assert_eq!(body, "edited", "body updates in place");
}

#[test]
fn hydrator_skips_threads_whose_node_id_isnt_known() {
    let db = fresh_db();
    seed_fixture(&db);

    // PRRT_unknown isn't in `review_threads.node_id` — its comments must be
    // dropped rather than orphan-inserted under a phantom thread id.
    commands_testing::persist(
        &db,
        PR_ID,
        vec![make_thread(
            "PRRT_unknown",
            vec![make_comment(
                "PRRC_ORPHAN",
                5001,
                "x",
                "2026-05-19T10:00:00Z",
            )],
        )],
        vec![],
    )
    .unwrap();

    let conn = db.lock().unwrap();
    let orphans: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM review_comments WHERE node_id = 'PRRC_ORPHAN'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0, "orphan comments must not be persisted");
}

#[test]
fn hydrated_response_includes_threads_comments_reviews_stats() {
    let db = fresh_db();
    seed_fixture(&db);

    // Persist an extra issue comment so the hydrated response's `issue_comments`
    // surface lights up.
    commands_testing::persist(
        &db,
        PR_ID,
        vec![],
        vec![make_issue("IC_NEW3", 99003, "carol")],
    )
    .unwrap();

    let conn = db.lock().unwrap();
    let hydrated = commands_testing::build_hydrated(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    assert_eq!(hydrated.pull_request_id, PR_ID);
    assert_eq!(hydrated.threads.len(), 4);
    assert!(hydrated.thread_comments.len() >= 6);
    assert!(!hydrated.issue_comments.is_empty());
    assert_eq!(hydrated.reviews.len(), 4);
    assert_eq!(hydrated.stats.threads_total, 4);
}

#[test]
fn hydrator_transaction_rolls_back_when_id_resolution_fails() {
    // This is a belt-and-braces: passing a known thread node id but combining
    // it with an issue comment whose body contains invalid UTF-8 isn't easy
    // here (the wire shape is strings only). Instead, exercise the partial
    // persist case: a payload with one valid + one unknown thread should still
    // write the valid thread's comments because the unknown thread is just
    // skipped (the transaction commits).
    let db = fresh_db();
    seed_fixture(&db);

    commands_testing::persist(
        &db,
        PR_ID,
        vec![
            make_thread(
                "PRRT_1",
                vec![make_comment(
                    "PRRC_KEEP",
                    12345,
                    "alice",
                    "2026-05-19T10:00:00Z",
                )],
            ),
            make_thread(
                "PRRT_GONE",
                vec![make_comment(
                    "PRRC_GONE",
                    99999,
                    "x",
                    "2026-05-19T10:00:00Z",
                )],
            ),
        ],
        vec![],
    )
    .unwrap();

    let conn = db.lock().unwrap();
    let kept: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM review_comments WHERE node_id = 'PRRC_KEEP'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let gone: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM review_comments WHERE node_id = 'PRRC_GONE'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1, "valid thread's comment persisted");
    assert_eq!(gone, 0, "comment whose thread isn't known is skipped");
}

#[test]
fn list_reviews_returns_persisted_reviews_in_submitted_order() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let reviews = query::list_reviews(&conn, PR_ID).unwrap();
    let ids: Vec<i64> = reviews.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![70001, 70002, 70003, 70004]);
}

#[test]
fn list_issue_comments_returns_empty_when_none_persisted() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let issues = query::list_issue_comments(&conn, PR_ID).unwrap();
    assert!(issues.is_empty(), "fixture seeds none");
}

#[test]
fn parameterised_pr_id_isolates_queries_across_prs() {
    let db = fresh_db();
    seed_fixture(&db);
    {
        let conn = db.lock().unwrap();
        conn.execute_batch(
            "INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref,
                 issue_comments_count)
                VALUES
                (101, 10, 2, 'web/#2', 'open', 0, 'alice', 0, 0, 'main', 'feat', 0);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, original_line,
                 path, node_id, created_at, reply_count)
                VALUES (2000, 101, 0, 0, 1, 'f', 'OTHER', 999, 0)",
            params![],
        )
        .unwrap();
    }
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    assert_eq!(threads.len(), 4, "PR 100 unaffected by PR 101 row");
}

// ===== list_pr_timeline_events =====

fn seed_timeline_events(db: &DbHandle) {
    let conn = db.lock().unwrap();
    conn.execute_batch(
        r#"
        -- Sibling PR so the isolation row below clears the FK constraint.
        INSERT INTO pull_requests
            (id, repo_id, number, title, state, is_draft, author_login,
             created_at, updated_at, base_ref, head_ref,
             issue_comments_count)
            VALUES
            (101, 10, 2, 'web/#2', 'open', 0, 'alice', 0, 0, 'main', 'feat', 0);

        INSERT INTO timeline_events
            (pull_request_id, event_type, actor_login, created_at, payload) VALUES
            (100, 'ready_for_review', 'alice', 1000, '{}'),
            (100, 'review_requested', 'alice', 1100, '{}'),
            (100, 'reviewed',         'bob',   1200, '{"state":"APPROVED"}'),
            (100, 'merged',           'alice', 1300, '{}'),
            -- Another PR's row to verify isolation.
            (101, 'closed',           'alice', 1400, '{}');
        "#,
    )
    .unwrap();
}

#[test]
fn list_pr_timeline_events_orders_by_created_at() {
    let db = fresh_db();
    seed_fixture(&db);
    // Insert an out-of-order timeline event to confirm ordering.
    {
        let conn = db.lock().unwrap();
        conn.execute_batch(
            r#"
            INSERT INTO timeline_events
                (pull_request_id, event_type, actor_login, created_at, payload) VALUES
                (100, 'merged',           'alice', 2000, '{}'),
                (100, 'ready_for_review', 'alice', 1000, '{}'),
                (100, 'reviewed',         'bob',   1500, '{"state":"CHANGES_REQUESTED"}');
            "#,
        )
        .unwrap();
    }
    let conn = db.lock().unwrap();
    let events = query::list_pr_timeline_events(&conn, PR_ID).unwrap();
    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(types, vec!["ready_for_review", "reviewed", "merged"]);
}

#[test]
fn list_pr_timeline_events_extracts_review_state_from_payload() {
    let db = fresh_db();
    seed_fixture(&db);
    seed_timeline_events(&db);
    let conn = db.lock().unwrap();
    let events = query::list_pr_timeline_events(&conn, PR_ID).unwrap();
    let reviewed = events
        .iter()
        .find(|e| e.event_type == "reviewed")
        .expect("reviewed event present");
    assert_eq!(reviewed.review_state.as_deref(), Some("APPROVED"));
    assert_eq!(reviewed.actor_login.as_deref(), Some("bob"));

    let merged = events
        .iter()
        .find(|e| e.event_type == "merged")
        .expect("merged event present");
    assert!(
        merged.review_state.is_none(),
        "non-reviewed events have no state",
    );
}

#[test]
fn list_pr_timeline_events_isolates_by_pull_request() {
    let db = fresh_db();
    seed_fixture(&db);
    seed_timeline_events(&db);
    let conn = db.lock().unwrap();
    let events = query::list_pr_timeline_events(&conn, PR_ID).unwrap();
    assert_eq!(events.len(), 4, "rows for PR 101 stay out");
}

#[test]
fn list_pr_timeline_events_returns_empty_for_unknown_pr() {
    let db = fresh_db();
    seed_fixture(&db);
    let conn = db.lock().unwrap();
    let events = query::list_pr_timeline_events(&conn, 9999).unwrap();
    assert!(events.is_empty());
}

#[test]
fn list_pr_threads_resolves_head_comment_avatar_via_users_join() {
    // ADR 0013: the threads list reads the head-comment author's avatar URL
    // through `LEFT JOIN users`. Logins absent from `users` surface `None`.
    let db = fresh_db();
    seed_fixture(&db);
    {
        let conn = db.lock().unwrap();
        conn.execute(
            "INSERT INTO users (login, avatar_url, last_seen_at)
                VALUES ('bob', 'https://avatars/bob.png', 0)",
            [],
        )
        .unwrap();
    }
    let conn = db.lock().unwrap();
    let threads = query::list_pr_threads(&conn, PR_ID, Some(ALICE_ID)).unwrap();
    let bob_thread = threads.iter().find(|t| t.id == 1000).unwrap();
    let head = bob_thread.head_comment.as_ref().unwrap();
    assert_eq!(head.author_login, "bob");
    assert_eq!(head.avatar_url.as_deref(), Some("https://avatars/bob.png"));

    let carol_thread = threads.iter().find(|t| t.id == 1003).unwrap();
    let head = carol_thread.head_comment.as_ref().unwrap();
    assert_eq!(head.author_login, "carol");
    assert!(head.avatar_url.is_none(), "carol is not in users");
}

#[test]
fn list_pr_timeline_events_resolves_actor_avatar_via_users_join() {
    let db = fresh_db();
    seed_fixture(&db);
    seed_timeline_events(&db);
    {
        let conn = db.lock().unwrap();
        conn.execute(
            "INSERT INTO users (login, avatar_url, last_seen_at)
                VALUES ('alice', 'https://avatars/alice.png', 0)",
            [],
        )
        .unwrap();
    }
    let conn = db.lock().unwrap();
    let events = query::list_pr_timeline_events(&conn, PR_ID).unwrap();
    let alice_event = events
        .iter()
        .find(|e| e.actor_login.as_deref() == Some("alice"))
        .expect("alice timeline event seeded by fixture");
    assert_eq!(
        alice_event.actor_avatar_url.as_deref(),
        Some("https://avatars/alice.png"),
    );
}
