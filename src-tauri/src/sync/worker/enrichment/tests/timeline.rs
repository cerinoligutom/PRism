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
        subject: None,
    }
}

fn tle_with_subject(
    kind: &str,
    at: time::OffsetDateTime,
    actor: Option<&str>,
    subject: Option<&str>,
) -> TimelineEvent {
    TimelineEvent {
        event: kind.into(),
        created_at: at,
        actor_login: actor.map(str::to_string),
        actor_avatar_url: None,
        review_state: None,
        subject: subject.map(str::to_string),
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
fn timeline_event_payload_emits_subject_for_adr_0027_renderable_events() {
    // The label-name / assignee / milestone string lives under `subject` so
    // the frontend can render the row without re-fetching from GitHub.
    let payload = timeline_event_payload(&tle_with_subject(
        "labeled",
        datetime!(2026-05-25 09:00:00 UTC),
        Some("alice"),
        Some("bug"),
    ));
    assert_eq!(payload, r#"{"subject":"bug"}"#);

    // Renderable events with no subject (force-push, base-ref, lock) emit
    // the empty object - same shape as status-change events without state.
    let payload = timeline_event_payload(&tle_with_subject(
        "head_ref_force_pushed",
        datetime!(2026-05-25 09:00:00 UTC),
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
fn write_pr_updates_persists_adr_0027_renderable_events_with_subject() {
    // The wider set from ADR 0027 (labeled, assigned, milestoned,
    // head_ref_force_pushed, base_ref_changed, locked) lands in
    // `timeline_events` alongside the ADR 0007 status-change set. The
    // `subject` payload field carries the label name / assignee / milestone
    // title for the renderer.
    let (db, repo_id, pr_id) = seed_db_with_pr();
    let events = vec![
        tle_with_subject(
            "labeled",
            datetime!(2026-05-25 09:00:00 UTC),
            Some("alice"),
            Some("bug"),
        ),
        tle_with_subject(
            "assigned",
            datetime!(2026-05-25 09:01:00 UTC),
            Some("alice"),
            Some("bob"),
        ),
        tle_with_subject(
            "milestoned",
            datetime!(2026-05-25 09:02:00 UTC),
            Some("alice"),
            Some("v1.0"),
        ),
        tle_with_subject(
            "head_ref_force_pushed",
            datetime!(2026-05-25 09:03:00 UTC),
            Some("alice"),
            None,
        ),
        tle_with_subject(
            "locked",
            datetime!(2026-05-25 09:04:00 UTC),
            Some("alice"),
            None,
        ),
    ];
    write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&events)).unwrap();

    let rows = read_timeline_events(&db, pr_id);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].0, "labeled");
    assert_eq!(rows[0].3, r#"{"subject":"bug"}"#);
    assert_eq!(rows[1].0, "assigned");
    assert_eq!(rows[1].3, r#"{"subject":"bob"}"#);
    assert_eq!(rows[2].0, "milestoned");
    assert_eq!(rows[2].3, r#"{"subject":"v1.0"}"#);
    assert_eq!(rows[3].0, "head_ref_force_pushed");
    assert_eq!(rows[3].3, "{}");
    assert_eq!(rows[4].0, "locked");
    assert_eq!(rows[4].3, "{}");
}

#[test]
fn write_pr_updates_renderable_events_do_not_bump_latest_status_change() {
    // ADR 0027 invariant: the renderable-only event set is persisted to
    // `timeline_events` but must not advance `latest_status_change_at`.
    let (db, repo_id, pr_id) = seed_db_with_pr();

    // First cycle lands the status-change baseline.
    let cycle1 = vec![tle(
        "ready_for_review",
        datetime!(2026-05-02 14:30:00 UTC),
        Some("alice"),
        None,
    )];
    write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&cycle1)).unwrap();

    let baseline_status_at: Option<i64> = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT latest_status_change_at FROM pull_requests WHERE id = ?1",
            params![pr_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(baseline_status_at.is_some());

    // Second cycle adds renderable-only events at a later timestamp.
    let cycle2 = vec![
        tle(
            "ready_for_review",
            datetime!(2026-05-02 14:30:00 UTC),
            Some("alice"),
            None,
        ),
        tle_with_subject(
            "labeled",
            datetime!(2026-05-25 09:00:00 UTC),
            Some("alice"),
            Some("bug"),
        ),
    ];
    write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&cycle2)).unwrap();

    let after_status_at: Option<i64> = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT latest_status_change_at FROM pull_requests WHERE id = ?1",
            params![pr_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        after_status_at, baseline_status_at,
        "labeled is renderable-only; latest_status_change_at must not move",
    );

    // But both event types ARE persisted to the table.
    let rows = read_timeline_events(&db, pr_id);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|r| r.0 == "labeled"));
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
