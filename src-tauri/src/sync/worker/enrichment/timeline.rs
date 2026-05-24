//! Timeline-event writes: wipe-and-rewrite per cycle for the PR plus the
//! payload shape used by the timeline tab.

use rusqlite::params;

/// Persist the qualifying timeline events for a PR.
///
/// Wipe-and-rewrite per cycle: GitHub timelines are append-only on the server,
/// so the latest fetch is authoritative for the PR's history. The wipe handles
/// rare cases where GitHub itself surfaces a corrected event ordering (e.g. a
/// backfill after support intervention) and keeps the table consistent with the
/// derivation that runs alongside this call.
///
/// `payload` stores per-event JSON for fields not modelled as dedicated
/// columns. Today the only consumer is `reviewed` events, which persist
/// `{"state": "APPROVED" | "CHANGES_REQUESTED" | ...}` so the timeline tab can
/// render the right badge without parsing the event type plus an out-of-band
/// state column.
pub(super) fn write_timeline_events(
    tx: &rusqlite::Transaction<'_>,
    pr_id: i64,
    events: &[crate::sync::status_timeline::TimelineEvent],
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "DELETE FROM timeline_events WHERE pull_request_id = ?1",
        params![pr_id],
    )?;
    for event in events {
        let payload = timeline_event_payload(event);
        tx.execute(
            "INSERT INTO timeline_events
                (pull_request_id, event_type, actor_login, created_at, payload)
                VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                pr_id,
                event.event,
                event.actor_login,
                event.created_at.unix_timestamp(),
                payload,
            ],
        )?;
    }
    Ok(())
}

/// Build the `payload` JSON column for one timeline event.
///
/// Two optional keys land in the JSON object today:
///
/// - `state` on `reviewed` events (`APPROVED`, `CHANGES_REQUESTED`,
///   `COMMENTED`, `DISMISSED`), already present pre-ADR 0027.
/// - `subject` on ADR 0027 renderable-only events that carry a secondary
///   string: the label name on `labeled` / `unlabeled`, the assignee login on
///   `assigned` / `unassigned`, the milestone title on `milestoned` /
///   `demilestoned`. Events with no secondary subject (status-change events
///   and the actor-only force-push / base-ref / lock events) omit the key.
///
/// Persisting `{}` rather than NULL when both are absent keeps the column's
/// NOT NULL invariant in 0001_init.sql.
pub(super) fn timeline_event_payload(
    event: &crate::sync::status_timeline::TimelineEvent,
) -> String {
    let mut obj = serde_json::Map::new();
    if let Some(state) = event.review_state.as_deref() {
        obj.insert("state".into(), serde_json::Value::String(state.into()));
    }
    if let Some(subject) = event.subject.as_deref() {
        obj.insert("subject".into(), serde_json::Value::String(subject.into()));
    }
    serde_json::Value::Object(obj).to_string()
}

pub(super) fn qualifying_event_wire_name(
    ev: crate::sync::status_timeline::QualifyingEvent,
) -> &'static str {
    use crate::sync::status_timeline::QualifyingEvent::*;
    match ev {
        ReadyForReview => "ready_for_review",
        ConvertToDraft => "convert_to_draft",
        ReviewRequested => "review_requested",
        Reviewed => "reviewed",
        Merged => "merged",
        Closed => "closed",
        Reopened => "reopened",
    }
}
