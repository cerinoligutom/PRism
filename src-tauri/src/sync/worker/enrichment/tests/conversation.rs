//! Conversation depth (M3-A) tests: review threads, reviews, issue comments
//! counters, and the pruning pass.

use super::*;
use crate::github::graphql::{
    IssueCommentConnection, PageInfo, PullRequestReviewConnection, PullRequestReviewNode,
    ReviewCommentConnection, ReviewCommentNode, ReviewThread,
};

pub(super) struct ThreadSpec<'a> {
    node_id: &'a str,
    is_resolved: bool,
    is_outdated: bool,
    path: &'a str,
    line: Option<i64>,
    start_line: Option<i64>,
    original_line: Option<i64>,
    head: Option<(&'a str, &'a str, &'a str)>,
    total_count: i64,
    /// Head comment's `url`. The thread permalink is derived from this at
    /// write time (issue #115).
    head_url: Option<&'a str>,
}

impl<'a> ThreadSpec<'a> {
    pub(super) fn open(node_id: &'a str, path: &'a str, head: (&'a str, &'a str, &'a str)) -> Self {
        Self {
            node_id,
            is_resolved: false,
            is_outdated: false,
            path,
            line: None,
            start_line: None,
            original_line: None,
            head: Some(head),
            total_count: 1,
            head_url: None,
        }
    }

    fn resolved(mut self, resolved: bool) -> Self {
        self.is_resolved = resolved;
        self
    }

    fn outdated(mut self, outdated: bool) -> Self {
        self.is_outdated = outdated;
        self
    }

    pub(super) fn lines(
        mut self,
        line: Option<i64>,
        start: Option<i64>,
        original: Option<i64>,
    ) -> Self {
        self.line = line;
        self.start_line = start;
        self.original_line = original;
        self
    }

    fn total_count(mut self, count: i64) -> Self {
        self.total_count = count;
        self
    }

    fn head_url(mut self, url: &'a str) -> Self {
        self.head_url = Some(url);
        self
    }
}

pub(super) fn thread(spec: ThreadSpec<'_>) -> ReviewThread {
    let head_url = spec.head_url.map(str::to_string);
    let head_node = spec.head.map(|(id, login, created_at)| ReviewCommentNode {
        id: id.into(),
        url: head_url,
        database_id: None,
        author: Some(Actor::new(login)),
        body: "head body".into(),
        body_html: None,
        body_text: "head body".into(),
        created_at: created_at.into(),
        path: None,
        line: None,
        original_line: None,
        side: None,
        diff_hunk: None,
    });
    ReviewThread {
        id: spec.node_id.into(),
        is_resolved: spec.is_resolved,
        is_outdated: spec.is_outdated,
        path: Some(spec.path.into()),
        line: spec.line,
        start_line: spec.start_line,
        original_line: spec.original_line,
        comments: ReviewCommentConnection {
            total_count: spec.total_count,
            page_info: PageInfo {
                has_next_page: false,
                end_cursor: None,
            },
            nodes: head_node.into_iter().collect(),
        },
    }
}

fn empty_thread(node_id: &str, path: &str) -> ReviewThread {
    ReviewThread {
        id: node_id.into(),
        is_resolved: false,
        is_outdated: false,
        path: Some(path.into()),
        line: None,
        start_line: None,
        original_line: None,
        comments: ReviewCommentConnection {
            total_count: 0,
            page_info: PageInfo {
                has_next_page: false,
                end_cursor: None,
            },
            nodes: vec![],
        },
    }
}

pub(super) fn review_threads(nodes: Vec<ReviewThread>) -> ReviewThreadConnection {
    ReviewThreadConnection {
        page_info: PageInfo {
            has_next_page: false,
            end_cursor: None,
        },
        nodes,
    }
}

pub(super) fn detail_with_threads(
    threads: ReviewThreadConnection,
    reviews: Option<PullRequestReviewConnection>,
    issue_comments: Option<IssueCommentConnection>,
) -> PullRequestDetail {
    let mut d = detail_with(None, None, None, "MERGEABLE", None, None, None);
    d.review_threads = threads;
    d.reviews = reviews;
    d.issue_comments = issue_comments;
    d
}

#[test]
fn write_pr_updates_upserts_review_threads_with_line_range_and_head_snapshot() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let detail = detail_with_threads(
        review_threads(vec![thread(
            ThreadSpec::open(
                "PRRT_1",
                "src/lib.rs",
                ("PRRC_1", "alice", "2026-05-18T10:00:00Z"),
            )
            .lines(Some(42), Some(40), Some(41))
            .total_count(3),
        )]),
        None,
        None,
    );

    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

