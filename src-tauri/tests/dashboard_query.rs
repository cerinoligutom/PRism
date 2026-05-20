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

use prism_lib::dashboard::query::list_pull_requests;
use prism_lib::dashboard::{DashboardSort, DashboardView, ReviewerEntry, ReviewerState};
use prism_lib::db::migrate;
use rusqlite::params;
use rusqlite::Connection;

fn fresh_db() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate::run(&mut conn).unwrap();
    conn
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
fn account_id_none_returns_union_across_accounts() {
    let conn = fresh_db();
    seed_fixture(&conn);

    // Watching across every account: every relation row with is_involved = 1.
    // Fixture has: (1,100), (1,200), (2,400), (1,400), (2,500).
    // The same PR can appear twice with different `account_id`s.
    let rows =
        list_pull_requests(&conn, DashboardView::Watching, DashboardSort::Updated, None).unwrap();
    let mut actual: Vec<(i64, i64)> = rows.iter().map(|r| (r.id, r.account_id)).collect();
    let mut expected = vec![(100, 1), (200, 1), (400, 2), (400, 1), (500, 2)];
    expected.sort();
    actual.sort();
    assert_eq!(actual, expected);
}

#[test]
fn is_you_marks_reviewers_matching_the_owning_account_login() {
    let conn = fresh_db();
    seed_fixture(&conn);

    // For alice: PR 400 has alice as both a submitted reviewer
    // (CHANGES_REQUESTED) and a requested reviewer (pending). Both must be
    // marked is_you.
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
    assert_eq!(alice_entries.len(), 2);
    assert!(alice_entries.iter().all(|r| r.is_you));
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
    assert_eq!(pr.account_id, 1);
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

// ===== threads rollup projection tests (M3-C) =====

#[test]
fn threads_is_none_when_pull_request_has_never_had_a_thread() {
    let conn = fresh_db();
    seed_fixture(&conn);

    // The fixture's PRs leave `threads_*` at the migration's `DEFAULT 0`, so
    // every projected row reads `threads = None`. The frontend renders the
    // muted em-dash state in that case.
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
fn threads_projects_four_buckets_when_populated() {
    let conn = fresh_db();
    seed_fixture(&conn);

    conn.execute(
        "UPDATE pull_requests
            SET threads_total = ?2,
                threads_unresolved_involved = ?3,
                threads_unresolved_uninvolved = ?4,
                threads_resolved_involved = ?5,
                threads_resolved_uninvolved = ?6
          WHERE id = ?1",
        params![100i64, 5i64, 1i64, 2i64, 1i64, 1i64],
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
    assert_eq!(threads.unresolved_involved, 1);
    assert_eq!(threads.unresolved_uninvolved, 2);
    assert_eq!(threads.resolved_involved, 1);
    assert_eq!(threads.resolved_uninvolved, 1);
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
