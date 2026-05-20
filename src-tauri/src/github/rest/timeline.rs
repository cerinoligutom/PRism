//! REST wrapper for `GET /repos/{owner}/{repo}/issues/{number}/timeline`.
//!
//! GitHub's timeline payload is non-uniform: most events carry the relevant
//! timestamp as `created_at`, the `reviewed` event surfaces it as
//! `submitted_at` (mirroring the `pull_request_review` payload shape), and the
//! `committed` event uses `committer.date`. We normalise on the wire so the
//! downstream derivation (see ADR 0007 and
//! [`crate::sync::status_timeline::latest_status_change`]) only ever sees a
//! `created_at`-shaped [`TimelineEvent`].
//!
//! Pagination is intentionally single-page in v1. GitHub's REST list endpoints
//! advertise further pages via the RFC 5988 `Link` header, which the shared
//! [`GitHubClient::get_conditional`] helper doesn't yet expose. We ask for
//! `per_page=100` to fit the qualifying events of the overwhelming majority of
//! PRs into one round trip; truly long-lived PRs (>100 timeline events) are
//! tracked under follow-up work to surface `Link` from the conditional helper.
//! The `max_pages` knob is kept on the public signature so callers don't have
//! to change once that follow-up lands.

use bytes::Bytes;
use serde::Deserialize;
use time::OffsetDateTime;

use crate::github::client::{Conditional, GitHubClient};
use crate::github::error::GitHubError;
use crate::sync::status_timeline::TimelineEvent;

/// Coordinates for a repository in a REST path.
#[derive(Debug, Clone, Copy)]
pub struct RepoCoord<'a> {
    pub owner: &'a str,
    pub repo: &'a str,
}

/// Result of a conditional timeline fetch.
#[derive(Debug)]
pub enum ListTimeline {
    /// Upstream returned 304; the cached timeline is still authoritative.
    NotModified,
    /// Fresh events from upstream, qualifying ones only.
    Events(Vec<TimelineEvent>),
}

impl ListTimeline {
    pub fn is_modified(&self) -> bool {
        matches!(self, ListTimeline::Events(_))
    }
}

/// Fetch the qualifying timeline events for a PR.
///
/// `_max_pages` is reserved for the multi-page walk once the conditional
/// helper exposes the `Link` header; v1 fetches a single page of up to 100
/// events and ignores it. A 304 short-circuits to [`ListTimeline::NotModified`]
/// so the caller can skip recomputation.
pub async fn list_pr_timeline(
    client: &GitHubClient,
    repo: RepoCoord<'_>,
    pr_number: u32,
    _max_pages: usize,
) -> Result<ListTimeline, GitHubError> {
    let path = format!(
        "/repos/{}/{}/issues/{}/timeline?per_page=100",
        repo.owner, repo.repo, pr_number
    );

    match client.get_conditional(&path).await? {
        Conditional::NotModified => Ok(ListTimeline::NotModified),
        Conditional::Modified { body, .. } => {
            let events = parse_timeline_page(&body)?;
            Ok(ListTimeline::Events(events))
        }
    }
}

fn parse_timeline_page(bytes: &Bytes) -> Result<Vec<TimelineEvent>, GitHubError> {
    let raw: Vec<RawTimelineEvent> = serde_json::from_slice(bytes)?;
    Ok(raw
        .into_iter()
        .filter_map(RawTimelineEvent::into_event)
        .collect())
}