    let conn = db.lock().unwrap();
    type Row = (
        String,
        i64,
        i64,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        i64,
    );
    let row: Row = conn
        .query_row(
            "SELECT node_id, is_resolved, is_outdated, path, line, start_line,
                    original_line, created_at, resolved_at, last_reply_at, reply_count
               FROM review_threads
              WHERE pull_request_id = ?1 AND node_id = 'PRRT_1'",
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
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(row.0, "PRRT_1");
    assert_eq!(row.1, 0); // is_resolved
    assert_eq!(row.2, 0); // is_outdated
    assert_eq!(row.3.as_deref(), Some("src/lib.rs"));
    assert_eq!(row.4, Some(42));
    assert_eq!(row.5, Some(40));
    assert_eq!(row.6, Some(41));
    // created_at + last_reply_at derived from the head comment's createdAt.
    assert_eq!(row.7, rfc3339_to_unix("2026-05-18T10:00:00Z"));
    assert_eq!(row.8, None); // resolved_at — unresolved on first write.
    assert_eq!(row.9, rfc3339_to_unix("2026-05-18T10:00:00Z"));
    assert_eq!(row.10, 2); // reply_count = totalCount(3) - 1

    // ADR 0029: the head comment metadata now lives in `review_comments`,
    // not denormalised onto review_threads. Verify the head comment row was
    // persisted by sync (per write_review_threads).
    let head: (String, String, i64) = conn
        .query_row(
            "SELECT c.author_login, c.body, c.created_at
               FROM review_comments c
               JOIN review_threads t ON t.id = c.review_thread_id
              WHERE t.node_id = 'PRRT_1'
              ORDER BY c.created_at ASC, c.id ASC
              LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(head.0, "alice");
    assert_eq!(head.1, "head body");
    assert_eq!(Some(head.2), rfc3339_to_unix("2026-05-18T10:00:00Z"));
}

#[test]
fn write_pr_updates_persists_review_thread_url_from_head_comment() {
    // Issue #115: `PullRequestReviewThread` has no `url` field on GitHub's
    // GraphQL schema, so the worker derives `review_threads.url` from the
    // head comment's `url` at write time. Confirm the derivation happens
    // on first insert and that a later payload with no head url leaves
    // the previously-persisted value intact (`COALESCE` in the upsert).
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let detail = detail_with_threads(
        review_threads(vec![thread(
            ThreadSpec::open(
                "PRRT_URL",
                "src/lib.rs",
                ("PRRC_U1", "alice", "2026-05-18T10:00:00Z"),
            )
            .head_url("https://github.com/owner/repo/pull/1#discussion_r42"),
        )]),
        None,
        None,
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();
    let url: Option<String> = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT url FROM review_threads WHERE node_id = 'PRRT_URL'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        url.as_deref(),
        Some("https://github.com/owner/repo/pull/1#discussion_r42")
    );

    // Cycle 2: same thread, head comment url absent. The COALESCE in the
    // upsert keeps the previously-persisted url rather than blanking it.
    let detail2 = detail_with_threads(
        review_threads(vec![thread(ThreadSpec::open(
            "PRRT_URL",
            "src/lib.rs",
            ("PRRC_U1", "alice", "2026-05-18T10:00:00Z"),
        ))]),
        None,
        None,
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail2), None).unwrap();
    let url_after: Option<String> = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT url FROM review_threads WHERE node_id = 'PRRT_URL'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        url_after.as_deref(),
        Some("https://github.com/owner/repo/pull/1#discussion_r42"),
        "thread url must survive a payload with no head-comment url"
    );
}

