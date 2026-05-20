//! Derives "latest status change" from a PR's timeline events.
//!
//! See ADR 0007 for the policy: a fixed set of qualifying GitHub timeline
//! event types contribute to PRism's per-PR "time since latest status change"
//! surface, and the most recent qualifying event wins.

use serde::Deserialize;
use time::OffsetDateTime;

/// GitHub timeline event types that count as a status change for PRism.
///
/// Kept as a closed enum (per ADR 0007's "finite enum" requirement) so the
/// compiler flags drift if GitHub adds a relevant event type and we forget
/// to wire it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualifyingEvent {
    ReadyForReview,
    ConvertToDraft,
    ReviewRequested,
    Reviewed,
    Merged,
    Closed,
    Reopened,
}

impl QualifyingEvent {
    /// Match GitHub's wire-format event name to a qualifying variant.
    /// Returns `None` for non-qualifying events (e.g. `labeled`, `assigned`).
    fn from_wire(event: &str) -> Option<Self> {
        match event {
            "ready_for_review" => Some(Self::ReadyForReview),
            "convert_to_draft" => Some(Self::ConvertToDraft),
            "review_requested" => Some(Self::ReviewRequested),
            "reviewed" => Some(Self::Reviewed),
            "merged" => Some(Self::Merged),
            "closed" => Some(Self::Closed),
            "reopened" => Some(Self::Reopened),
            _ => None,
        }
    }
}

/// A single timeline event as surfaced by the GitHub REST timeline API.
///
/// The `event` + `created_at` pair drives the latest-status-change derivation
/// (see [`latest_status_change`]). `actor_login` and `review_state` carry the
/// extra context the M3 persistence path writes into `timeline_events`; they
/// are `None` when GitHub did not surface the field (e.g. system-generated
/// events with no actor, or non-`reviewed` events for which `review_state` is
/// meaningless).
#[derive(Debug, Clone, Deserialize)]
pub struct TimelineEvent {
    /// GitHub's `event` string, e.g. `"ready_for_review"`, `"labeled"`.
    pub event: String,
    /// ISO-8601 timestamp from GitHub. Parsed via `serde-well-known` (RFC 3339).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Actor login (the user who triggered the event). `None` for events with
    /// no associated user (rare; mostly system-driven `closed` events).
    #[serde(default)]
    pub actor_login: Option<String>,
    /// Review state for `reviewed` events: `APPROVED`, `CHANGES_REQUESTED`,
    /// `COMMENTED`, `DISMISSED`. `None` for non-`reviewed` events.
    #[serde(default)]
    pub review_state: Option<String>,
}

/// The result of derivation: the qualifying event type and its timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatestStatusChange {
    pub event_type: QualifyingEvent,
    pub at: OffsetDateTime,
}

