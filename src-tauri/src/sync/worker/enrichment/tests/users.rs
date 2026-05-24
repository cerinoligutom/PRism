//! Users cache (ADR 0013) - avatar caching tests.

use super::conversation::ThreadSpec;
use super::*;
use crate::sync::status_timeline::TimelineEvent;
use time::macros::datetime;

fn read_user(db: &DbHandle, login: &str) -> Option<String> {
    db.lock()
        .unwrap()
        .query_row(
            "SELECT avatar_url FROM users WHERE login = ?1",
            params![login],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
}

fn detail_with_author_avatar(login: &str, url: &str) -> PullRequestDetail {
    let mut d = detail_with(None, None, None, "MERGEABLE", None, None, None);
    d.author = Some(Actor {
        login: login.into(),
        avatar_url: Some(url.into()),
    });
    d
}

#[test]
fn write_pr_updates_upserts_pr_author_avatar_into_users() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let detail = detail_with_author_avatar("alice", "https://avatars/alice.png");
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();
    assert_eq!(
        read_user(&db, "alice").as_deref(),
        Some("https://avatars/alice.png"),
    );
}

#[test]
fn write_pr_updates_skips_users_upsert_when_avatar_missing() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let detail = detail_with(None, None, None, "MERGEABLE", None, None, None);
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();
    // Author is "alice" (from `detail_with`) with `avatar_url = None`; no
    // users row should land because we never store NULL avatars.
    let count: i64 = db
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn write_pr_updates_upserts_thread_head_comment_authors() {
    use super::conversation::{detail_with_threads, review_threads, thread};
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let mut detail = detail_with_threads(
        review_threads(vec![thread(
            ThreadSpec::open(
                "PRRT_1",
                "src/lib.rs",
                ("PRRC_1", "bob", "2026-05-18T10:00:00Z"),
            )
            .lines(Some(1), None, None),
        )]),
        None,
        None,
    );
    // Stamp an avatar URL onto the head comment's author so the upsert
    // surfaces a populated row.
    detail.review_threads.nodes[0].comments.nodes[0].author = Some(Actor {
        login: "bob".into(),
        avatar_url: Some("https://avatars/bob.png".into()),
    });
    write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();
    assert_eq!(
        read_user(&db, "bob").as_deref(),
        Some("https://avatars/bob.png"),
    );
}

#[test]
fn write_pr_updates_upserts_timeline_actor_avatars() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let events = vec![TimelineEvent {
        event: "reviewed".into(),
        created_at: datetime!(2026-05-03 10:00:00 UTC),
        actor_login: Some("carol".into()),
        actor_avatar_url: Some("https://avatars/carol.png".into()),
        review_state: Some("APPROVED".into()),
    }];
    write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&events)).unwrap();
    assert_eq!(
        read_user(&db, "carol").as_deref(),
        Some("https://avatars/carol.png"),
    );
}

#[test]
fn write_pr_updates_refreshes_avatar_url_on_change() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    write_pr_updates(
        &db,
        1,
        repo_id,
        pr_id,
        Some(&detail_with_author_avatar(
            "alice",
            "https://avatars/old.png",
        )),
        None,
    )
    .unwrap();
    write_pr_updates(
        &db,
        1,
        repo_id,
        pr_id,
        Some(&detail_with_author_avatar(
            "alice",
            "https://avatars/new.png",
        )),
        None,
    )
    .unwrap();
    assert_eq!(
        read_user(&db, "alice").as_deref(),
        Some("https://avatars/new.png"),
    );
}
