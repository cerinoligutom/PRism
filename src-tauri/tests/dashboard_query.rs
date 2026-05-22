//! Integration tests for `dashboard::query::list_pull_requests`.
//!
//! Each test seeds a small fixture into a fresh in-memory SQLite DB (with the
//! same migrations the app runs), invokes the public query function, and
//! asserts on the returned `DashboardPullRequest` projection.
//!
//! Fixture shape (shared across every test):
//!
//! - Accounts: 1 = alice (login `alice`), 2 = bob (login `bob`).
//! - Repos: 10 (alice/web), 20 (alice/api, team-tracked), 30 (bob/cli,
//!   team-tracked).
//! - PRs:
//!   - 100 (alice/web, #1): alice authored; bob review-requested.
//!     `latest_status_change_at = 1000`. Has two submitted reviews + pending.
//!   - 200 (alice/web, #2): alice involved (commented). `updated_at = 900`,
//!     no `latest_status_change_at`.
//!   - 300 (alice/api, #1): team-tracked repo, no relations seeded.
//!     `latest_status_change_at = 1500`. Carries a DISMISSED review that must
//!     be dropped from the projection.
//!   - 400 (bob/cli,  #1): bob authored; alice involved + review-requested.
//!     Team-tracked. `latest_status_change_at = 2000`.
//!   - 500 (alice/web, #3): bob involved. `updated_at = 800`,
//!     `latest_status_change_at = NULL`. CI = SUCCESS 5/5.

use prism_lib::dashboard::query::list_pull_requests as inner_list_pull_requests;
use prism_lib::dashboard::types::DashboardPullRequest;
use prism_lib::dashboard::{DashboardSort, DashboardView, ReviewerEntry, ReviewerState};
use prism_lib::db::migrate;
use prism_lib::triage::types::ChipKey;
use rusqlite::Connection;

fn fresh_db() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate::run(&mut conn).unwrap();
    conn
}

/// Backwards-compatible shim for tests that pre-date the M4-D chip-filter
/// argument. Defaults to "no chips active" so the assertions stay focused on
/// view / sort / account behaviour. Tests that exercise the chip composition
/// call [`inner_list_pull_requests`] directly.
fn list_pull_requests(
    conn: &Connection,
    view: DashboardView,
    sort: DashboardSort,
    account_id: Option<i64>,
) -> Result<Vec<DashboardPullRequest>, rusqlite::Error> {
    inner_list_pull_requests(conn, view, sort, account_id, &[])
}

fn seed_fixture(conn: &Connection) {
    conn.execute_batch(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'alice-acct', 'github.com', 'alice', 0),
            (2, 'bob-acct',   'github.com', 'bob',   0);

        INSERT INTO repos (id, account_id, owner, name, visibility, is_team_tracked) VALUES
            (10, 1, 'alice', 'web', 'public', 0),
            (20, 1, 'alice', 'api', 'public', 1),
            (30, 2, 'bob',   'cli', 'public', 1);

        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, latest_status_change_at, base_ref, head_ref,
             mergeable, review_decision, additions, deletions, changed_files,
             ci_state, ci_total, ci_passing) VALUES
            (100, 10, 1, 'web/#1', 'open', 0, 'alice', 0, 950,  1000, 'main', 'feat-a',
             'MERGEABLE', 'REVIEW_REQUIRED', 10, 5, 2, 'PENDING', 3, 1),
            (200, 10, 2, 'web/#2', 'open', 0, 'carol', 0, 900,  NULL, 'main', 'feat-b',
             NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL),
            (300, 20, 1, 'api/#1', 'open', 1, 'dave',  0, 1450, 1500, 'main', 'feat-c',
             'UNKNOWN', NULL, 1, 1, 1, NULL, NULL, NULL),
            (400, 30, 1, 'cli/#1', 'open', 0, 'bob',   0, 1950, 2000, 'main', 'feat-d',
             'MERGEABLE', 'APPROVED', 100, 20, 8, 'FAILURE', 4, 2),
            (500, 10, 3, 'web/#3', 'open', 0, 'erin',  0, 800,  NULL, 'main', 'feat-e',
             'MERGEABLE', 'APPROVED', 2, 0, 1, 'SUCCESS', 5, 5);

        -- Relations: alice authored PR 100; bob review-requested on PR 100.
        INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at) VALUES
            (1, 100, 1, 0, 1, 0),
            (2, 100, 0, 1, 0, 0),
            (1, 200, 0, 0, 1, 0),
            (2, 400, 1, 0, 1, 0),
            (1, 400, 0, 1, 1, 0),
            (2, 500, 0, 0, 1, 0);

        -- Submitted reviews.
        INSERT INTO reviews (id, pull_request_id, reviewer_login, state, submitted_at) VALUES
            (9001, 100, 'bob',   'COMMENTED',         500),
            (9002, 100, 'carol', 'APPROVED',          600),
            (9003, 300, 'frank', 'DISMISSED',         700),
            (9004, 400, 'alice', 'CHANGES_REQUESTED', 800);

        -- Requested reviewers (pending).
        INSERT INTO requested_reviewers (id, pull_request_id, login, reviewer_type) VALUES
            (8001, 100, 'dora',  'user'),
            (8002, 400, 'alice', 'user');
        "#,
    )
    .unwrap();
}

#[test]
fn authored_view_returns_only_authored_prs_per_account() {
    let conn = fresh_db();
    seed_fixture(&conn);

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![100], "alice authored only PR 100");

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(2),
    )
    .unwrap();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![400], "bob authored only PR 400");
}

#[test]
fn authored_pr_hydrates_reviews_and_requested_reviewers() {
    let conn = fresh_db();
    seed_fixture(&conn);

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = &rows[0];
    assert_eq!(pr.reviewers.len(), 3);

    assert!(pr
        .reviewers
        .iter()
        .any(|r| r.login == "carol" && r.state == ReviewerState::Approved));
    assert!(pr
        .reviewers
        .iter()
        .any(|r| r.login == "bob" && r.state == ReviewerState::Commented));
    assert!(pr
        .reviewers
        .iter()
        .any(|r| r.login == "dora" && r.state == ReviewerState::Pending));
}

#[test]
fn assigned_view_returns_only_review_requested_prs_per_account() {
    let conn = fresh_db();
    seed_fixture(&conn);

    let rows = list_pull_requests(
        &conn,
        DashboardView::Assigned,
        DashboardSort::Updated,
        Some(2),
    )
    .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![100],
        "bob is review-requested on PR 100 only"
    );

    let rows = list_pull_requests(
        &conn,
        DashboardView::Assigned,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![400],
        "alice is review-requested on PR 400 only"
    );
}

#[test]
fn watching_view_returns_only_involved_prs_per_account() {
    let conn = fresh_db();
    seed_fixture(&conn);

    let rows = list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    // alice is involved with 100, 200, 400. Ordered by COALESCE desc:
    // 400 (lsc=2000), 100 (lsc=1000), 200 (updated_at=900).
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![400, 100, 200]
    );
}

#[test]
fn team_view_uses_repo_flag_and_skips_relations_table() {
    let conn = fresh_db();
    seed_fixture(&conn);

    // PR 300 sits in repo 20 (account 1, team-tracked); PR 400 in repo 30
    // (account 2, team-tracked). PR 100 sits in repo 10 (not team-tracked)
    // even though alice authored it, so it must not appear here.
    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, None).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![400, 300],
        "Team view: ordered by COALESCE desc across every team-tracked repo"
    );

    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, Some(1)).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![300],
        "Team view filtered to account 1's team-tracked repos"
    );
}

#[test]
fn team_view_drops_dismissed_reviews_from_projection() {
    let conn = fresh_db();
    seed_fixture(&conn);

    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, None).unwrap();
    let pr_300 = rows.iter().find(|r| r.id == 300).unwrap();
    assert!(
        pr_300.reviewers.is_empty(),
        "PR 300's only review is DISMISSED, which must be dropped"
    );
}