/// Pick the latest qualifying status-change event from a timeline slice.
///
/// Returns `None` when no event in the input qualifies. Ties on `created_at`
/// are broken by input order: the later index wins, matching GitHub's
/// timeline-events API contract (events arrive oldest-first, newest-last,
/// so the later-indexed event is the more recent one).
pub fn latest_status_change(events: &[TimelineEvent]) -> Option<LatestStatusChange> {
    let mut best: Option<LatestStatusChange> = None;

    for event in events {
        let Some(kind) = QualifyingEvent::from_wire(&event.event) else {
            continue;
        };
        let candidate = LatestStatusChange {
            event_type: kind,
            at: event.created_at,
        };
        // `>=` rather than `>` so that on a tie the later-indexed event wins.
        match best {
            None => best = Some(candidate),
            Some(current) if candidate.at >= current.at => best = Some(candidate),
            _ => {}
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn evt(kind: &str, at: OffsetDateTime) -> TimelineEvent {
        TimelineEvent {
            event: kind.to_string(),
            created_at: at,
            actor_login: None,
            review_state: None,
        }
    }

    #[test]
    fn empty_input_returns_none() {
        assert_eq!(latest_status_change(&[]), None);
    }

    #[test]
    fn each_qualifying_type_is_recognised() {
        let cases: &[(&str, QualifyingEvent)] = &[
            ("ready_for_review", QualifyingEvent::ReadyForReview),
            ("convert_to_draft", QualifyingEvent::ConvertToDraft),
            ("review_requested", QualifyingEvent::ReviewRequested),
            ("reviewed", QualifyingEvent::Reviewed),
            ("merged", QualifyingEvent::Merged),
            ("closed", QualifyingEvent::Closed),
            ("reopened", QualifyingEvent::Reopened),
        ];
        let ts = datetime!(2026-05-19 10:00:00 UTC);
        for (wire, expected) in cases {
            let result = latest_status_change(&[evt(wire, ts)]);
            assert_eq!(
                result,
                Some(LatestStatusChange {
                    event_type: *expected,
                    at: ts,
                }),
                "wire event {wire} should map to {expected:?}",
            );
        }
    }

    #[test]
    fn non_qualifying_types_are_ignored() {
        let ts = datetime!(2026-05-19 10:00:00 UTC);
        let non_qualifying = ["labeled", "assigned", "commented", "renamed", "subscribed"];
        for kind in non_qualifying {
            assert_eq!(
                latest_status_change(&[evt(kind, ts)]),
                None,
                "{kind} should not qualify",
            );
        }
    }

    #[test]
    fn picks_latest_by_timestamp_regardless_of_input_order() {
        let early = datetime!(2026-05-19 10:00:00 UTC);
        let mid = datetime!(2026-05-19 11:00:00 UTC);
        let late = datetime!(2026-05-19 12:00:00 UTC);

        // Latest event placed in the middle of the input.
        let events = [
            evt("review_requested", early),
            evt("reviewed", late),
            evt("ready_for_review", mid),
        ];

        assert_eq!(
            latest_status_change(&events),
            Some(LatestStatusChange {
                event_type: QualifyingEvent::Reviewed,
                at: late,
            }),
        );
    }

    #[test]
    fn non_qualifying_events_do_not_displace_qualifying_ones() {
        let early = datetime!(2026-05-19 10:00:00 UTC);
        let later = datetime!(2026-05-19 13:00:00 UTC);

        let events = [
            evt("ready_for_review", early),
            evt("labeled", later),
            evt("commented", later),
        ];

        assert_eq!(
            latest_status_change(&events),
            Some(LatestStatusChange {
                event_type: QualifyingEvent::ReadyForReview,
                at: early,
            }),
        );
    }

    #[test]
    fn tie_on_timestamp_resolves_to_later_input_index() {
        // GitHub's timeline API returns events oldest-first, newest-last,
        // so on equal `created_at` the later-indexed event is the more recent
        // wall-clock event and must win.
        let ts = datetime!(2026-05-19 10:00:00 UTC);
        let events = [
            evt("review_requested", ts),
            evt("reviewed", ts),
            evt("ready_for_review", ts),
        ];

        assert_eq!(
            latest_status_change(&events),
            Some(LatestStatusChange {
                event_type: QualifyingEvent::ReadyForReview,
                at: ts,
            }),
        );
    }

    #[test]
    fn realistic_pr_lifecycle_picks_most_recent_qualifier() {
        // Sequence echoes the ADR 0007 narrative: a draft PR is marked
        // ready, gets review requests and reviews, then is flipped back
        // to draft. The convert_to_draft is the latest status change.
        let events = [
            evt("labeled", datetime!(2026-05-01 09:00:00 UTC)),
            evt("ready_for_review", datetime!(2026-05-02 14:30:00 UTC)),
            evt("review_requested", datetime!(2026-05-02 14:35:00 UTC)),
            evt("commented", datetime!(2026-05-03 08:15:00 UTC)),
            evt("reviewed", datetime!(2026-05-03 10:00:00 UTC)),
            evt("convert_to_draft", datetime!(2026-05-04 16:45:00 UTC)),
            evt("labeled", datetime!(2026-05-05 09:00:00 UTC)),
        ];

        assert_eq!(
            latest_status_change(&events),
            Some(LatestStatusChange {
                event_type: QualifyingEvent::ConvertToDraft,
                at: datetime!(2026-05-04 16:45:00 UTC),
            }),
        );
    }

    #[test]
    fn merged_then_reopened_lifecycle() {
        // A PR merged then reopened (rare, but possible via the API) should
        // surface the reopen as the latest status change.
        let events = [
            evt("ready_for_review", datetime!(2026-05-10 09:00:00 UTC)),
            evt("merged", datetime!(2026-05-11 17:00:00 UTC)),
            evt("closed", datetime!(2026-05-11 17:00:00 UTC)),
            evt("reopened", datetime!(2026-05-12 08:30:00 UTC)),
        ];

        assert_eq!(
            latest_status_change(&events),
            Some(LatestStatusChange {
                event_type: QualifyingEvent::Reopened,
                at: datetime!(2026-05-12 08:30:00 UTC),
            }),
        );
    }

    #[test]
    fn deserialises_from_github_timeline_payload_shape() {
        // Verify the `TimelineEvent` deserialiser handles the RFC 3339
        // timestamp format GitHub emits.
        let json = r#"[
            {"event":"labeled","created_at":"2026-05-01T09:00:00Z"},
            {"event":"ready_for_review","created_at":"2026-05-02T14:30:00Z"},
            {"event":"reviewed","created_at":"2026-05-03T10:00:00Z"}
        ]"#;
        let parsed: Vec<TimelineEvent> = serde_json::from_str(json).expect("parse");
        assert_eq!(parsed.len(), 3);
        let derived = latest_status_change(&parsed).expect("derived");
        assert_eq!(derived.event_type, QualifyingEvent::Reviewed);
        assert_eq!(derived.at, datetime!(2026-05-03 10:00:00 UTC));
    }
}
