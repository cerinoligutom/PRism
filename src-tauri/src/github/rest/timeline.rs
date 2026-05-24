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
    // Parse the page as a JSON array of opaque values first, then attempt
    // strongly-typed deserialisation per element. A single malformed entry
    // (GitHub shape drift, an undocumented event variant, a `reviewed` event
    // missing `submitted_at` because the review is in some pending limbo)
    // would otherwise abort the whole page and stall sync for the PR. The
    // skip path logs the event type so the failure is diagnosable from the
    // sync activity buffer rather than silently swallowed.
    let raw: Vec<serde_json::Value> = serde_json::from_slice(bytes)?;
    let mut out = Vec::with_capacity(raw.len());
    for value in raw {
        match serde_json::from_value::<RawTimelineEvent>(value.clone()) {
            Ok(evt) => {
                if let Some(timeline_event) = evt.into_event() {
                    out.push(timeline_event);
                }
            }
            Err(err) => {
                let kind = value
                    .get("event")
                    .and_then(|v| v.as_str())
                    .unwrap_or("<no event field>");
                tracing::debug!(event = kind, %err, "timeline: skipping element");
            }
        }
    }
    Ok(out)
}

/// Actor entry on a timeline event payload.
#[derive(Debug, Deserialize)]
struct RawActor {
    login: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

/// Reviewer entry on the `reviewed` event - GitHub puts the user under `user`
/// rather than `actor` on this variant alone.
#[derive(Debug, Deserialize)]
struct RawReviewedUser {
    login: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

/// Assignee subject on `assigned` / `unassigned` events. GitHub nests the
/// user under `assignee`; we only need the login to surface as the timeline
/// row's subject (ADR 0027).
#[derive(Debug, Deserialize)]
struct RawAssignee {
    login: String,
}

/// Label subject on `labeled` / `unlabeled` events. GitHub nests `{ name,
/// color }` under `label`; we only persist the name for the timeline row.
#[derive(Debug, Deserialize)]
struct RawLabel {
    name: String,
}

/// Milestone subject on `milestoned` / `demilestoned` events. GitHub nests
/// `{ title }` under `milestone`; we persist the title as the subject.
#[derive(Debug, Deserialize)]
struct RawMilestone {
    title: String,
}

/// Wire-shape for one element of the `/issues/{n}/timeline` list.
///
/// The set of variants covers two groups:
///
/// - the seven ADR 0007 status-change events (`ready_for_review`,
///   `convert_to_draft`, `review_requested`, `reviewed`, `merged`, `closed`,
///   `reopened`) which drive `latest_status_change_at`, and
/// - the ADR 0027 renderable-only events (`labeled` / `unlabeled`, `assigned`
///   / `unassigned`, `milestoned` / `demilestoned`, `head_ref_force_pushed`,
///   `base_ref_changed`, `locked` / `unlocked`) which only feed the timeline
///   tab.
///
/// `committed` is modelled explicitly so we can ignore it (it carries
/// `committer.date` rather than `created_at`). Unknown events fall through to
/// [`RawTimelineEvent::Other`].
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum RawTimelineEvent {
    ReadyForReview {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
    },
    ConvertToDraft {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
    },
    ReviewRequested {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
    },
    /// `reviewed` puts its timestamp under `submitted_at`, not `created_at`,
    /// and the reviewer under `user` rather than `actor`. The `state` field
    /// (`approved` / `changes_requested` / `commented` / `dismissed`) is
    /// surfaced lowercase by GitHub; we normalise to upper-case when
    /// persisting so the wire shape matches the GraphQL
    /// `PullRequestReviewState` enum the frontend already consumes.
    ///
    /// `submitted_at` is treated as optional: in practice every `reviewed`
    /// event ships with one, but GitHub has been observed to omit it on
    /// rare edge cases (in-progress reviews surfaced through the timeline,
    /// shape drift on legacy PRs). A missing timestamp drops the event in
    /// `into_event` rather than failing the whole page.
    Reviewed {
        #[serde(default, with = "time::serde::rfc3339::option")]
        submitted_at: Option<OffsetDateTime>,
        #[serde(default)]
        user: Option<RawReviewedUser>,
        #[serde(default)]
        state: Option<String>,
    },
    Merged {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
    },
    Closed {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
    },
    Reopened {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
    },
    Assigned {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
        #[serde(default)]
        assignee: Option<RawAssignee>,
    },
    Unassigned {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
        #[serde(default)]
        assignee: Option<RawAssignee>,
    },
    Labeled {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
        #[serde(default)]
        label: Option<RawLabel>,
    },
    Unlabeled {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
        #[serde(default)]
        label: Option<RawLabel>,
    },
    Milestoned {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
        #[serde(default)]
        milestone: Option<RawMilestone>,
    },
    Demilestoned {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
        #[serde(default)]
        milestone: Option<RawMilestone>,
    },
    HeadRefForcePushed {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
    },
    BaseRefChanged {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
    },
    Locked {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
    },
    Unlocked {
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(default)]
        actor: Option<RawActor>,
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
            Self::ReadyForReview { created_at, actor } => Some(event_from_actor(
                "ready_for_review",
                created_at,
                actor,
                None,
            )),
            Self::ConvertToDraft { created_at, actor } => Some(event_from_actor(
                "convert_to_draft",
                created_at,
                actor,
                None,
            )),
            Self::ReviewRequested { created_at, actor } => Some(event_from_actor(
                "review_requested",
                created_at,
                actor,
                None,
            )),
            Self::Reviewed {
                submitted_at,
                user,
                state,
            } => {
                let submitted_at = submitted_at?;
                let (actor_login, actor_avatar_url) = match user {
                    Some(u) => (Some(u.login), u.avatar_url),
                    None => (None, None),
                };
                Some(TimelineEvent {
                    event: "reviewed".into(),
                    created_at: submitted_at,
                    actor_login,
                    actor_avatar_url,
                    review_state: state.map(|s| s.to_uppercase()),
                    subject: None,
                })
            }
            Self::Merged { created_at, actor } => {
                Some(event_from_actor("merged", created_at, actor, None))
            }
            Self::Closed { created_at, actor } => {
                Some(event_from_actor("closed", created_at, actor, None))
            }
            Self::Reopened { created_at, actor } => {
                Some(event_from_actor("reopened", created_at, actor, None))
            }
            Self::Assigned {
                created_at,
                actor,
                assignee,
            } => Some(event_with_subject(
                "assigned",
                created_at,
                actor,
                assignee.map(|a| a.login),
            )),
            Self::Unassigned {
                created_at,
                actor,
                assignee,
            } => Some(event_with_subject(
                "unassigned",
                created_at,
                actor,
                assignee.map(|a| a.login),
            )),
            Self::Labeled {
                created_at,
                actor,
                label,
            } => Some(event_with_subject(
                "labeled",
                created_at,
                actor,
                label.map(|l| l.name),
            )),
            Self::Unlabeled {
                created_at,
                actor,
                label,
            } => Some(event_with_subject(
                "unlabeled",
                created_at,
                actor,
                label.map(|l| l.name),
            )),
            Self::Milestoned {
                created_at,
                actor,
                milestone,
            } => Some(event_with_subject(
                "milestoned",
                created_at,
                actor,
                milestone.map(|m| m.title),
            )),
            Self::Demilestoned {
                created_at,
                actor,
                milestone,
            } => Some(event_with_subject(
                "demilestoned",
                created_at,
                actor,
                milestone.map(|m| m.title),
            )),
            Self::HeadRefForcePushed { created_at, actor } => Some(event_with_subject(
                "head_ref_force_pushed",
                created_at,
                actor,
                None,
            )),
            Self::BaseRefChanged { created_at, actor } => Some(event_with_subject(
                "base_ref_changed",
                created_at,
                actor,
                None,
            )),
            Self::Locked { created_at, actor } => {
                Some(event_with_subject("locked", created_at, actor, None))
            }
            Self::Unlocked { created_at, actor } => {
                Some(event_with_subject("unlocked", created_at, actor, None))
            }
            Self::Committed | Self::Other => None,
        }
    }
}