#[test]
fn account_id_none_returns_deduped_rows_with_merged_account_ids() {
    let conn = fresh_db();
    seed_fixture(&conn);

    // Watching across every account (ADR 0016): the fixture's relation rows
    // for `is_involved = 1` are (1,100), (1,200), (2,400), (1,400), (2,500).
    // PR 400 has two involved-relation owners and PR 100's `is_involved = 1`
    // sits on account 1 with account 2 also having a (non-involved-flagged)
    // relation row. The unified path GROUPs by `pr.id` and folds every
    // relation owner into `account_ids`. The view-filter EXISTS still gates
    // on `is_involved = 1`, so PR 500 (only involved under account 2) and
    // PR 100 (involved under account 1) both surface once.
    let rows =
        list_pull_requests(&conn, DashboardView::Watching, DashboardSort::Updated, None).unwrap();
    let mut actual: Vec<(i64, Vec<i64>)> =
        rows.iter().map(|r| (r.id, r.account_ids.clone())).collect();
    actual.sort();
    // PR 100: account 1 (involved) + account 2 (review-requested) -> [1, 2].
    // PR 200: account 1 only -> [1].
    // PR 400: account 1 + account 2 both involved -> [1, 2].
    // PR 500: account 2 only -> [2].
    let mut expected = vec![
        (100, vec![1, 2]),
        (200, vec![1]),
        (400, vec![1, 2]),
        (500, vec![2]),
    ];
    expected.sort();
    assert_eq!(actual, expected);
}

#[test]
fn is_you_marks_reviewers_matching_the_owning_account_login() {
    let conn = fresh_db();
    seed_fixture(&conn);

    // For alice: PR 400 has alice as both a submitted reviewer
    // (CHANGES_REQUESTED) and a requested reviewer (pending). After
    // dedup, exactly one alice row remains carrying the submitted state, and
    // it must be marked is_you.
    let rows = list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr_400 = rows.iter().find(|r| r.id == 400).unwrap();
    let alice_entries: Vec<&ReviewerEntry> = pr_400
        .reviewers
        .iter()
        .filter(|r| r.login == "alice")
        .collect();
    assert_eq!(alice_entries.len(), 1);
    assert_eq!(alice_entries[0].state, ReviewerState::ChangesRequested);
    assert!(alice_entries[0].is_you);
}

#[test]
fn is_you_is_false_when_login_mismatches_owning_account() {
    let conn = fresh_db();
    seed_fixture(&conn);

    // PR 400 surfaced under bob's account (he authored it): same reviewer rows
    // exist but alice's login != bob, so is_you must be false on those rows.
    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(2),
    )
    .unwrap();
    let pr_400 = rows.iter().find(|r| r.id == 400).unwrap();
    assert!(pr_400
        .reviewers
        .iter()
        .filter(|r| r.login == "alice")
        .all(|r| !r.is_you));
}

#[test]
fn sort_order_uses_latest_status_change_then_updated_at() {
    let conn = fresh_db();
    seed_fixture(&conn);

    // Watching for alice puts the rows under deterministic control:
    // - PR 400 lsc=2000
    // - PR 100 lsc=1000
    // - PR 200 lsc=NULL, updated_at=900
    let rows = list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let ordered: Vec<i64> = rows.iter().map(|r| r.id).collect();
    assert_eq!(ordered, vec![400, 100, 200]);
}

#[test]
fn ci_summary_is_none_when_state_is_null() {
    let conn = fresh_db();
    seed_fixture(&conn);

    let rows = list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr_200 = rows.iter().find(|r| r.id == 200).unwrap();
    assert!(pr_200.ci.is_none());
}

#[test]
fn ci_summary_populates_state_total_passing() {
    let conn = fresh_db();
    seed_fixture(&conn);

    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, None).unwrap();
    let pr_400 = rows.iter().find(|r| r.id == 400).unwrap();
    let ci = pr_400.ci.as_ref().expect("PR 400 has CI");
    assert_eq!(ci.state, "FAILURE");
    assert_eq!(ci.total, 4);
    assert_eq!(ci.passing, 2);
}

#[test]
fn url_is_synthesised_from_account_host_and_repo_coords() {
    let conn = fresh_db();
    seed_fixture(&conn);

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    assert_eq!(rows[0].url, "https://github.com/alice/web/pull/1");
}

#[test]
fn empty_db_returns_empty_vec_for_every_view() {
    let conn = fresh_db();
    for view in [
        DashboardView::Authored,
        DashboardView::Assigned,
        DashboardView::Watching,
        DashboardView::Team,
    ] {
        let rows = list_pull_requests(&conn, view, DashboardSort::Updated, None).unwrap();
        assert!(rows.is_empty(), "{view:?} should be empty on a bare DB");
    }
}

#[test]
fn account_filter_excludes_other_accounts_in_authored_view() {
    let conn = fresh_db();
    seed_fixture(&conn);

    conn.execute_batch(
        "INSERT INTO accounts (id, label, host, login, created_at)
            VALUES (3, 'c', 'github.com', 'carol', 0);
         INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at)
            VALUES (3, 500, 1, 0, 0, 0);",
    )
    .unwrap();

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        vec![100],
        "account 3's stray authored relation must not surface for account 1"
    );
}

#[test]
fn projection_carries_top_level_pr_fields() {
    let conn = fresh_db();
    seed_fixture(&conn);

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = &rows[0];

    assert_eq!(pr.id, 100);
    assert_eq!(pr.number, 1);
    assert_eq!(pr.title, "web/#1");
    assert_eq!(pr.state, "open");
    assert!(!pr.is_draft);
    assert_eq!(pr.mergeable.as_deref(), Some("MERGEABLE"));
    assert_eq!(pr.review_decision.as_deref(), Some("REVIEW_REQUIRED"));
    assert_eq!(pr.author_login, "alice");
    assert_eq!(pr.base_ref, "main");
    assert_eq!(pr.head_ref, "feat-a");
    assert_eq!(pr.latest_status_change_at, Some(1000));
    assert_eq!(pr.additions, Some(10));
    assert_eq!(pr.deletions, Some(5));
    assert_eq!(pr.changed_files, Some(2));
    assert_eq!(pr.repo.id, 10);
    assert_eq!(pr.repo.owner, "alice");
    assert_eq!(pr.repo.name, "web");
    assert_eq!(pr.account_ids, vec![1]);
}

#[test]
fn draft_flag_round_trips_as_bool() {
    let conn = fresh_db();
    seed_fixture(&conn);

    // PR 300 is the only draft in the fixture, surfaced via the Team view.
    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, Some(1)).unwrap();
    let pr_300 = rows.iter().find(|r| r.id == 300).unwrap();
    assert!(pr_300.is_draft);
}

// ===== threads rollup projection tests (ADR 0016, query-time computation) =====

#[test]
fn threads_is_none_when_pull_request_has_no_review_threads() {
    let conn = fresh_db();
    seed_fixture(&conn);

    // The fixture seeds no `review_threads` rows, so the LEFT JOIN misses
    // for every PR and `COALESCE(tb.total, 0) = 0` trips `threads = None`.
    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    assert!(rows[0].threads.is_none());
}

#[test]
fn threads_buckets_match_single_account_involvement() {
    // Single-account view (alice). Five threads on PR 100:
    //   t1: unresolved, alice commented      -> unresolved_involved
    //   t2: unresolved, alice commented      -> unresolved_involved
    //   t3: unresolved, only bob commented   -> unresolved_uninvolved
    //   t4: resolved,   alice commented      -> resolved_involved
    //   t5: resolved,   no comments at all   -> resolved_uninvolved
    let conn = fresh_db();
    seed_fixture(&conn);
    conn.execute_batch(
        r#"
        INSERT INTO review_threads (id, pull_request_id, is_resolved, is_outdated, node_id) VALUES
            (1001, 100, 0, 0, 'RT_1'),
            (1002, 100, 0, 0, 'RT_2'),
            (1003, 100, 0, 0, 'RT_3'),
            (1004, 100, 1, 0, 'RT_4'),
            (1005, 100, 1, 0, 'RT_5');
        INSERT INTO review_comments (id, review_thread_id, author_login, body, created_at) VALUES
            (2001, 1001, 'alice', 'a', 1),
            (2002, 1002, 'alice', 'b', 2),
            (2003, 1003, 'bob',   'c', 3),
            (2004, 1004, 'alice', 'd', 4);
        "#,
    )
    .unwrap();

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    let threads = pr.threads.as_ref().expect("threads populated");
    assert_eq!(threads.total, 5);
    assert_eq!(threads.unresolved_involved, 2);
    assert_eq!(threads.unresolved_uninvolved, 1);
    assert_eq!(threads.resolved_involved, 1);
    assert_eq!(threads.resolved_uninvolved, 1);
}

