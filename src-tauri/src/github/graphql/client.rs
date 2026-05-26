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
    /// Fetch the first page of the PR detail payload (review-thread state,
    /// up to 100 threads + 100 issue comments).
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
                    "threadsAfter": serde_json::Value::Null,
                    "issueCommentsAfter": serde_json::Value::Null,
                }),
            )
            .await?;
        Ok((data.repository.and_then(|r| r.pull_request), body))
    }

    /// Fetch a single follow-up page of the PR detail. Used by
    /// [`pr_detail_extend_pages`] to drain remaining thread / issue-comment
    /// pages after the first call landed. `threads_after` / `issues_after`
    /// drive the two paginated connections independently — pass `None` for a
    /// connection that has already been drained.
    async fn pr_detail_page(
        &self,
        coord: PrCoord<'_>,
        threads_after: Option<&str>,
        issues_after: Option<&str>,
    ) -> Result<Option<PullRequestDetail>, GitHubError> {
        let data: PrDetailData = self
            .post_graphql(
                PR_DETAIL_QUERY,
                json!({
                    "owner": coord.owner,
                    "name": coord.name,
                    "number": coord.number,
                    "threadsAfter": threads_after,
                    "issueCommentsAfter": issues_after,
                }),
            )
            .await?;
        Ok(data.repository.and_then(|r| r.pull_request))
    }

    /// Walk the remaining `reviewThreads` and `issueComments` pages and merge
    /// them into `detail`. The first page lands via [`pr_detail`] /
    /// [`pr_detail_with_raw`]; this method is a no-op when both connections
    /// already report `hasNextPage = false`. Stops at `max_pages` per
    /// connection as a defensive backstop — ADR 0029 sets four for the v1
    /// surface, well above any realistic PR.
    ///
    /// The merge is append-only: each follow-up page returns distinct thread
    /// nodes / distinct issue-comment nodes, and per-thread comments are
    /// already capped at `first: 100` on the first page (ADR 0029 doesn't
    /// paginate inside a thread).
    pub async fn pr_detail_extend_pages(
        &self,
        coord: PrCoord<'_>,
        detail: &mut PullRequestDetail,
        max_pages: usize,
    ) -> Result<(), GitHubError> {
        let mut threads_cursor = next_cursor(&detail.review_threads.page_info);
        let mut issues_cursor = detail
            .issue_comments
            .as_ref()
            .and_then(|ic| next_cursor(&ic.page_info));
        let mut iterations = 0usize;
        while iterations < max_pages && (threads_cursor.is_some() || issues_cursor.is_some()) {
            let Some(next) = self
                .pr_detail_page(
                    coord.clone(),
                    threads_cursor.as_deref(),
                    issues_cursor.as_deref(),
                )
                .await?
            else {
                break;
            };

            if threads_cursor.is_some() {
                detail
                    .review_threads
                    .nodes
                    .extend(next.review_threads.nodes);
                detail.review_threads.page_info = next.review_threads.page_info;
                threads_cursor = next_cursor(&detail.review_threads.page_info);
            }

            if issues_cursor.is_some() {
                match (detail.issue_comments.as_mut(), next.issue_comments) {
                    (Some(into), Some(from)) => {
                        into.nodes.extend(from.nodes);
                        into.page_info = from.page_info;
                        issues_cursor = next_cursor(&into.page_info);
                    }
                    (None, Some(from)) => {
                        // First page didn't carry an issueComments block but a
                        // follow-up did. Defensive: graft the page in.
                        detail.issue_comments = Some(from);
                        issues_cursor = detail
                            .issue_comments
                            .as_ref()
                            .and_then(|ic| next_cursor(&ic.page_info));
                    }
                    _ => {
                        issues_cursor = None;
                    }
                }
            }

            iterations += 1;
        }
        Ok(())
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

fn next_cursor(page_info: &crate::github::graphql::PageInfo) -> Option<String> {
    if page_info.has_next_page {
        page_info.end_cursor.clone()
    } else {
        None
    }
}