fn event_from_actor(
    event: &str,
    created_at: OffsetDateTime,
    actor: Option<RawActor>,
    review_state: Option<String>,
) -> TimelineEvent {
    let (actor_login, actor_avatar_url) = match actor {
        Some(a) => (Some(a.login), a.avatar_url),
        None => (None, None),
    };
    TimelineEvent {
        event: event.into(),
        created_at,
        actor_login,
        actor_avatar_url,
        review_state,
        subject: None,
    }
}

/// Same as [`event_from_actor`] but populates the `subject` field for the
/// ADR 0027 renderable-only events that carry a secondary string (the label
/// name on `labeled`, the assignee on `assigned`, the milestone title on
/// `milestoned`, etc.). Events with no secondary subject pass `None`.
fn event_with_subject(
    event: &str,
    created_at: OffsetDateTime,
    actor: Option<RawActor>,
    subject: Option<String>,
) -> TimelineEvent {
    let (actor_login, actor_avatar_url) = match actor {
        Some(a) => (Some(a.login), a.avatar_url),
        None => (None, None),
    };
    TimelineEvent {
        event: event.into(),
        created_at,
        actor_login,
        actor_avatar_url,
        review_state: None,
        subject,
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
        // These three are not in the qualifying or renderable sets (the
        // renderable set was expanded in ADR 0027 but `subscribed`,
        // `cross-referenced`, and `mentioned` stay out per the ADR's
        // skipped-events list).
        let json = br#"[
            { "event": "subscribed", "created_at": "2026-05-01T09:00:00Z" },
            { "event": "mentioned", "created_at": "2026-05-01T09:00:00Z" },
            { "event": "cross-referenced", "created_at": "2026-05-01T09:00:00Z" }
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

    #[test]
    fn actor_login_carries_through_to_event() {
        let json = br#"[
            {
                "event": "ready_for_review",
                "created_at": "2026-05-02T14:30:00Z",
                "actor": { "login": "alice", "id": 1 }
            }
        ]"#;
        let events = parse_timeline_page(&Bytes::from_static(json)).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].actor_login.as_deref(), Some("alice"));
        assert!(events[0].review_state.is_none());
    }

    #[test]
    fn reviewed_event_carries_user_login_and_state_upper_cased() {
        let json = br#"[
            {
                "event": "reviewed",
                "submitted_at": "2026-05-03T10:00:00Z",
                "state": "approved",
                "user": { "login": "bob", "id": 2 }
            }
        ]"#;
        let events = parse_timeline_page(&Bytes::from_static(json)).unwrap();
        assert_eq!(events[0].actor_login.as_deref(), Some("bob"));
        assert_eq!(events[0].review_state.as_deref(), Some("APPROVED"));
    }

    #[test]
    fn event_with_missing_actor_falls_back_to_none() {
        let json = br#"[
            { "event": "closed", "created_at": "2026-05-06T11:00:00Z" }
        ]"#;
        let events = parse_timeline_page(&Bytes::from_static(json)).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].actor_login.is_none());
    }

    #[test]
    fn reviewed_event_without_submitted_at_is_skipped() {
        // GitHub has been observed (rarely) to surface a `reviewed` event with
        // no `submitted_at` - probably a pending review threaded into the
        // timeline. The page parser must drop the malformed entry rather than
        // abort, so the rest of the timeline still feeds the latest-status
        // derivation.
        let json = br#"[
            { "event": "reviewed", "state": "pending" },
            { "event": "closed", "created_at": "2026-05-06T11:00:00Z" }
        ]"#;
        let events = parse_timeline_page(&Bytes::from_static(json)).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "closed");
    }

    #[test]
    fn adr_0027_renderable_events_round_trip() {
        // ADR 0027: the timeline tab persists this wider set; the subject
        // column carries the label name / assignee login / milestone title
        // so the frontend can render the row without re-fetching.
        let json = br#"[
            {
                "event": "assigned",
                "created_at": "2026-05-25T09:00:00Z",
                "actor": { "login": "alice" },
                "assignee": { "login": "bob" }
            },
            {
                "event": "unassigned",
                "created_at": "2026-05-25T09:01:00Z",
                "actor": { "login": "alice" },
                "assignee": { "login": "bob" }
            },
            {
                "event": "labeled",
                "created_at": "2026-05-25T09:02:00Z",
                "actor": { "login": "alice" },
                "label": { "name": "bug", "color": "red" }
            },
            {
                "event": "unlabeled",
                "created_at": "2026-05-25T09:03:00Z",
                "actor": { "login": "alice" },
                "label": { "name": "bug", "color": "red" }
            },
            {
                "event": "milestoned",
                "created_at": "2026-05-25T09:04:00Z",
                "actor": { "login": "alice" },
                "milestone": { "title": "v1.0" }
            },
            {
                "event": "demilestoned",
                "created_at": "2026-05-25T09:05:00Z",
                "actor": { "login": "alice" },
                "milestone": { "title": "v1.0" }
            },
            {
                "event": "head_ref_force_pushed",
                "created_at": "2026-05-25T09:06:00Z",
                "actor": { "login": "alice" }
            },
            {
                "event": "base_ref_changed",
                "created_at": "2026-05-25T09:07:00Z",
                "actor": { "login": "alice" }
            },
            {
                "event": "locked",
                "created_at": "2026-05-25T09:08:00Z",
                "actor": { "login": "alice" }
            },
            {
                "event": "unlocked",
                "created_at": "2026-05-25T09:09:00Z",
                "actor": { "login": "alice" }
            }
        ]"#;
        let events = parse_timeline_page(&Bytes::from_static(json)).unwrap();
        let names: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "assigned",
                "unassigned",
                "labeled",
                "unlabeled",
                "milestoned",
                "demilestoned",
                "head_ref_force_pushed",
                "base_ref_changed",
                "locked",
                "unlocked",
            ],
        );
        // Subject fields are populated from the per-event payload key.
        assert_eq!(events[0].subject.as_deref(), Some("bob"));
        assert_eq!(events[2].subject.as_deref(), Some("bug"));
        assert_eq!(events[4].subject.as_deref(), Some("v1.0"));
        // Events with no payload-side subject leave the field None.
        assert!(events[6].subject.is_none());
        assert!(events[8].subject.is_none());
    }

    #[test]
    fn assigned_event_with_missing_assignee_keeps_subject_none() {
        // GitHub has been observed to omit the nested user payload on rare
        // edge cases (deleted assignee account, replay of a very old event).
        // The event must still parse - we keep the row and drop the subject.
        let json = br#"[
            { "event": "assigned", "created_at": "2026-05-25T09:00:00Z" }
        ]"#;
        let events = parse_timeline_page(&Bytes::from_static(json)).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "assigned");
        assert!(events[0].subject.is_none());
    }

    #[test]
    fn page_tolerates_individual_malformed_elements() {
        // Defensive guard: a single shape-drift element shouldn't kill the
        // whole page. The parser logs the bad element and keeps the rest.
        let json = br#"[
            { "event": "ready_for_review", "created_at": "2026-05-01T01:00:00Z" },
            { "event": "reviewed", "submitted_at": "not-a-timestamp", "state": "approved" },
            { "event": "merged", "created_at": "2026-05-01T05:00:00Z" }
        ]"#;
        let events = parse_timeline_page(&Bytes::from_static(json)).unwrap();
        let names: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
        assert_eq!(names, vec!["ready_for_review", "merged"]);
    }
}
