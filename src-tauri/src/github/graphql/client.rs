//! Higher-level GraphQL helpers built on top of [`GitHubClient`].
//!
//! These methods know about the specific v1 queries (PR detail, timeline) and
//! handle pagination for timeline events. Anything more bespoke should call
//! `GitHubClient::post_graphql` directly.

use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::github::client::GitHubClient;
use crate::github::error::GitHubError;
use crate::github::graphql::queries::{
    PrDetailData, PrTimelineData, PullRequestDetail, TimelineEvent, PR_DETAIL_QUERY,
    PR_TIMELINE_QUERY,
};

/// Repository + PR-number coordinates for a query.
#[derive(Debug, Clone)]
pub struct PrCoord<'a> {
    pub owner: &'a str,
    pub name: &'a str,
    pub number: i64,
}

impl GitHubClient {
    /// Fetch the PR detail payload (including review-thread resolution state).
    pub async fn pr_detail(
        &self,
        coord: PrCoord<'_>,
    ) -> Result<Option<PullRequestDetail>, GitHubError> {
        let (detail, _body) = self.pr_detail_with_raw(coord).await?;
        Ok(detail)
    }

    /// Variant of [`pr_detail`] that returns the raw response bytes alongside
    /// the parsed detail. The sync worker hashes the bytes against the cache
    /// (issue #234) to skip the per-PR DB writes when the upstream response is
    /// byte-identical to last cycle.
    pub async fn pr_detail_with_raw(
        &self,
        coord: PrCoord<'_>,
    ) -> Result<(Option<PullRequestDetail>, Bytes), GitHubError> {
        let (data, body): (PrDetailData, Bytes) = self
            .post_graphql_with_raw(
                PR_DETAIL_QUERY,
                json!({
                    "owner": coord.owner,
                    "name": coord.name,
                    "number": coord.number,
                }),
            )
            .await?;
        Ok((data.repository.and_then(|r| r.pull_request), body))
    }

    /// Fetch a single page of timeline events.
    ///
    /// Returns the event list and the next cursor (if any). Callers walk
    /// pages newest-first and stop as soon as a qualifying event is found.
    pub async fn pr_timeline_page(
        &self,
        coord: PrCoord<'_>,
        after: Option<&str>,
    ) -> Result<TimelinePage, GitHubError> {
        let data: PrTimelineData = self
            .post_graphql(
                PR_TIMELINE_QUERY,
                json!({
                    "owner": coord.owner,
                    "name": coord.name,
                    "number": coord.number,
                    "after": after,
                }),
            )
            .await?;

        let timeline = data
            .repository
            .and_then(|r| r.pull_request)
            .map(|pr| pr.timeline_items);

        Ok(match timeline {
            Some(t) => TimelinePage {
                events: t.nodes,
                next_cursor: if t.page_info.has_next_page {
                    t.page_info.end_cursor
                } else {
                    None
                },
            },
            None => TimelinePage::default(),
        })
    }

    /// Walk every timeline page, collecting events. Use [`pr_timeline_page`] for
    /// short-circuit walks.
    ///
    /// `max_pages` bounds the loop to avoid runaway iteration on PRs with
    /// adversarially long histories.
    pub async fn pr_timeline_all(
        &self,
        coord: PrCoord<'_>,
        max_pages: usize,
    ) -> Result<Vec<TimelineEvent>, GitHubError> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..max_pages {
            let page = self
                .pr_timeline_page(coord.clone(), cursor.as_deref())
                .await?;
            all.extend(page.events);
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(all)
    }

    /// Escape hatch for callers that need a custom query shape but want the
    /// same envelope handling and rate-limit accounting.
    pub async fn graphql<T>(&self, query: &str, vars: serde_json::Value) -> Result<T, GitHubError>
    where
        T: DeserializeOwned,
    {
        self.post_graphql(query, vars).await
    }

    /// Body-capturing variant of [`graphql`]. Discovery (issue #234) uses this
    /// to hash the raw response against the GraphQL body cache before deciding
    /// whether to run the per-node ingest path.
    pub async fn graphql_with_raw<T>(
        &self,
        query: &str,
        vars: serde_json::Value,
    ) -> Result<(T, Bytes), GitHubError>
    where
        T: DeserializeOwned,
    {
        self.post_graphql_with_raw(query, vars).await
    }
}

#[derive(Debug, Default)]
pub struct TimelinePage {
    pub events: Vec<TimelineEvent>,
    pub next_cursor: Option<String>,
}