#[test]
fn threads_buckets_union_involvement_across_every_account() {
    // No account filter (account_id = None): the involvement test admits any
    // tracked account's login. A thread with a comment by bob is involved
    // when bob's account is in the in-scope set; same for alice.
    let conn = fresh_db();
    seed_fixture(&conn);
    conn.execute_batch(
        r#"
        INSERT INTO review_threads (id, pull_request_id, is_resolved, is_outdated, node_id) VALUES
            (1001, 100, 0, 0, 'RT_un_1'),
            (1002, 100, 0, 0, 'RT_un_2'),
            (1003, 100, 0, 0, 'RT_un_3');
        INSERT INTO review_comments (id, review_thread_id, author_login, body, created_at) VALUES
            (2001, 1001, 'alice',   'a', 1),
            (2002, 1002, 'bob',     'b', 2),
            (2003, 1003, 'stranger','c', 3);
        "#,
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Watching, DashboardSort::Updated, None).unwrap();
    // Watching union (ADR 0016 dedupe-and-merge): PR 100 surfaces once with
    // every involved relation owner folded into `account_ids`.
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    let threads = pr.threads.as_ref().expect("threads populated");
    assert_eq!(threads.total, 3);
    assert_eq!(
        threads.unresolved_involved, 2,
        "union admits both alice and bob; only stranger isn't tracked"
    );
    assert_eq!(threads.unresolved_uninvolved, 1, "stranger's thread");
}

#[test]
fn threads_buckets_uninvolved_when_no_in_scope_account_matches_comment_author() {
    // Single-account filter on account 1 (login=alice). The comment is
    // authored by a name no tracked account uses (e.g. 'stranger'). The
    // EXISTS subquery in the involvement test misses for every thread, so
    // every thread counts as uninvolved. This is the practical "empty
    // in-scope" shape: in_scope contains alice, but no comment author
    // matches her login - same outcome as zero accounts in scope.
    let conn = fresh_db();
    seed_fixture(&conn);
    conn.execute_batch(
        r#"
        INSERT INTO review_threads (id, pull_request_id, is_resolved, is_outdated, node_id) VALUES
            (1001, 100, 0, 0, 'RT_z1'),
            (1002, 100, 0, 0, 'RT_z2');
        INSERT INTO review_comments (id, review_thread_id, author_login, body, created_at) VALUES
            (2001, 1001, 'stranger', 'x', 1),
            (2002, 1002, 'someone-else', 'y', 2);
        "#,
    )
    .unwrap();

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    let threads = pr.threads.as_ref().expect("threads populated");
    assert_eq!(threads.total, 2);
    assert_eq!(
        threads.unresolved_involved, 0,
        "no tracked account authored either thread's comments"
    );
    assert_eq!(threads.unresolved_uninvolved, 2);
}

#[test]
fn threads_buckets_uninvolved_when_active_account_login_doesnt_match_comment_author() {
    // Single-account filter on bob. Comment is by alice. Active account
    // login mismatch -> involvement false; thread counts as uninvolved.
    let conn = fresh_db();
    seed_fixture(&conn);
    conn.execute_batch(
        r#"
        INSERT INTO review_threads (id, pull_request_id, is_resolved, is_outdated, node_id) VALUES
            (1001, 100, 0, 0, 'RT_x');
        INSERT INTO review_comments (id, review_thread_id, author_login, body, created_at) VALUES
            (2001, 1001, 'alice', 'a', 1);
        "#,
    )
    .unwrap();

    // The fixture's (2, 100) relation flags `is_review_requested = 1` but
    // not `is_involved`. Promote it so the Watching view returns PR 100
    // under account 2.
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET is_involved = 1
          WHERE account_id = 2 AND pull_request_id = 100",
        [],
    )
    .unwrap();
    let rows = list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(2),
    )
    .unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    let threads = pr.threads.as_ref().expect("threads populated");
    assert_eq!(
        threads.unresolved_involved, 0,
        "bob isn't the comment author; involvement misses"
    );
    assert_eq!(threads.unresolved_uninvolved, 1);
}

#[test]
fn threads_buckets_all_resolved_zeros_the_unresolved_columns() {
    let conn = fresh_db();
    seed_fixture(&conn);
    conn.execute_batch(
        r#"
        INSERT INTO review_threads (id, pull_request_id, is_resolved, is_outdated, node_id) VALUES
            (1001, 100, 1, 0, 'RT_r1'),
            (1002, 100, 1, 0, 'RT_r2');
        INSERT INTO review_comments (id, review_thread_id, author_login, body, created_at) VALUES
            (2001, 1001, 'alice', 'a', 1);
        "#,
    )
    .unwrap();

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    let threads = pr.threads.as_ref().expect("threads populated");
    assert_eq!(threads.total, 2);
    assert_eq!(threads.unresolved_involved, 0);
    assert_eq!(threads.unresolved_uninvolved, 0);
    assert_eq!(threads.resolved_involved, 1);
    assert_eq!(threads.resolved_uninvolved, 1);
}

#[test]
fn threads_buckets_outdated_threads_still_count_in_their_bucket() {
    // ADR 0012 preserved: outdated threads sort by (is_resolved, involved)
    // like any other thread; they're no longer carved out.
    let conn = fresh_db();
    seed_fixture(&conn);
    conn.execute_batch(
        r#"
        INSERT INTO review_threads (id, pull_request_id, is_resolved, is_outdated, node_id) VALUES
            (1001, 100, 0, 1, 'RT_o1'),
            (1002, 100, 1, 1, 'RT_o2');
        INSERT INTO review_comments (id, review_thread_id, author_login, body, created_at) VALUES
            (2001, 1001, 'alice', 'a', 1);
        "#,
    )
    .unwrap();

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    let threads = pr.threads.as_ref().expect("threads populated");
    assert_eq!(threads.total, 2, "outdated still in the denominator");
    assert_eq!(threads.unresolved_involved, 1);
    assert_eq!(threads.resolved_uninvolved, 1);
}

#[test]
fn reviewer_hydration_deduplicates_multiple_reviews_per_login() {
    // Seed: one PR with three submitted reviews from the same login at
    // ascending timestamps (COMMENTED -> APPROVED -> CHANGES_REQUESTED) plus
    // a pending request for a second login. Expect exactly two reviewer
    // entries: the latest state for the dup login, plus the pending.
    let conn = fresh_db();
    conn.execute_batch(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'alice-acct', 'github.com', 'alice', 0);

        INSERT INTO repos (id, account_id, owner, name, visibility, is_team_tracked) VALUES
            (10, 1, 'alice', 'web', 'public', 0);

        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, latest_status_change_at, base_ref, head_ref,
             mergeable, review_decision, additions, deletions, changed_files,
             ci_state, ci_total, ci_passing) VALUES
            (100, 10, 1, 'web/#1', 'open', 0, 'alice', 0, 1000, 1000, 'main', 'feat-a',
             NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);

        INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at) VALUES
            (1, 100, 1, 0, 1, 0);

        -- bob submits three reviews on PR 100 at ascending timestamps.
        INSERT INTO reviews (id, pull_request_id, reviewer_login, state, submitted_at) VALUES
            (9001, 100, 'bob', 'COMMENTED',         500),
            (9002, 100, 'bob', 'APPROVED',          600),
            (9003, 100, 'bob', 'CHANGES_REQUESTED', 700);

        -- carol is requested but has not submitted.
        INSERT INTO requested_reviewers (id, pull_request_id, login, reviewer_type) VALUES
            (8001, 100, 'carol', 'user');
        "#,
    )
    .unwrap();

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = &rows[0];
    assert_eq!(pr.reviewers.len(), 2, "expected one bob + one carol");
    let bob = pr.reviewers.iter().find(|r| r.login == "bob").unwrap();
    assert_eq!(
        bob.state,
        ReviewerState::ChangesRequested,
        "latest submitted_at should win"
    );
    let carol = pr.reviewers.iter().find(|r| r.login == "carol").unwrap();
    assert_eq!(carol.state, ReviewerState::Pending);
}

