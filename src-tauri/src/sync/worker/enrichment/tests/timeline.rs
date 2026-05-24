//! `timeline_events` persistence tests + payload-builder coverage.

use super::super::timeline::timeline_event_payload;
use super::*;
use crate::sync::status_timeline::TimelineEvent;
use time::macros::datetime;

fn tle(
    kind: &str,
    at: time::OffsetDateTime,
    actor: Option<&str>,
    state: Option<&str>,
) -> TimelineEvent {
    TimelineEvent {
        event: kind.into(),
        created_at: at,
        actor_login: actor.map(str::to_string),
        actor_avatar_url: None,
        review_state: state.map(str::to_string),
    }
}

fn read_timeline_events(db: &DbHandle, pr_id: i64) -> Vec<(String, Option<String>, i64, String)> {
    let conn = db.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT event_type, actor_login, created_at, payload
               FROM timeline_events
              WHERE pull_request_id = ?1
              ORDER BY created_at, id",
        )
        .unwrap();
    stmt.query_map(params![pr_id], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn timeline_event_payload_emits_review_state_for_reviewed_events() {
    let payload = timeline_event_payload(&tle(
        "reviewed",
        datetime!(2026-05-03 10:00:00 UTC),
        Some("bob"),
        Some("APPROVED"),
    ));
    assert_eq!(payload, r#"{"state":"APPROVED"}"#);

    let payload = timeline_event_payload(&tle(
        "ready_for_review",
        datetime!(2026-05-02 14:30:00 UTC),
        Some("alice"),
        None,
    ));
    assert_eq!(payload, "{}");
}

#[test]
fn write_pr_updates_persists_qualifying_timeline_events() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let events = vec![
        tle(
            "ready_for_review",
            datetime!(2026-05-02 14:30:00 UTC),
            Some("alice"),
            None,
        ),
        tle(
            "reviewed",
            datetime!(2026-05-03 10:00:00 UTC),
            Some("bob"),
            Some("APPROVED"),
        ),
        tle(
            "merged",
            datetime!(2026-05-06 11:00:00 UTC),
            Some("alice"),
            None,
        ),
    ];
    write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&events)).unwrap();

    let rows = read_timeline_events(&db, pr_id);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].0, "ready_for_review");
    assert_eq!(rows[0].1.as_deref(), Some("alice"));
    assert_eq!(rows[0].3, "{}");
    assert_eq!(rows[1].0, "reviewed");
    assert_eq!(rows[1].3, r#"{"state":"APPROVED"}"#);
    assert_eq!(rows[2].0, "merged");
}

#[test]
fn write_pr_updates_overwrites_existing_timeline_events_on_rerun() {
    let (db, repo_id, pr_id) = seed_db_with_pr();

    let cycle1 = vec![
        tle(
            "ready_for_review",
            datetime!(2026-05-02 14:30:00 UTC),
            Some("alice"),
            None,
        ),
        tle(
            "reviewed",
            datetime!(2026-05-03 10:00:00 UTC),
            Some("bob"),
            Some("APPROVED"),
        ),
    ];
    write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&cycle1)).unwrap();
    assert_eq!(read_timeline_events(&db, pr_id).len(), 2);

    let cycle2 = vec![
        tle(
            "ready_for_review",
            datetime!(2026-05-02 14:30:00 UTC),
            Some("alice"),
            None,
        ),
        tle(
            "reviewed",
            datetime!(2026-05-03 10:00:00 UTC),
            Some("bob"),
            Some("CHANGES_REQUESTED"),
        ),
        tle(
            "merged",
            datetime!(2026-05-06 11:00:00 UTC),
            Some("alice"),
            None,
        ),
    ];
    write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&cycle2)).unwrap();

    let rows = read_timeline_events(&db, pr_id);
    assert_eq!(rows.len(), 3, "wipe-and-rewrite replaces the whole set");
    // The reviewed event's payload state must reflect the second cycle.
    let reviewed = rows
        .iter()
        .find(|r| r.0 == "reviewed")
        .expect("reviewed event present");
    assert_eq!(reviewed.3, r#"{"state":"CHANGES_REQUESTED"}"#);
}

#[test]
fn write_pr_updates_empty_events_clears_existing_timeline_rows() {
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let cycle1 = vec![tle(
        "ready_for_review",
        datetime!(2026-05-02 14:30:00 UTC),
        Some("alice"),
        None,
    )];
    write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&cycle1)).unwrap();
    assert_eq!(read_timeline_events(&db, pr_id).len(), 1);

    write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&[])).unwrap();
    assert_eq!(
        read_timeline_events(&db, pr_id).len(),
        0,
        "empty fetch clears the table for this PR"
    );
}

#[test]
fn write_pr_updates_none_events_leaves_existing_timeline_rows_intact() {
    // A 304 from the REST timeline endpoint surfaces as `events: None`;
    // we must not touch the cached rows on that path.
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let cycle1 = vec![tle(
        "ready_for_review",
        datetime!(2026-05-02 14:30:00 UTC),
        Some("alice"),
        None,
    )];
    write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&cycle1)).unwrap();

    write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
    assert_eq!(
        read_timeline_events(&db, pr_id).len(),
        1,
        "None events => no rewrite, no deletion"
    );
}
