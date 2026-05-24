//! Test fixtures + entry-level write_pr_updates / compute_ci_rollup coverage.
//! Larger test categories sit in sibling modules (`conversation`, `timeline`,
//! `users`) and pull the helpers below via `use super::*`.

use super::*;
use crate::db::DbHandle;

// Sub-modules. Each handles a focused area: conversation depth (threads +
// reviews), timeline events, and the users avatar cache. They share the
// helpers defined below via `use super::*;`.
mod conversation;
mod timeline;
mod users;

// ===== write_pr_updates persistence tests =====
//
// Each test stands up an in-memory SQLite DB at the latest migration,
// seeds an account + repo + placeholder PR row, then calls
// `write_pr_updates` with a hand-rolled `PullRequestDetail`.

use crate::github::graphql::{
    Actor, PrCommit, PrCommitConnection, PrCommitNode, PullRequestDetail, RequestedReviewer,
    ReviewRequest, ReviewRequestConnection, ReviewThreadConnection, StatusCheckContext,
    StatusCheckContexts, StatusCheckRollup,
};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

fn empty_review_threads() -> ReviewThreadConnection {
    ReviewThreadConnection {
        page_info: crate::github::graphql::PageInfo {
            has_next_page: false,
            end_cursor: None,
        },
        nodes: vec![],
    }
}

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

fn detail_with(
    additions: Option<i64>,
    deletions: Option<i64>,
    changed_files: Option<i64>,
    mergeable: &str,
    review_decision: Option<&str>,
    review_requests: Option<ReviewRequestConnection>,
    commits: Option<PrCommitConnection>,
) -> PullRequestDetail {
    PullRequestDetail {
        id: "PR_test".into(),
        number: 42,
        title: "Add a thing".into(),
        is_draft: false,
        state: "OPEN".into(),
        merged: false,
        mergeable: mergeable.into(),
        url: "https://github.com/owner/repo/pull/42".into(),
        created_at: "2026-05-18T10:00:00Z".into(),
        updated_at: "2026-05-19T11:00:00Z".into(),
        author: Some(Actor::new("alice")),
        base_ref_name: "main".into(),
        head_ref_name: "feat/thing".into(),
        review_decision: review_decision.map(str::to_string),
        additions,
        deletions,
        changed_files,
        review_requests,
        commits,
        review_threads: empty_review_threads(),
        reviews: None,
        issue_comments: None,
    }
}

fn rollup_with(state: &str, total: i64, nodes: Vec<StatusCheckContext>) -> PrCommitConnection {
    PrCommitConnection {
        nodes: vec![PrCommitNode {
            commit: PrCommit {
                status_check_rollup: Some(StatusCheckRollup {
                    state: state.into(),
                    contexts: StatusCheckContexts {
                        total_count: total,
                        nodes,
                    },
                }),
            },
        }],
    }
}

#[test]
fn write_pr_updates_persists_every_new_column() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let detail = detail_with(
        Some(120),
        Some(30),
        Some(5),
        "MERGEABLE",
        Some("APPROVED"),
        None,
        Some(rollup_with(
            "SUCCESS",
            3,
            vec![
                StatusCheckContext::CheckRun {
                    conclusion: Some("SUCCESS".into()),
                    status: Some("COMPLETED".into()),
                },
                StatusCheckContext::CheckRun {
                    conclusion: Some("SUCCESS".into()),
                    status: Some("COMPLETED".into()),
                },
                StatusCheckContext::StatusContext {
                    state: "SUCCESS".into(),
                },
            ],
        )),
    );

    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

    let conn = db.lock().unwrap();
    type Row = (
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<i64>,
        Option<i64>,
    );
    let row: Row = conn
        .query_row(
            "SELECT mergeable, review_decision, additions, deletions, changed_files,
                    ci_state, ci_total, ci_passing
               FROM pull_requests WHERE id = ?1",
            params![pr_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row.0.as_deref(), Some("MERGEABLE"));
    assert_eq!(row.1.as_deref(), Some("APPROVED"));
    assert_eq!(row.2, Some(120));
    assert_eq!(row.3, Some(30));
    assert_eq!(row.4, Some(5));
    assert_eq!(row.5.as_deref(), Some("SUCCESS"));
    assert_eq!(row.6, Some(3));
    assert_eq!(row.7, Some(3));
}

#[test]
fn write_pr_updates_replaces_requested_reviewers() {
    let (db, repo_id, pr_id) = seed_db_with_pr();

    // Seed an existing reviewer that should be replaced.
    db.lock()
        .unwrap()
        .execute(
            "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                VALUES (?1, 'stale-user', 'user')",
            params![pr_id],
        )
        .unwrap();

    let detail = detail_with(
        None,
        None,
        None,
        "UNKNOWN",
        None,
        Some(ReviewRequestConnection {
            nodes: vec![
                ReviewRequest {
                    requested_reviewer: Some(RequestedReviewer::User {
                        login: "dave".into(),
                        avatar_url: Some("https://avatars/dave".into()),
                    }),
                },
                ReviewRequest {
                    requested_reviewer: Some(RequestedReviewer::Team {
                        slug: "platform".into(),
                    }),
                },
                // A null reviewer (deleted account) must be silently
                // dropped, not persisted as an empty-string row.
                ReviewRequest {
                    requested_reviewer: None,
                },
            ],
        }),
        None,
    );

    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

    let conn = db.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT login, reviewer_type FROM requested_reviewers
              WHERE pull_request_id = ?1 ORDER BY reviewer_type, login",
        )
        .unwrap();
    let rows: Vec<(String, String)> = stmt
        .query_map(params![pr_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rows,
        vec![
            ("platform".to_string(), "team".to_string()),
            ("dave".to_string(), "user".to_string()),
        ],
        "delete-then-insert: stale-user is gone, dave + platform are present"
    );
}