#[test]
fn reviewer_hydration_state_priority_tiebreak_on_equal_submitted_at() {
    // Two reviews from the same login at the same `submitted_at`. The
    // tie-break order is CHANGES_REQUESTED > APPROVED > COMMENTED > DISMISSED
    // > PENDING, so CHANGES_REQUESTED must win regardless of insertion order.
    let conn = fresh_db();
    conn.execute_batch(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'alice-acct', 'github.com', 'alice', 0);

        INSERT INTO repos (id, account_id, owner, name, visibility, is_team_tracked) VALUES
            (10, 1, 'alice', 'web', 'public', 0);

        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, latest_status_change_at, base_ref, head_ref,
             mergeable, review_decision, additions, deletions, changed_files,
             ci_state, ci_total, ci_passing) VALUES
            (100, 10, 1, 'web/#1', 'open', 0, 'alice', 0, 1000, 1000, 'main', 'feat-a',
             NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);

        INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at) VALUES
            (1, 100, 1, 0, 1, 0);

        -- Same login + same submitted_at; APPROVED inserted before
        -- CHANGES_REQUESTED so a naive query would pick APPROVED.
        INSERT INTO reviews (id, pull_request_id, reviewer_login, state, submitted_at) VALUES
            (9001, 100, 'bob', 'APPROVED',          500),
            (9002, 100, 'bob', 'CHANGES_REQUESTED', 500);
        "#,
    )
    .unwrap();

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = &rows[0];
    assert_eq!(pr.reviewers.len(), 1);
    assert_eq!(pr.reviewers[0].state, ReviewerState::ChangesRequested);
}

#[test]
fn reviewer_hydration_drops_login_whose_only_state_is_dismissed_even_when_requested() {
    // A login with a single DISMISSED review _and_ a pending request must
    // surface as neither a reviewer entry nor a pending entry: the submitted
    // review wins the slot, and DISMISSED maps to None.
    let conn = fresh_db();
    conn.execute_batch(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'alice-acct', 'github.com', 'alice', 0);

        INSERT INTO repos (id, account_id, owner, name, visibility, is_team_tracked) VALUES
            (10, 1, 'alice', 'web', 'public', 0);

        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, latest_status_change_at, base_ref, head_ref,
             mergeable, review_decision, additions, deletions, changed_files,
             ci_state, ci_total, ci_passing) VALUES
            (100, 10, 1, 'web/#1', 'open', 0, 'alice', 0, 1000, 1000, 'main', 'feat-a',
             NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);

        INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at) VALUES
            (1, 100, 1, 0, 1, 0);

        INSERT INTO reviews (id, pull_request_id, reviewer_login, state, submitted_at) VALUES
            (9001, 100, 'frank', 'DISMISSED', 500);

        INSERT INTO requested_reviewers (id, pull_request_id, login, reviewer_type) VALUES
            (8001, 100, 'frank', 'user');
        "#,
    )
    .unwrap();

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = &rows[0];
    assert!(
        pr.reviewers.iter().all(|r| r.login != "frank"),
        "DISMISSED submitted review must suppress the pending re-entry"
    );
}

#[test]
fn author_and_reviewer_avatar_urls_left_join_users_table() {
    // ADR 0013: dashboard list reads `avatar_url` for the PR author and each
    // reviewer via `LEFT JOIN users`. Missing rows produce `None`, which the
    // frontend renders as the initials fallback.
    let conn = fresh_db();
    seed_fixture(&conn);

    // Seed two users: the PR author + one reviewer. The remaining reviewers
    // intentionally have no users row to assert the `None` branch.
    conn.execute_batch(
        r#"
        INSERT INTO users (login, avatar_url, last_seen_at) VALUES
            ('alice', 'https://avatars/alice.png', 0),
            ('bob',   'https://avatars/bob.png',   0);
        "#,
    )
    .unwrap();

    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    assert_eq!(pr.author_login, "alice");
    assert_eq!(
        pr.author_avatar_url.as_deref(),
        Some("https://avatars/alice.png"),
    );

    let bob = pr.reviewers.iter().find(|r| r.login == "bob").unwrap();
    assert_eq!(bob.avatar_url.as_deref(), Some("https://avatars/bob.png"));

    // `carol` was a reviewer but no users row exists → avatar_url is None.
    let carol = pr.reviewers.iter().find(|r| r.login == "carol").unwrap();
    assert!(carol.avatar_url.is_none());
}

// ===== M4-D Stale / NeedsMe sort + chip-filter composition tests =====

/// Seed a small Watching-view fixture for alice (account 1) with varied
/// `updated_at` so the Stale sort produces a deterministic order. Returns the
/// connection.
fn seed_stale_fixture(conn: &Connection) {
    conn.execute_batch(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'alice-acct', 'github.com', 'alice', 0);

        INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
            (10, 1, 'alice', 'web', 'public');

        -- updated_at varied: PR 101 oldest, PR 103 newest. The Stale sort is
        -- ASC, so the row order must be 101, 102, 103.
        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, base_ref, head_ref) VALUES
            (101, 10, 1, 'old',    'open', 0, 'bob', 0, 100, 'main', 'a'),
            (102, 10, 2, 'middle', 'open', 0, 'bob', 0, 500, 'main', 'b'),
            (103, 10, 3, 'new',    'open', 0, 'bob', 0, 900, 'main', 'c');

        INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at) VALUES
            (1, 101, 0, 0, 1, 0),
            (1, 102, 0, 0, 1, 0),
            (1, 103, 0, 0, 1, 0);
        "#,
    )
    .unwrap();
}

#[test]
fn stale_sort_returns_oldest_updated_at_first() {
    let conn = fresh_db();
    seed_stale_fixture(&conn);

    let rows = inner_list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Stale,
        Some(1),
        &[],
    )
    .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![101, 102, 103],
        "Stale sort must order rows oldest-activity-first"
    );
}

/// Seed a fixture for the NeedsMe sort. PR 200 has `needs_attention = 1`;
/// PR 201 and 202 don't. Among the two non-attention rows, 202 has the more
/// recent `latest_status_change_at` and must tie-break first.
fn seed_needs_me_fixture(conn: &Connection) {
    conn.execute_batch(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'alice-acct', 'github.com', 'alice', 0);

        INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
            (10, 1, 'alice', 'web', 'public');

        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, latest_status_change_at, base_ref, head_ref) VALUES
            (200, 10, 1, 'attn',     'open', 0, 'bob', 0, 100, 100,  'main', 'a'),
            (201, 10, 2, 'no-attn1', 'open', 0, 'bob', 0, 200, 200,  'main', 'b'),
            (202, 10, 3, 'no-attn2', 'open', 0, 'bob', 0, 300, 1000, 'main', 'c');

        INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at, needs_attention) VALUES
            (1, 200, 0, 0, 1, 0, 1),
            (1, 201, 0, 0, 1, 0, 0),
            (1, 202, 0, 0, 1, 0, 0);
        "#,
    )
    .unwrap();
}

#[test]
fn needs_me_sort_surfaces_attention_rows_first_then_breaks_ties_by_status_change() {
    let conn = fresh_db();
    seed_needs_me_fixture(&conn);

    let rows = inner_list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::NeedsMe,
        Some(1),
        &[],
    )
    .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![200, 202, 201],
        "NeedsMe sort puts needs_attention=1 first; ties break by COALESCE(lsc, updated_at) DESC"
    );
}

/// Seed a Watching-view fixture covering every chip predicate independently.
fn seed_chip_fixture(conn: &Connection) {
    // `now - 604800` is the "exactly 7 days ago" boundary; subtract a few
    // extra seconds so the Stale chip's strict `>` predicate fires.
    let stale_updated_at = "(strftime('%s','now') - 800000)";
    let fresh_updated_at = "strftime('%s','now')";
    conn.execute_batch(&format!(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'alice-acct', 'github.com', 'alice', 0);

        INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
            (10, 1, 'alice', 'web', 'public');

        -- PR 300: draft only.
        -- PR 301: ci_state = FAILURE.
        -- PR 302: stale (old updated_at).
        -- PR 303: unresolved threads (ADR 0016: seeded via review_threads).
        -- PR 304: needs_attention precomputed on the relation row.
        -- PR 305: nothing - control row, matches no chip.
        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, base_ref, head_ref, ci_state) VALUES
            (300, 10, 1, 'draft',   'open', 1, 'bob', 0, {fresh_updated_at}, 'main', 'a', NULL),
            (301, 10, 2, 'ci',      'open', 0, 'bob', 0, {fresh_updated_at}, 'main', 'b', 'FAILURE'),
            (302, 10, 3, 'stale',   'open', 0, 'bob', 0, {stale_updated_at}, 'main', 'c', NULL),
            (303, 10, 4, 'threads', 'open', 0, 'bob', 0, {fresh_updated_at}, 'main', 'd', NULL),
            (304, 10, 5, 'attn',    'open', 0, 'bob', 0, {fresh_updated_at}, 'main', 'e', NULL),
            (305, 10, 6, 'control', 'open', 0, 'bob', 0, {fresh_updated_at}, 'main', 'f', NULL);

        INSERT INTO review_threads (id, pull_request_id, is_resolved, is_outdated, node_id) VALUES
            (4000, 303, 0, 0, 'RT_chip_303');

        INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at, needs_attention) VALUES
            (1, 300, 0, 0, 1, 0, 0),
            (1, 301, 0, 0, 1, 0, 0),
            (1, 302, 0, 0, 1, 0, 0),
            (1, 303, 0, 0, 1, 0, 0),
            (1, 304, 0, 0, 1, 0, 1),
            (1, 305, 0, 0, 1, 0, 0);
        "#,
    ))
    .unwrap();
}

