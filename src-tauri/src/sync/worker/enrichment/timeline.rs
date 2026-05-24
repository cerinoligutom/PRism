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
/// `reviewed` events carry the review state (`APPROVED`, `CHANGES_REQUESTED`,
/// `COMMENTED`, `DISMISSED`); all other qualifying events produce `{}` because
/// no auxiliary field exists for them today. Persisting a value rather than
/// NULL keeps the `payload` column's NOT NULL invariant in 0001_init.sql.
pub(super) fn timeline_event_payload(
    event: &crate::sync::status_timeline::TimelineEvent,
) -> String {
    match event.review_state.as_deref() {
        Some(state) => serde_json::json!({ "state": state }).to_string(),
        None => "{}".to_string(),
    }
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