#[test]
fn write_pr_updates_clears_requested_reviewers_when_empty() {
    let (db, repo_id, pr_id) = seed_db_with_pr();

    db.lock()
        .unwrap()
        .execute(
            "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                VALUES (?1, 'stale-user', 'user')",
            params![pr_id],
        )
        .unwrap();

    // Empty `nodes` array — upstream returned the field, but no reviewers.
    let detail = detail_with(
        None,
        None,
        None,
        "UNKNOWN",
        None,
        Some(ReviewRequestConnection { nodes: vec![] }),
        None,
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

    let count: i64 = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM requested_reviewers WHERE pull_request_id = ?1",
            params![pr_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn write_pr_updates_skips_requested_reviewers_when_absent() {
    let (db, repo_id, pr_id) = seed_db_with_pr();

    db.lock()
        .unwrap()
        .execute(
            "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                VALUES (?1, 'keeper', 'user')",
            params![pr_id],
        )
        .unwrap();

    // `review_requests` absent from the response (None) — leave existing
    // cache untouched so a partial detail doesn't drop the set.
    let detail = detail_with(None, None, None, "UNKNOWN", None, None, None);
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

    let count: i64 = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM requested_reviewers WHERE pull_request_id = ?1",
            params![pr_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn compute_ci_rollup_tallies_mixed_contexts_and_in_progress() {
    // 4 contexts: 1 SUCCESS CheckRun, 1 in-progress CheckRun (null
    // conclusion), 1 SUCCESS StatusContext, 1 FAILURE StatusContext.
    // Expected: state PENDING (rollup-provided), total 4, passing 2.
    let detail = detail_with(
        None,
        None,
        None,
        "UNKNOWN",
        None,
        None,
        Some(rollup_with(
            "PENDING",
            4,
            vec![
                StatusCheckContext::CheckRun {
                    conclusion: Some("SUCCESS".into()),
                    status: Some("COMPLETED".into()),
                },
                StatusCheckContext::CheckRun {
                    conclusion: None,
                    status: Some("IN_PROGRESS".into()),
                },
                StatusCheckContext::StatusContext {
                    state: "SUCCESS".into(),
                },
                StatusCheckContext::StatusContext {
                    state: "FAILURE".into(),
                },
            ],
        )),
    );

    let ci = compute_ci_rollup(&detail);
    assert_eq!(ci.state.as_deref(), Some("PENDING"));
    assert_eq!(ci.total, Some(4));
    assert_eq!(ci.passing, Some(2));
}

#[test]
fn compute_ci_rollup_returns_none_when_rollup_absent() {
    // No commits at all.
    let no_commits = detail_with(None, None, None, "UNKNOWN", None, None, None);
    let ci = compute_ci_rollup(&no_commits);
    assert_eq!(
        ci,
        CiRollup {
            state: None,
            total: None,
            passing: None,
        }
    );

    // Commit present but no rollup attached.
    let no_rollup = detail_with(
        None,
        None,
        None,
        "UNKNOWN",
        None,
        None,
        Some(PrCommitConnection {
            nodes: vec![PrCommitNode {
                commit: PrCommit {
                    status_check_rollup: None,
                },
            }],
        }),
    );
    let ci = compute_ci_rollup(&no_rollup);
    assert_eq!(
        ci,
        CiRollup {
            state: None,
            total: None,
            passing: None,
        }
    );
}

#[test]
fn write_pr_updates_persists_ci_rollup_with_in_progress_run() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let detail = detail_with(
        None,
        None,
        None,
        "MERGEABLE",
        None,
        None,
        Some(rollup_with(
            "PENDING",
            3,
            vec![
                StatusCheckContext::CheckRun {
                    conclusion: Some("SUCCESS".into()),
                    status: Some("COMPLETED".into()),
                },
                StatusCheckContext::CheckRun {
                    conclusion: None,
                    status: Some("IN_PROGRESS".into()),
                },
                StatusCheckContext::StatusContext {
                    state: "SUCCESS".into(),
                },
            ],
        )),
    );

    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

    let (state, total, passing): (Option<String>, Option<i64>, Option<i64>) = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT ci_state, ci_total, ci_passing FROM pull_requests WHERE id = ?1",
            params![pr_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(state.as_deref(), Some("PENDING"));
    assert_eq!(total, Some(3));
    assert_eq!(passing, Some(2));
}

#[test]
fn write_pr_updates_skips_unknown_reviewer_typenames() {
    let (db, repo_id, pr_id) = seed_db_with_pr();

    let detail = detail_with(
        None,
        None,
        None,
        "UNKNOWN",
        None,
        Some(ReviewRequestConnection {
            nodes: vec![
                ReviewRequest {
                    requested_reviewer: Some(RequestedReviewer::Other),
                },
                ReviewRequest {
                    requested_reviewer: Some(RequestedReviewer::User {
                        login: "alice".into(),
                        avatar_url: None,
                    }),
                },
            ],
        }),
        None,
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

    let logins: Vec<String> = {
        let conn = db.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT login FROM requested_reviewers
                  WHERE pull_request_id = ?1 ORDER BY login",
            )
            .unwrap();
        stmt.query_map(params![pr_id], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(logins, vec!["alice".to_string()]);
}