#[test]
fn drafts_chip_narrows_results_to_draft_prs() {
    let conn = fresh_db();
    seed_chip_fixture(&conn);

    let rows = inner_list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
        &[ChipKey::Drafts],
    )
    .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![300],
        "Drafts chip narrows to PR 300 only"
    );
}

#[test]
fn ci_failing_chip_narrows_results_to_failing_ci() {
    let conn = fresh_db();
    seed_chip_fixture(&conn);

    let rows = inner_list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
        &[ChipKey::CiFailing],
    )
    .unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![301]);
}

#[test]
fn stale_chip_narrows_to_prs_older_than_seven_days() {
    let conn = fresh_db();
    seed_chip_fixture(&conn);

    let rows = inner_list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
        &[ChipKey::Stale],
    )
    .unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![302]);
}

#[test]
fn unresolved_threads_chip_narrows_to_prs_with_unresolved_thread_counts() {
    let conn = fresh_db();
    seed_chip_fixture(&conn);

    let rows = inner_list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
        &[ChipKey::UnresolvedThreads],
    )
    .unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![303]);
}

#[test]
fn needs_attention_chip_narrows_to_relation_flagged_rows() {
    let conn = fresh_db();
    seed_chip_fixture(&conn);

    let rows = inner_list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
        &[ChipKey::NeedsAttention],
    )
    .unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![304]);
}

#[test]
fn two_active_chips_intersect_via_and_composition() {
    // Add a draft PR with failing CI so the AND-intersection is non-empty.
    let conn = fresh_db();
    seed_chip_fixture(&conn);
    conn.execute_batch(
        r#"
        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, base_ref, head_ref, ci_state) VALUES
            (306, 10, 7, 'draft+ci', 'open', 1, 'bob', 0,
             strftime('%s','now'), 'main', 'g', 'ERROR');
        INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at) VALUES
            (1, 306, 0, 0, 1, 0);
        "#,
    )
    .unwrap();

    let rows = inner_list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
        &[ChipKey::Drafts, ChipKey::CiFailing],
    )
    .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![306],
        "AND composition narrows to PR 306 alone (draft AND failing CI)"
    );
}

#[test]
fn empty_chip_set_preserves_baseline_view_results() {
    let conn = fresh_db();
    seed_chip_fixture(&conn);

    let baseline = inner_list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
        &[],
    )
    .unwrap();
    assert_eq!(baseline.len(), 6, "no chips means no filter");
}

// ===== triage projection tests (M4-C) =====

/// `unread` defaults to true when no relation row carries a `read_at`
/// watermark (the row is fresh from discovery or the viewer has never opened
/// the PR).
#[test]
fn unread_defaults_to_true_when_read_at_is_null() {
    let conn = fresh_db();
    seed_fixture(&conn);
    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    assert!(pr.unread, "fresh relation row reads as unread");
}

/// `unread` clears once `read_at` is set and the PR's `updated_at` hasn't
/// advanced past the captured `read_pr_updated_at` watermark.
#[test]
fn unread_clears_when_read_at_is_after_pr_updated_at() {
    let conn = fresh_db();
    seed_fixture(&conn);
    // PR 100 has updated_at = 950. Set both watermarks to >= 950 so the
    // derivation treats the open as fresh.
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET read_at = 1000, read_pr_updated_at = 950
          WHERE account_id = 1 AND pull_request_id = 100",
        [],
    )
    .unwrap();
    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    assert!(!pr.unread, "read watermark covers PR.updated_at");
}

/// `unread` flips back to true when sync bumps `pull_requests.updated_at`
/// past the captured `read_pr_updated_at`.
#[test]
fn unread_flips_back_when_pr_updated_at_overtakes_read_watermark() {
    let conn = fresh_db();
    seed_fixture(&conn);
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET read_at = 1000, read_pr_updated_at = 900
          WHERE account_id = 1 AND pull_request_id = 100",
        [],
    )
    .unwrap();
    // PR 100 starts at updated_at = 950; bump it past the read watermark.
    conn.execute(
        "UPDATE pull_requests SET updated_at = 1100 WHERE id = 100",
        [],
    )
    .unwrap();
    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    assert!(
        pr.unread,
        "updated_at > read_pr_updated_at flips back to unread"
    );
}

/// `needs_attention` projects the relation column verbatim through COALESCE.
#[test]
fn needs_attention_projects_relation_column() {
    let conn = fresh_db();
    seed_fixture(&conn);
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET needs_attention = 1
          WHERE account_id = 1 AND pull_request_id = 100",
        [],
    )
    .unwrap();
    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    assert!(pr.needs_attention);
}

/// `needs_attention` defaults to false when the relation row carries 0.
#[test]
fn needs_attention_defaults_to_false() {
    let conn = fresh_db();
    seed_fixture(&conn);
    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    assert!(!pr.needs_attention);
}

/// `mentioned_count_unread` projects the relation column verbatim.
#[test]
fn mentioned_count_unread_projects_relation_column() {
    let conn = fresh_db();
    seed_fixture(&conn);
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET mentioned_count_unread = 4
          WHERE account_id = 1 AND pull_request_id = 100",
        [],
    )
    .unwrap();
    let rows = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    assert_eq!(pr.mentioned_count_unread, 4);
}

/// Team view without an account filter short-circuits the relation join.
/// Every row defaults to `unread = true`, `needs_attention = false`,
/// `mentioned_count_unread = 0`.
#[test]
fn team_view_union_defaults_triage_fields() {
    let conn = fresh_db();
    seed_fixture(&conn);
    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, None).unwrap();
    assert!(!rows.is_empty());
    for pr in rows.iter() {
        assert!(pr.unread, "PR {} should default to unread", pr.id);
        assert!(
            !pr.needs_attention,
            "PR {} should default to no attention",
            pr.id
        );
        assert_eq!(pr.mentioned_count_unread, 0);
    }
}

/// Team view scoped to an account defaults the triage projections to false /
/// 0 for PRs whose owning-account has no relation row. In the seeded fixture
/// PR 300 sits in `alice/api` (team-tracked, owned by account 1) and no
/// relations row references it - the LEFT JOIN misses and COALESCE trips.
#[test]
fn team_view_account_scoped_defaults_when_no_relation_row() {
    let conn = fresh_db();
    seed_fixture(&conn);
    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, Some(1)).unwrap();
    let pr_300 = rows.iter().find(|r| r.id == 300).unwrap();
    assert!(pr_300.unread, "missing relation row reads as unread");
    assert!(!pr_300.needs_attention);
    assert_eq!(pr_300.mentioned_count_unread, 0);
}

/// Team view scoped to an account reads the triage state from the matching
/// relation row when one exists. The fixture's PR 100 sits in `alice/web`
/// (not team-tracked) so it doesn't surface in the Team view; instead seed a
/// fresh team-tracked PR that has a populated relation row for the same
/// account.
#[test]
fn team_view_account_scoped_reads_relation_row_when_present() {
    let conn = fresh_db();
    seed_fixture(&conn);
    // Promote alice/web (repo 10) to team-tracked so PR 100's existing
    // relation row surfaces via the Team view, then populate the triage
    // columns on that row.
    conn.execute("UPDATE repos SET is_team_tracked = 1 WHERE id = 10", [])
        .unwrap();
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET needs_attention = 1,
                mentioned_count_unread = 3
          WHERE account_id = 1 AND pull_request_id = 100",
        [],
    )
    .unwrap();
    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, Some(1)).unwrap();
    let pr = rows
        .iter()
        .find(|r| r.id == 100)
        .expect("PR 100 surfaces in account-scoped Team view");
    assert!(pr.needs_attention);
    assert_eq!(pr.mentioned_count_unread, 3);
}

