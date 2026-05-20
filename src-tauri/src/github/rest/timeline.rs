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
//! Pagination follows the RFC 5988 `Link` header: each response advertises the
//! next page under `rel="next"`. We walk until either no `next` link is present
//! or the `max_pages` cap is reached. A 304 on the first page short-circuits
//! to [`ListTimeline::NotModified`]; a 304 on a later page just stops the walk
//! (rare in practice given GitHub's per-page ETag behaviour). Each page is
//! cached independently in the ETag store keyed by its path+query.

use bytes::Bytes;
use serde::Deserialize;
use time::OffsetDateTime;
use url::Url;

use crate::github::client::{parse_next_link, Conditional, GitHubClient};
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

/// Fetch the qualifying timeline events for a PR, walking `Link rel="next"`
/// until exhausted or `max_pages` is hit.
///
/// A 304 on page 1 short-circuits to [`ListTimeline::NotModified`] so the
/// caller can skip recomputation entirely. On later pages a 304 just stops the
/// walk — pages already fetched stay in the returned vector. `per_page=100`
/// keeps the round-trip count low for the typical PR while still letting
/// long-lived PRs (>100 timeline events) reconstruct fully.
pub async fn list_pr_timeline(
    client: &GitHubClient,
    repo: RepoCoord<'_>,
    pr_number: u32,
    max_pages: usize,
) -> Result<ListTimeline, GitHubError> {
    let mut path = format!(
        "/repos/{}/{}/issues/{}/timeline?per_page=100",
        repo.owner, repo.repo, pr_number
    );
    let mut all_events: Vec<TimelineEvent> = Vec::new();

    for page_index in 0..max_pages.max(1) {
        match client.get_conditional(&path).await? {
            Conditional::NotModified => {
                if page_index == 0 {
                    return Ok(ListTimeline::NotModified);
                }
                break;
            }
            Conditional::Modified { body, headers, .. } => {
                all_events.extend(parse_timeline_page(&body)?);
                match parse_next_link(&headers).and_then(|s| relative_path(&s)) {
                    Some(next) => path = next,
                    None => break,
                }
            }
        }
    }

    Ok(ListTimeline::Events(all_events))
}

/// Strip scheme + host from an absolute URL emitted by GitHub's `Link` header,
/// leaving `/path?query` so it can be fed back to `client.get_conditional`
/// (which is path-relative and keys the ETag store by path).
fn relative_path(absolute: &str) -> Option<String> {
    let url = Url::parse(absolute).ok()?;
    let mut out = url.path().to_string();
    if let Some(q) = url.query() {
        out.push('?');
        out.push_str(q);
    }
    Some(out)
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