/// Wire-shape for one element of the `/issues/{n}/timeline` list.
///
/// The set of variants intentionally mirrors `QualifyingEvent` plus the
/// `committed` outlier (modelled so we can ignore it explicitly rather than
/// silently). Unknown events fall through to [`RawTimelineEvent::Other`].
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum RawTimelineEvent {
    ReadyForReview {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    },
    ConvertToDraft {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    },
    ReviewRequested {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    },
    /// `reviewed` puts its timestamp under `submitted_at`, not `created_at`.
    Reviewed {
        #[serde(with = "time::serde::rfc3339")]
        submitted_at: OffsetDateTime,
    },
    Merged {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    },
    Closed {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    },
    Reopened {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    },
    /// `committed` carries `committer.date` instead of a top-level timestamp
    /// and is not a qualifying status-change event. We model it explicitly to
    /// document the carve-out; deserialisation tolerates the missing
    /// `created_at`.
    Committed,
    #[serde(other)]
    Other,
}

impl RawTimelineEvent {
    fn into_event(self) -> Option<TimelineEvent> {
        match self {
            Self::ReadyForReview { created_at } => Some(TimelineEvent {
                event: "ready_for_review".into(),
                created_at,
            }),
            Self::ConvertToDraft { created_at } => Some(TimelineEvent {
                event: "convert_to_draft".into(),
                created_at,
            }),
            Self::ReviewRequested { created_at } => Some(TimelineEvent {
                event: "review_requested".into(),
                created_at,
            }),
            Self::Reviewed { submitted_at } => Some(TimelineEvent {
                event: "reviewed".into(),
                created_at: submitted_at,
            }),
            Self::Merged { created_at } => Some(TimelineEvent {
                event: "merged".into(),
                created_at,
            }),
            Self::Closed { created_at } => Some(TimelineEvent {
                event: "closed".into(),
                created_at,
            }),
            Self::Reopened { created_at } => Some(TimelineEvent {
                event: "reopened".into(),
                created_at,
            }),
            Self::Committed | Self::Other => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn reviewed_event_pulls_submitted_at_not_created_at() {
        let json = br#"[
            {
                "event": "reviewed",
                "submitted_at": "2026-05-03T10:00:00Z",
                "state": "approved"
            }
        ]"#;
        let events = parse_timeline_page(&Bytes::from_static(json)).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "reviewed");
        assert_eq!(events[0].created_at, datetime!(2026-05-03 10:00:00 UTC));
    }

    #[test]
    fn committed_event_is_dropped() {
        // `committed` carries `committer.date` rather than `created_at` and
        // is not a qualifying status-change event.
        let json = br#"[
            {
                "event": "committed",
                "sha": "abc",
                "committer": { "date": "2026-05-03T10:00:00Z" }
            }
        ]"#;
        let events = parse_timeline_page(&Bytes::from_static(json)).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn unknown_event_types_are_dropped() {
        let json = br#"[
            { "event": "labeled", "created_at": "2026-05-01T09:00:00Z" },
            { "event": "assigned", "created_at": "2026-05-01T09:00:00Z" }
        ]"#;
        let events = parse_timeline_page(&Bytes::from_static(json)).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn every_qualifying_event_round_trips() {
        let json = br#"[
            { "event": "ready_for_review", "created_at": "2026-05-01T01:00:00Z" },
            { "event": "convert_to_draft", "created_at": "2026-05-01T02:00:00Z" },
            { "event": "review_requested", "created_at": "2026-05-01T03:00:00Z" },
            { "event": "reviewed", "submitted_at": "2026-05-01T04:00:00Z", "state": "approved" },
            { "event": "merged", "created_at": "2026-05-01T05:00:00Z" },
            { "event": "closed", "created_at": "2026-05-01T06:00:00Z" },
            { "event": "reopened", "created_at": "2026-05-01T07:00:00Z" }
        ]"#;
        let events = parse_timeline_page(&Bytes::from_static(json)).unwrap();
        let names: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "ready_for_review",
                "convert_to_draft",
                "review_requested",
                "reviewed",
                "merged",
                "closed",
                "reopened",
            ]
        );
    }

    #[test]
    fn list_timeline_is_modified_predicate() {
        assert!(ListTimeline::Events(vec![]).is_modified());
        assert!(!ListTimeline::NotModified.is_modified());
    }
}