// ===== cross-host (login collision) is_you tests (issue #169) =====
//
// Two accounts share login `ada` on different hosts. The PR is owned by
// account 1 (github.com). When the dashboard surfaces the PR under account
// 2's relation row (a deliberate edge case forced via direct INSERT - the
// sync flow doesn't normally create cross-host relations), the `is_you`
// reviewer marker must NOT flip for reviewers whose login matches account
// 2's login string but whose identity lives on the PR's host.

/// Build a fixture with one PR owned by account 1 (github.com), where a
/// second account shares the login on a different host. Both accounts get a
/// relation row to the same PR so the union path can return either.
fn seed_cross_host_login_collision_fixture(conn: &Connection) {
    conn.execute_batch(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'gh-acct',  'github.com',       'ada', 0),
            (2, 'ghe-acct', 'github.acme.corp', 'ada', 0);

        INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
            (10, 1, 'ada', 'web', 'public');

        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, latest_status_change_at, base_ref, head_ref) VALUES
            (100, 10, 1, 'web/#1', 'open', 0, 'someone-else', 0, 1000, 1000,
             'main', 'feat-a');

        INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at) VALUES
            (1, 100, 0, 0, 1, 0),
            (2, 100, 0, 0, 1, 0);

        INSERT INTO reviews (id, pull_request_id, reviewer_login, state, submitted_at) VALUES
            (9001, 100, 'ada', 'APPROVED', 500);
        "#,
    )
    .unwrap();
}

#[test]
fn is_you_does_not_flip_cross_host_when_viewer_login_matches() {
    let conn = fresh_db();
    seed_cross_host_login_collision_fixture(&conn);

    // Query under account 2 (ghe). The PR is on github.com - reviewer 'ada'
    // is the github.com 'ada', not account 2's ghe identity. is_you must be
    // false.
    let rows = list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(2),
    )
    .unwrap();
    let pr = rows
        .iter()
        .find(|r| r.id == 100)
        .expect("PR 100 surfaces under account 2's relation row");
    let ada = pr
        .reviewers
        .iter()
        .find(|r| r.login == "ada")
        .expect("ada is a reviewer");
    assert!(
        !ada.is_you,
        "account 2 lives on a different host; the github.com 'ada' is not its identity"
    );
}

#[test]
fn is_you_still_flips_same_host_when_viewer_login_matches() {
    let conn = fresh_db();
    seed_cross_host_login_collision_fixture(&conn);

    // Regression guard: under account 1 (github.com - the PR's host),
    // reviewer 'ada' IS the viewer.
    let rows = list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let pr = rows
        .iter()
        .find(|r| r.id == 100)
        .expect("PR 100 surfaces under account 1's relation row");
    let ada = pr
        .reviewers
        .iter()
        .find(|r| r.login == "ada")
        .expect("ada is a reviewer");
    assert!(
        ada.is_you,
        "account 1 IS the github.com 'ada' identity (same login, same host)"
    );
}

// ===== ADR 0016: unified-mode dedupe and merge (issue #167) =====
//
// Two accounts (Alice + Bob) share a PR via different relation types. The
// unified path GROUPs by `pr.id`, merges triage signals (`unread = MAX`,
// `needs_attention = MAX`, `mentioned_count_unread = SUM`), and surfaces a
// single row whose `account_ids` carries every relation owner.

/// Build a two-account fixture: alice authored PR 100, bob review-requested
/// on the same PR. Both accounts live on github.com (same host) so the
/// reviewer-identity tests below can flip `is_you` against either login.
fn seed_two_account_shared_pr_fixture(conn: &Connection) {
    conn.execute_batch(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'alice-acct', 'github.com', 'alice', 0),
            (2, 'bob-acct',   'github.com', 'bob',   0);

        INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
            (10, 1, 'alice', 'web', 'public');

        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, latest_status_change_at, base_ref, head_ref) VALUES
            (100, 10, 1, 'shared', 'open', 0, 'someone-else', 0, 1000, 1000,
             'main', 'feat-a');

        INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at) VALUES
            (1, 100, 1, 0, 0, 0),
            (2, 100, 0, 1, 0, 0);
        "#,
    )
    .unwrap();
}

#[test]
fn union_dedupes_pr_with_relations_under_two_accounts_to_one_row() {
    // PR 100 has Authored relation (account 1) and Review-Requested relation
    // (account 2). The unified Watching/Authored/Assigned views each group
    // by `pr.id` so the PR surfaces once. `account_ids` carries every
    // relation owner sorted ascending.
    let conn = fresh_db();
    seed_two_account_shared_pr_fixture(&conn);

    let authored =
        list_pull_requests(&conn, DashboardView::Authored, DashboardSort::Updated, None).unwrap();
    assert_eq!(authored.len(), 1, "Authored union surfaces PR 100 once");
    assert_eq!(authored[0].id, 100);
    assert_eq!(
        authored[0].account_ids,
        vec![1, 2],
        "account_ids must include every relation owner, sorted ascending"
    );

    let assigned =
        list_pull_requests(&conn, DashboardView::Assigned, DashboardSort::Updated, None).unwrap();
    assert_eq!(assigned.len(), 1, "Assigned union surfaces PR 100 once");
    assert_eq!(assigned[0].account_ids, vec![1, 2]);
}

#[test]
fn union_merges_unread_via_max_across_relation_owners() {
    // Two relation rows for the same PR: alice has read it (read_at set);
    // bob hasn't. MAX(unread) = 1; the row reads unread.
    let conn = fresh_db();
    seed_two_account_shared_pr_fixture(&conn);
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET read_at = 1100, read_pr_updated_at = 1000
          WHERE account_id = 1 AND pull_request_id = 100",
        [],
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Authored, DashboardSort::Updated, None).unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    assert!(
        pr.unread,
        "MAX(unread) = 1 when any in-scope account is unread"
    );

    // Inverse: both accounts read -> row is read.
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET read_at = 1100, read_pr_updated_at = 1000
          WHERE pull_request_id = 100",
        [],
    )
    .unwrap();
    let rows =
        list_pull_requests(&conn, DashboardView::Authored, DashboardSort::Updated, None).unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    assert!(
        !pr.unread,
        "MAX(unread) = 0 when every in-scope account is read"
    );
}

#[test]
fn union_merges_needs_attention_via_max_across_relation_owners() {
    // Bob's relation flags needs_attention; alice's doesn't.
    // MAX(needs_attention) = 1.
    let conn = fresh_db();
    seed_two_account_shared_pr_fixture(&conn);
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET needs_attention = 1
          WHERE account_id = 2 AND pull_request_id = 100",
        [],
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Authored, DashboardSort::Updated, None).unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    assert!(pr.needs_attention);
}

#[test]
fn union_merges_mentioned_count_unread_via_sum_across_relation_owners() {
    // Alice's relation has 2 unread mentions; bob's has 3. SUM = 5.
    let conn = fresh_db();
    seed_two_account_shared_pr_fixture(&conn);
    conn.execute_batch(
        "UPDATE pull_request_viewer_relations
            SET mentioned_count_unread = 2
          WHERE account_id = 1 AND pull_request_id = 100;
         UPDATE pull_request_viewer_relations
            SET mentioned_count_unread = 3
          WHERE account_id = 2 AND pull_request_id = 100;",
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Authored, DashboardSort::Updated, None).unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    assert_eq!(
        pr.mentioned_count_unread, 5,
        "SUM aggregates mentions across every in-scope relation owner"
    );
}

#[test]
fn union_failure_isolation_drops_one_accounts_relations_other_account_still_surfaces_pr() {
    // ADR 0016 ("Failure isolation"). PR 100 originally has relations under
    // accounts 1 and 2. Simulate account 1 failing mid-sync (its relation
    // row pruned). Account 2's relation row keeps the PR visible in the
    // union. The view filter EXISTS still admits the PR via account 2.
    let conn = fresh_db();
    seed_two_account_shared_pr_fixture(&conn);
    conn.execute(
        "DELETE FROM pull_request_viewer_relations
          WHERE account_id = 1 AND pull_request_id = 100",
        [],
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Assigned, DashboardSort::Updated, None).unwrap();
    let pr = rows
        .iter()
        .find(|r| r.id == 100)
        .expect("account 2's Review-Requested relation surfaces the PR even with account 1 pruned");
    assert_eq!(
        pr.account_ids,
        vec![2],
        "only the surviving relation owner appears in account_ids"
    );
}