#[test]
fn write_pr_updates_thread_url_stays_null_without_head_comment() {
    // Defensive: a thread that arrives with no head comment leaves
    // `review_threads.url` NULL rather than blowing up.
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let detail = detail_with_threads(
        review_threads(vec![empty_thread("PRRT_empty_url", "x.rs")]),
        None,
        None,
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();
    let url: Option<String> = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT url FROM review_threads WHERE node_id = 'PRRT_empty_url'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(url.is_none());
}

#[test]
fn write_pr_updates_tracks_resolved_at_transitions() {
    let (db, repo_id, pr_id) = seed_db_with_pr();

    // Cycle 1: unresolved.
    let d1 = detail_with_threads(
        review_threads(vec![thread(
            ThreadSpec::open(
                "PRRT_1",
                "src/lib.rs",
                ("PRRC_1", "alice", "2026-05-18T10:00:00Z"),
            )
            .lines(Some(1), None, None),
        )]),
        None,
        None,
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&d1), None).unwrap();
    let resolved_at: Option<i64> = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT resolved_at FROM review_threads WHERE node_id = 'PRRT_1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(resolved_at, None);

    // Cycle 2: resolved. resolved_at must be set.
    let d2 = detail_with_threads(
        review_threads(vec![thread(
            ThreadSpec::open(
                "PRRT_1",
                "src/lib.rs",
                ("PRRC_1", "alice", "2026-05-18T10:00:00Z"),
            )
            .lines(Some(1), None, None)
            .resolved(true),
        )]),
        None,
        None,
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&d2), None).unwrap();
    let resolved_at: Option<i64> = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT resolved_at FROM review_threads WHERE node_id = 'PRRT_1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        resolved_at.is_some(),
        "resolved_at must be stamped on transition to resolved"
    );
    let stamped = resolved_at.unwrap();

    // Cycle 3: still resolved. resolved_at preserved (not bumped).
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&d2), None).unwrap();
    let resolved_at: Option<i64> = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT resolved_at FROM review_threads WHERE node_id = 'PRRT_1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        resolved_at,
        Some(stamped),
        "resolved_at must be preserved when state is unchanged"
    );

    // Cycle 4: thread flips back to unresolved. resolved_at must clear.
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&d1), None).unwrap();
    let resolved_at: Option<i64> = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT resolved_at FROM review_threads WHERE node_id = 'PRRT_1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(resolved_at, None);

    // Cycle 5: thread becomes outdated (still unresolved). Outdated flag
    // recorded, resolved_at remains null.
    let d3 = detail_with_threads(
        review_threads(vec![thread(
            ThreadSpec::open(
                "PRRT_1",
                "src/lib.rs",
                ("PRRC_1", "alice", "2026-05-18T10:00:00Z"),
            )
            .lines(Some(1), None, None)
            .outdated(true),
        )]),
        None,
        None,
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&d3), None).unwrap();
    let (is_outdated, resolved_at): (i64, Option<i64>) = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT is_outdated, resolved_at FROM review_threads WHERE node_id = 'PRRT_1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(is_outdated, 1);
    assert_eq!(resolved_at, None);
}