#[test]
fn union_reviewer_is_you_matches_any_in_scope_account_login() {
    // Reviewer login matches account 2's login (`bob`) but not account 1's
    // (`alice`). The PR's host is github.com (matches both accounts). With
    // the unified-mode `is_you` scan testing against every account_id in the
    // row, `is_you` flips for the `bob` reviewer entry.
    let conn = fresh_db();
    seed_two_account_shared_pr_fixture(&conn);
    conn.execute_batch(
        "INSERT INTO reviews (id, pull_request_id, reviewer_login, state, submitted_at)
            VALUES (9001, 100, 'bob', 'COMMENTED', 500);",
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Authored, DashboardSort::Updated, None).unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    let bob = pr
        .reviewers
        .iter()
        .find(|r| r.login == "bob")
        .expect("bob is the submitted reviewer");
    assert!(
        bob.is_you,
        "union-mode is_you must flip for any account_id whose (login, host) matches the reviewer"
    );
}

#[test]
fn union_reviewer_is_you_stays_false_when_no_in_scope_account_matches() {
    // Reviewer is a third party (`carol`). Neither alice nor bob match.
    let conn = fresh_db();
    seed_two_account_shared_pr_fixture(&conn);
    conn.execute_batch(
        "INSERT INTO reviews (id, pull_request_id, reviewer_login, state, submitted_at)
            VALUES (9001, 100, 'carol', 'COMMENTED', 500);",
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Authored, DashboardSort::Updated, None).unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    let carol = pr.reviewers.iter().find(|r| r.login == "carol").unwrap();
    assert!(!carol.is_you);
}

#[test]
fn union_team_view_surfaces_team_repo_pr_without_relations() {
    // A PR in a team-tracked repo with no relation rows still surfaces in
    // the Team union view; the view filter is `repos.is_team_tracked = 1`,
    // not the relations table. `account_ids` is empty for such rows.
    let conn = fresh_db();
    seed_fixture(&conn);
    // PR 300 sits in alice/api (team-tracked) with no relation rows seeded.
    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, None).unwrap();
    let pr_300 = rows.iter().find(|r| r.id == 300).unwrap();
    assert!(
        pr_300.account_ids.is_empty(),
        "Team-view PR with no relations carries an empty account_ids list"
    );
    assert!(pr_300.unread, "no relation -> defaults to unread");
}

#[test]
fn union_team_view_merges_relations_when_present() {
    // PR 400 (bob/cli, team-tracked) has relations under both accounts. The
    // union Team view aggregates over both rows: account_ids = [1, 2] and
    // the triage merge applies. Seed needs_attention on alice's relation so
    // the MAX is non-zero.
    let conn = fresh_db();
    seed_fixture(&conn);
    conn.execute_batch(
        "UPDATE pull_request_viewer_relations
            SET needs_attention = 1, mentioned_count_unread = 2
          WHERE account_id = 1 AND pull_request_id = 400;
         UPDATE pull_request_viewer_relations
            SET mentioned_count_unread = 4
          WHERE account_id = 2 AND pull_request_id = 400;",
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, None).unwrap();
    let pr_400 = rows.iter().find(|r| r.id == 400).unwrap();
    assert_eq!(pr_400.account_ids, vec![1, 2]);
    assert!(pr_400.needs_attention);
    assert_eq!(
        pr_400.mentioned_count_unread, 6,
        "SUM merges mentions across both relation owners"
    );
}

#[test]
fn union_url_uses_repo_owning_account_host_not_first_relation_owner() {
    // A PR owned by repo on github.com (account 1) but with a relation row
    // from a GHE account would still get the github.com URL. The URL host
    // comes from the repo's owning account, not the first account_ids entry.
    let conn = fresh_db();
    conn.execute_batch(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'gh',  'github.com',       'alice', 0),
            (2, 'ghe', 'github.acme.corp', 'alice', 0);

        INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
            (10, 1, 'alice', 'web', 'public');

        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, latest_status_change_at, base_ref, head_ref) VALUES
            (100, 10, 1, 'cross-host', 'open', 0, 'someone-else', 0, 1000, 1000,
             'main', 'feat-a');

        INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at) VALUES
            (1, 100, 1, 0, 0, 0),
            (2, 100, 1, 0, 0, 0);
        "#,
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Authored, DashboardSort::Updated, None).unwrap();
    let pr = rows.iter().find(|r| r.id == 100).unwrap();
    assert_eq!(
        pr.url, "https://github.com/alice/web/pull/1",
        "URL host comes from the repo's owning account, never from the union of relation owners"
    );
}

// ===== ADR 0018: archive exclusion + Archive view (issue #194) =====
//
// Default views (Authored / Assigned / Watching / Team) hide archived rows.
// The new `DashboardView::Archive` returns the inverse - only archived rows -
// and defaults to `archived_at DESC`. Unified scope respects the merged-row
// rule: a PR is archived in the union iff every relation owner has archived
// it.

/// Default views: an archived row drops out of every relation-based view
/// (Authored / Assigned / Watching). Single-account scope; the archived
/// relation row is filtered by the WHERE.
#[test]
fn default_views_hide_archived_rows_under_single_account_scope() {
    let conn = fresh_db();
    seed_fixture(&conn);
    // Archive alice's relations on PR 100 (authored) and PR 200 (watching),
    // and bob's review-requested relation on PR 100. The seeded fixture
    // doesn't have an `is_review_requested = 1` relation under account 1
    // beyond PR 400 (also archived below).
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET archived_at = strftime('%s', 'now')
          WHERE account_id = 1 AND pull_request_id IN (100, 200, 400)",
        [],
    )
    .unwrap();

    let authored = list_pull_requests(
        &conn,
        DashboardView::Authored,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    assert!(
        authored.iter().all(|r| r.id != 100),
        "alice's authored archive must drop PR 100; got {:?}",
        authored.iter().map(|r| r.id).collect::<Vec<_>>()
    );

    let watching = list_pull_requests(
        &conn,
        DashboardView::Watching,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    assert!(
        watching.iter().all(|r| r.id != 200),
        "PR 200 was archived; Watching must hide it"
    );

    let assigned = list_pull_requests(
        &conn,
        DashboardView::Assigned,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    assert!(
        assigned.iter().all(|r| r.id != 400),
        "alice's Assigned archive must drop PR 400"
    );
}

/// Team view (single-account scope): archived (account, PR) row hides the
/// per-account triage state but the PR still surfaces if the active account
/// owns the team-tracked repo. The archive predicate sits on the LEFT JOIN's
/// ON clause, so the relation row drops to NULL and the PR keeps surfacing
/// with default triage values - same shape as a Team-view PR the viewer has
/// no relation to. This is the closest read of ADR 0018 for Team's
/// relation-as-overlay model.
#[test]
fn team_view_archived_relation_collapses_to_default_triage_under_single_account_scope() {
    let conn = fresh_db();
    seed_fixture(&conn);
    // Promote alice/web (repo 10) to team-tracked so PR 100 (alice authored)
    // surfaces in Team. Then archive alice's relation on PR 100 and set
    // needs_attention - the team-view row should still appear but with the
    // default false / 0 triage values.
    conn.execute_batch(
        "UPDATE repos SET is_team_tracked = 1 WHERE id = 10;
         UPDATE pull_request_viewer_relations
            SET archived_at = strftime('%s', 'now'),
                needs_attention = 1,
                mentioned_count_unread = 3
          WHERE account_id = 1 AND pull_request_id = 100;",
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, Some(1)).unwrap();
    let pr_100 = rows
        .iter()
        .find(|r| r.id == 100)
        .expect("PR 100 still appears via the team-tracked repo");
    assert!(
        !pr_100.needs_attention,
        "archived relation must not leak its triage state"
    );
    assert_eq!(pr_100.mentioned_count_unread, 0);
}

/// Unified scope: partial-archive (one account archived, the other not)
/// keeps the PR visible. The `account_ids` reflects only the unarchived
/// relation owners so the merged row presents as "active" rather than
/// half-archived.
#[test]
fn default_views_keep_partial_archive_pr_visible_under_unified_scope() {
    let conn = fresh_db();
    seed_two_account_shared_pr_fixture(&conn);
    // Archive alice's relation only. Bob's relation stays unarchived.
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET archived_at = strftime('%s', 'now')
          WHERE account_id = 1 AND pull_request_id = 100",
        [],
    )
    .unwrap();

    // Authored union: alice's archive removes her relation from the merge.
    // The view-filter EXISTS only finds alice's `is_authored = 1` (now
    // archived), so PR 100 drops from Authored. This is intentional: under
    // ADR 0018, the EXISTS requires at least one unarchived relation with
    // the view flag.
    let authored =
        list_pull_requests(&conn, DashboardView::Authored, DashboardSort::Updated, None).unwrap();
    assert!(
        authored.iter().all(|r| r.id != 100),
        "no unarchived is_authored relation -> PR drops from Authored union"
    );

    // Assigned union: bob's review-requested relation stays unarchived, so
    // PR 100 surfaces. account_ids carries only bob (alice's archived
    // relation drops out of the merge).
    let assigned =
        list_pull_requests(&conn, DashboardView::Assigned, DashboardSort::Updated, None).unwrap();
    let pr = assigned
        .iter()
        .find(|r| r.id == 100)
        .expect("bob's unarchived review-request keeps the PR in Assigned");
    assert_eq!(
        pr.account_ids,
        vec![2],
        "merged row carries only unarchived relation owners; archived ones drop"
    );
}

/// Unified scope: every relation archived -> PR drops from every default
/// view (the merged row has no unarchived relation to keep it alive).
#[test]
fn default_views_hide_fully_archived_pr_under_unified_scope() {
    let conn = fresh_db();
    seed_two_account_shared_pr_fixture(&conn);
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET archived_at = strftime('%s', 'now')
          WHERE pull_request_id = 100",
        [],
    )
    .unwrap();

    for view in [
        DashboardView::Authored,
        DashboardView::Assigned,
        DashboardView::Watching,
    ] {
        let rows = list_pull_requests(&conn, view, DashboardSort::Updated, None).unwrap();
        assert!(
            rows.iter().all(|r| r.id != 100),
            "{view:?} must hide a PR with every relation archived"
        );
    }
}

/// Unified scope Team view: a team-tracked PR with no relations stays
/// visible (nothing to archive). A team-tracked PR with every relation
/// archived drops.
#[test]
fn team_view_unified_scope_archive_semantics() {
    let conn = fresh_db();
    seed_fixture(&conn);
    // PR 300 sits in alice/api (team-tracked) with no relation rows - stays
    // visible. PR 400 sits in bob/cli (team-tracked) with relations on
    // accounts 1 and 2 - archive both so it should drop.
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET archived_at = strftime('%s', 'now')
          WHERE pull_request_id = 400",
        [],
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, None).unwrap();
    assert!(
        rows.iter().any(|r| r.id == 300),
        "team-tracked PR with no relations stays visible"
    );
    assert!(
        rows.iter().all(|r| r.id != 400),
        "team-tracked PR with every relation archived drops; got {:?}",
        rows.iter().map(|r| r.id).collect::<Vec<_>>()
    );
}

/// Unified scope Team view: a partial archive keeps the team-tracked PR
/// visible via the unarchived relation, with `account_ids` reflecting only
/// the surviving relation owners.
#[test]
fn team_view_unified_scope_partial_archive_keeps_pr_visible() {
    let conn = fresh_db();
    seed_fixture(&conn);
    // Archive account 1's relation on PR 400 (team-tracked under bob/cli).
    // Account 2's relation stays unarchived.
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET archived_at = strftime('%s', 'now')
          WHERE account_id = 1 AND pull_request_id = 400",
        [],
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Team, DashboardSort::Updated, None).unwrap();
    let pr_400 = rows
        .iter()
        .find(|r| r.id == 400)
        .expect("partial archive keeps the PR visible");
    assert_eq!(
        pr_400.account_ids,
        vec![2],
        "merged row carries only unarchived relation owners"
    );
}

/// Archive view returns only rows where `rel.archived_at IS NOT NULL`,
/// regardless of which default-view bucket they would otherwise fall into.
#[test]
fn archive_view_returns_only_archived_rows() {
    let conn = fresh_db();
    seed_fixture(&conn);
    // Archive alice's relations on PR 100 (authored) and PR 200 (watching),
    // and bob's on PR 500 (watching). Leave PR 400 unarchived under both
    // accounts to confirm it doesn't surface in the archive view.
    conn.execute_batch(
        "UPDATE pull_request_viewer_relations
            SET archived_at = strftime('%s', 'now')
          WHERE (account_id = 1 AND pull_request_id IN (100, 200))
             OR (account_id = 2 AND pull_request_id = 500);",
    )
    .unwrap();

    let rows = list_pull_requests(
        &conn,
        DashboardView::Archive,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    assert!(
        ids.contains(&100) && ids.contains(&200),
        "alice's archive must contain her two archived PRs (100, 200); got {ids:?}"
    );
    assert!(
        !ids.contains(&400),
        "PR 400 is not archived under alice; must not appear"
    );
    assert!(
        !ids.contains(&500),
        "PR 500 is archived under bob but not alice; alice's archive must skip it"
    );
}

/// Archive view default sort is `archived_at DESC` when the caller passes
/// `DashboardSort::Updated` (the contract's default).
#[test]
fn archive_view_default_sort_is_archived_at_desc() {
    let conn = fresh_db();
    seed_fixture(&conn);
    // Archive PR 100 first (older), PR 200 second, PR 400 third. The query
    // should return them in 400, 200, 100 order.
    conn.execute_batch(
        "UPDATE pull_request_viewer_relations
            SET archived_at = 1000
          WHERE account_id = 1 AND pull_request_id = 100;
         UPDATE pull_request_viewer_relations
            SET archived_at = 2000
          WHERE account_id = 1 AND pull_request_id = 200;
         UPDATE pull_request_viewer_relations
            SET archived_at = 3000
          WHERE account_id = 1 AND pull_request_id = 400;",
    )
    .unwrap();

    let rows = list_pull_requests(
        &conn,
        DashboardView::Archive,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        vec![400, 200, 100],
        "Archive view default sort orders most-recently-archived first"
    );
}

/// Archive view unified scope: a PR archived under any account surfaces
/// once with `account_ids` containing only the archiving relation owners.
#[test]
fn archive_view_unified_scope_dedupes_partial_archive() {
    let conn = fresh_db();
    seed_two_account_shared_pr_fixture(&conn);
    // Archive account 1's relation only. Account 2 keeps its unarchived
    // relation row.
    conn.execute(
        "UPDATE pull_request_viewer_relations
            SET archived_at = strftime('%s', 'now')
          WHERE account_id = 1 AND pull_request_id = 100",
        [],
    )
    .unwrap();

    let rows =
        list_pull_requests(&conn, DashboardView::Archive, DashboardSort::Updated, None).unwrap();
    let pr = rows
        .iter()
        .find(|r| r.id == 100)
        .expect("partial-archive PR surfaces in the unified archive view");
    assert_eq!(
        pr.account_ids,
        vec![1],
        "archive view's merged row carries only the archiving relation owners"
    );
}

/// Archive view ignores the four-view-split predicates entirely; a row that
/// would surface in Authored, Assigned, Watching, OR Team falls into the
/// archive based purely on `rel.archived_at IS NOT NULL`.
#[test]
fn archive_view_ignores_view_split_predicates() {
    let conn = fresh_db();
    conn.execute_batch(
        r#"
        INSERT INTO accounts (id, label, host, login, created_at) VALUES
            (1, 'alice-acct', 'github.com', 'alice', 0);
        INSERT INTO repos (id, account_id, owner, name, visibility, is_team_tracked) VALUES
            (10, 1, 'alice', 'web', 'public', 0);

        -- Four PRs covering each view-split flag exactly once.
        INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, base_ref, head_ref) VALUES
            (501, 10, 1, 'auth',     'open', 0, 'alice', 0, 100, 'main', 'a'),
            (502, 10, 2, 'review',   'open', 0, 'bob',   0, 200, 'main', 'b'),
            (503, 10, 3, 'watching', 'open', 0, 'bob',   0, 300, 'main', 'c'),
            (504, 10, 4, 'team',     'open', 0, 'bob',   0, 400, 'main', 'd');

        INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at, archived_at) VALUES
            (1, 501, 1, 0, 0, 0, 1000),
            (1, 502, 0, 1, 0, 0, 2000),
            (1, 503, 0, 0, 1, 0, 3000),
            (1, 504, 0, 0, 0, 0, 4000);
        "#,
    )
    .unwrap();

    let rows = list_pull_requests(
        &conn,
        DashboardView::Archive,
        DashboardSort::Updated,
        Some(1),
    )
    .unwrap();
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        vec![504, 503, 502, 501],
        "Archive admits rows from every view-split bucket sorted by archived_at DESC"
    );
}