#[test]
fn write_pr_updates_prunes_removed_threads_and_reviews() {
    let (db, repo_id, pr_id) = seed_db_with_pr();

    // Cycle 1: two threads + two reviews persisted.
    let d1 = detail_with_threads(
        review_threads(vec![
            thread(
                ThreadSpec::open(
                    "PRRT_keep",
                    "a.rs",
                    ("PRRC_a", "alice", "2026-05-18T10:00:00Z"),
                )
                .lines(Some(1), None, None),
            ),
            thread(
                ThreadSpec::open(
                    "PRRT_drop",
                    "b.rs",
                    ("PRRC_b", "bob", "2026-05-18T11:00:00Z"),
                )
                .lines(Some(2), None, None),
            ),
        ]),
        Some(PullRequestReviewConnection {
            nodes: vec![
                PullRequestReviewNode {
                    id: "PRR_keep".into(),
                    state: "APPROVED".into(),
                    body: Some("LGTM".into()),
                    body_html: None,
                    submitted_at: Some("2026-05-18T12:00:00Z".into()),
                    url: None,
                    author: Some(Actor::new("alice")),
                },
                PullRequestReviewNode {
                    id: "PRR_drop".into(),
                    state: "COMMENTED".into(),
                    body: None,
                    body_html: None,
                    submitted_at: Some("2026-05-18T13:00:00Z".into()),
                    url: None,
                    author: Some(Actor::new("bob")),
                },
            ],
        }),
        None,
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&d1), None).unwrap();

    let thread_count: i64 = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM review_threads WHERE pull_request_id = ?1",
            params![pr_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(thread_count, 2);
    let review_count: i64 = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM reviews WHERE pull_request_id = ?1",
            params![pr_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(review_count, 2);

    // Cycle 2: only the "keep" thread + review remain upstream.
    let d2 = detail_with_threads(
        review_threads(vec![thread(
            ThreadSpec::open(
                "PRRT_keep",
                "a.rs",
                ("PRRC_a", "alice", "2026-05-18T10:00:00Z"),
            )
            .lines(Some(1), None, None),
        )]),
        Some(PullRequestReviewConnection {
            nodes: vec![PullRequestReviewNode {
                id: "PRR_keep".into(),
                state: "APPROVED".into(),
                body: Some("LGTM".into()),
                body_html: None,
                submitted_at: Some("2026-05-18T12:00:00Z".into()),
                url: None,
                author: Some(Actor::new("alice")),
            }],
        }),
        None,
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&d2), None).unwrap();

    let surviving_threads: Vec<String> = {
        let conn = db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT node_id FROM review_threads WHERE pull_request_id = ?1")
            .unwrap();
        stmt.query_map(params![pr_id], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(surviving_threads, vec!["PRRT_keep".to_string()]);

    let surviving_reviews: Vec<String> = {
        let conn = db.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT node_id FROM reviews WHERE pull_request_id = ?1")
            .unwrap();
        stmt.query_map(params![pr_id], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(surviving_reviews, vec!["PRR_keep".to_string()]);
}

#[test]
fn write_pr_updates_clamps_reply_count_to_zero_on_empty_thread() {
    // Defensive: GraphQL shouldn't surface totalCount = 0 for a populated
    // thread, but guard against negative reply_count if it ever does.
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let detail = detail_with_threads(
        review_threads(vec![empty_thread("PRRT_empty", "x.rs")]),
        None,
        None,
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

    let reply_count: i64 = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT reply_count FROM review_threads WHERE node_id = 'PRRT_empty'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(reply_count, 0);
}

#[test]
fn write_pr_updates_writes_issue_comments_count() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let detail = detail_with_threads(
        empty_review_threads(),
        None,
        Some(IssueCommentConnection {
            total_count: 17,
            page_info: PageInfo {
                has_next_page: false,
                end_cursor: None,
            },
            nodes: vec![],
        }),
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

    let count: i64 = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT issue_comments_count FROM pull_requests WHERE id = ?1",
            params![pr_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 17);
}

#[test]
fn write_pr_updates_persists_reviews_with_optional_body() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let detail = detail_with_threads(
        empty_review_threads(),
        Some(PullRequestReviewConnection {
            nodes: vec![
                PullRequestReviewNode {
                    id: "PRR_a".into(),
                    state: "APPROVED".into(),
                    body: Some("LGTM".into()),
                    body_html: None,
                    submitted_at: Some("2026-05-18T12:00:00Z".into()),
                    url: None,
                    author: Some(Actor::new("alice")),
                },
                PullRequestReviewNode {
                    id: "PRR_b".into(),
                    state: "COMMENTED".into(),
                    body: None,
                    body_html: None,
                    submitted_at: None,
                    url: None,
                    author: None,
                },
            ],
        }),
        None,
    );
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

    type ReviewRow = (String, String, Option<String>, Option<i64>, String);
    let rows: Vec<ReviewRow> = {
        let conn = db.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT node_id, state, body, submitted_at, reviewer_login
                   FROM reviews
                  WHERE pull_request_id = ?1
                  ORDER BY node_id",
            )
            .unwrap();
        stmt.query_map(params![pr_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    };
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "PRR_a");
    assert_eq!(rows[0].1, "APPROVED");
    assert_eq!(rows[0].2.as_deref(), Some("LGTM"));
    assert_eq!(rows[0].3, rfc3339_to_unix("2026-05-18T12:00:00Z"));
    assert_eq!(rows[0].4, "alice");
    assert_eq!(rows[1].0, "PRR_b");
    assert_eq!(rows[1].1, "COMMENTED");
    assert!(rows[1].2.is_none());
    assert!(rows[1].3.is_none());
    assert_eq!(rows[1].4, "");
}
