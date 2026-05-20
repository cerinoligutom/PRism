//! PR discovery via the GitHub Search API (ADR 0009).
//!
//! Runs as the first phase of every sync cycle, per account. Three search
//! queries (Authored / Review-requested / Involves) fan out and the results
//! populate `repos`, `pull_requests`, and `pull_request_viewer_relations`. The
//! enrichment phase then consumes the union of newly-discovered PRs alongside
//! anything still present in the relations table.
//!
//! Pruning is the cycle's final phase: any relation whose `last_seen_at`
//! predates the cycle start is dropped, so revoked relationships (an unassigned
//! review, a removed mention) disappear within one cycle.

use rusqlite::params;
use serde_json::json;
use thiserror::Error;

use crate::db::DbHandle;
use crate::github::graphql::{DiscoveryData, DiscoveryNode, DiscoveryPullRequest, DISCOVERY_QUERY};
use crate::github::{AccountId, GitHubClient, GitHubError};

/// Max search-result pages walked per query. 4 pages * 50 nodes = 200 results
/// per relation flag, matching the contract in `docs/contracts/dashboard-data.md`.
/// The 50-per-page count is hard-coded in the `DISCOVERY_QUERY` string itself.
pub const MAX_DISCOVERY_PAGES: usize = 4;

/// Errors surfaced by the discovery phase. Split so the worker can route
/// `GitHub` variants by the underlying HTTP status (Unauthorized / RateLimited)
/// while local persistence failures fall through to a generic `Failed` outcome.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error(transparent)]
    GitHub(#[from] GitHubError),
    #[error("discovery persistence: {0}")]
    Db(#[from] rusqlite::Error),
}

/// Which relation a discovery query populates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryRelation {
    Authored,
    ReviewRequested,
    Involves,
}

impl DiscoveryRelation {
    /// Search-API qualifier for this relation. `@me` resolves on the server.
    pub fn query_string(self) -> &'static str {
        match self {
            Self::Authored => "is:pr is:open author:@me sort:updated",
            Self::ReviewRequested => "is:pr is:open review-requested:@me sort:updated",
            Self::Involves => "is:pr is:open involves:@me sort:updated",
        }
    }
}

/// Outcome summary for one discovery phase. Useful for tests and debug logging.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiscoveryReport {
    /// Total raw PR nodes returned across all three queries (before dedup).
    pub nodes_seen: usize,
    /// Distinct `pull_requests.id` values upserted this phase.
    pub distinct_prs: usize,
    /// Number of pages fetched (one per `endCursor` walk step).
    pub pages_fetched: usize,
    /// Number of queries that hit the 200-result cap and stopped early.
    pub truncated_queries: usize,
}

/// Identifies a PR row written by discovery. The caller folds these into the
/// enrichment list for the same cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPr {
    pub pull_request_id: i64,
    pub repo_id: i64,
    pub number: i64,
    pub owner: String,
    pub repo_name: String,
}

/// Mutable state threaded through every discovery query in one cycle.
/// Splitting it out keeps `run_relation_query` to a sane signature while still
/// allowing the three queries to dedupe + accumulate against shared state.
struct DiscoveryAccumulator<'a> {
    db: &'a DbHandle,
    account_id: AccountId,
    cycle_start: i64,
    report: DiscoveryReport,
    discovered: Vec<DiscoveredPr>,
    seen_ids: std::collections::HashSet<i64>,
}

impl<'a> DiscoveryAccumulator<'a> {
    fn new(db: &'a DbHandle, account_id: AccountId, cycle_start: i64) -> Self {
        Self {
            db,
            account_id,
            cycle_start,
            report: DiscoveryReport::default(),
            discovered: Vec::new(),
            seen_ids: std::collections::HashSet::new(),
        }
    }

    fn finish(mut self) -> (Vec<DiscoveredPr>, DiscoveryReport) {
        self.report.distinct_prs = self.seen_ids.len();
        (self.discovered, self.report)
    }

    fn ingest_node(
        &mut self,
        relation: DiscoveryRelation,
        pr: &DiscoveryPullRequest,
    ) -> Result<(), DiscoveryError> {
        self.report.nodes_seen += 1;
        let repo_id = upsert_repo(self.db, self.account_id, pr)?;
        let pr_id = upsert_pull_request(self.db, repo_id, pr)?;
        upsert_viewer_relation(self.db, self.account_id, pr_id, relation, self.cycle_start)?;

        if self.seen_ids.insert(pr_id) {
            self.discovered.push(DiscoveredPr {
                pull_request_id: pr_id,
                repo_id,
                number: pr.number,
                owner: pr.repository.owner.login.clone(),
                repo_name: pr.repository.name.clone(),
            });
        }
        Ok(())
    }
}

/// Run all three discovery queries for one account and persist the results.
///
/// `cycle_start` is the unix-seconds timestamp the cycle began; it's written
/// as `last_seen_at` on every confirmed relation row so the pruning phase can
/// identify stale entries with a single `<` comparison.
pub async fn discover_account(
    db: &DbHandle,
    client: &GitHubClient,
    account_id: AccountId,
    cycle_start: i64,
) -> Result<(Vec<DiscoveredPr>, DiscoveryReport), DiscoveryError> {
    let mut acc = DiscoveryAccumulator::new(db, account_id, cycle_start);

    for relation in [
        DiscoveryRelation::Authored,
        DiscoveryRelation::ReviewRequested,
        DiscoveryRelation::Involves,
    ] {
        run_relation_query(client, relation, &mut acc).await?;
    }

    Ok(acc.finish())
}

async fn run_relation_query(
    client: &GitHubClient,
    relation: DiscoveryRelation,
    acc: &mut DiscoveryAccumulator<'_>,
) -> Result<(), DiscoveryError> {
    let mut cursor: Option<String> = None;
    let query_string = relation.query_string();

    for page in 0..MAX_DISCOVERY_PAGES {
        let vars = json!({ "q": query_string, "after": cursor });
        let data: DiscoveryData = client.graphql(DISCOVERY_QUERY, vars).await?;

        acc.report.pages_fetched += 1;

        for node in &data.search.nodes {
            let DiscoveryNode::PullRequest(pr) = node else {
                continue;
            };
            acc.ingest_node(relation, pr.as_ref())?;
        }

        if !data.search.page_info.has_next_page {
            return Ok(());
        }
        cursor = data.search.page_info.end_cursor;
        if cursor.is_none() {
            return Ok(());
        }
        if page + 1 == MAX_DISCOVERY_PAGES {
            // Hit the 200-result cap. Logged via the report so the caller can
            // surface a warning if it wants; the next cycle picks up the rest.
            acc.report.truncated_queries += 1;
        }
    }
    Ok(())
}

fn upsert_repo(
    db: &DbHandle,
    account_id: AccountId,
    pr: &DiscoveryPullRequest,
) -> Result<i64, rusqlite::Error> {
    let conn = db.lock().expect("db poisoned");
    let owner = pr.repository.owner.login.as_str();
    let name = pr.repository.name.as_str();
    let visibility = pr.repository.visibility();

    conn.execute(
        "INSERT INTO repos (id, account_id, owner, name, visibility)
            VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
            owner = excluded.owner,
            name = excluded.name,
            visibility = excluded.visibility",
        params![
            pr.repository.database_id,
            account_id as i64,
            owner,
            name,
            visibility,
        ],
    )?;

    Ok(pr.repository.database_id)
}

fn upsert_pull_request(
    db: &DbHandle,
    repo_id: i64,
    pr: &DiscoveryPullRequest,
) -> Result<i64, rusqlite::Error> {
    let conn = db.lock().expect("db poisoned");
    let state = pr.state.to_lowercase();
    let author = pr.author.as_ref().map(|a| a.login.as_str()).unwrap_or("");
    let created = rfc3339_to_unix(&pr.created_at).unwrap_or(0);
    let updated = rfc3339_to_unix(&pr.updated_at).unwrap_or(0);

    // Conflict on `id` (the search-derived databaseId) so a PR that exists in
    // multiple discovery queries upserts cleanly; the unique `(repo_id, number)`
    // constraint guards against a row collision on re-seed.
    conn.execute(
        "INSERT INTO pull_requests
            (id, repo_id, number, title, state, draft, author_login,
             created_at, updated_at, base_ref, head_ref)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(id) DO UPDATE SET
            title = excluded.title,
            state = excluded.state,
            draft = excluded.draft,
            author_login = excluded.author_login,
            updated_at = excluded.updated_at,
            base_ref = excluded.base_ref,
            head_ref = excluded.head_ref",
        params![
            pr.database_id,
            repo_id,
            pr.number,
            pr.title,
            state,
            pr.is_draft as i64,
            author,
            created,
            updated,
            pr.base_ref_name,
            pr.head_ref_name,
        ],
    )?;

    Ok(pr.database_id)
}

fn upsert_viewer_relation(
    db: &DbHandle,
    account_id: AccountId,
    pr_id: i64,
    relation: DiscoveryRelation,
    cycle_start: i64,
) -> Result<(), rusqlite::Error> {
    let conn = db.lock().expect("db poisoned");
    let (is_authored, is_review_requested, is_involved) = match relation {
        DiscoveryRelation::Authored => (1i64, 0i64, 0i64),
        DiscoveryRelation::ReviewRequested => (0, 1, 0),
        DiscoveryRelation::Involves => (0, 0, 1),
    };

    // OR-combine the flag bits on conflict so a PR that shows up in multiple
    // queries (e.g. authored + involves) ends with all matching flags set.
    conn.execute(
        "INSERT INTO pull_request_viewer_relations
            (account_id, pull_request_id, is_authored, is_review_requested,
             is_involved, last_seen_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(account_id, pull_request_id) DO UPDATE SET
            is_authored = is_authored | excluded.is_authored,
            is_review_requested = is_review_requested | excluded.is_review_requested,
            is_involved = is_involved | excluded.is_involved,
            last_seen_at = excluded.last_seen_at",
        params![
            account_id as i64,
            pr_id,
            is_authored,
            is_review_requested,
            is_involved,
            cycle_start,
        ],
    )?;

    Ok(())
}

/// Drop stale relations for one account whose `last_seen_at` predates
/// `cycle_start`. Returns the number of rows removed.
///
/// Per-account scoping matters because each account's cycle runs on its own
/// schedule; pruning globally on a single account's `cycle_start` would
/// nuke another account's still-valid rows.
pub fn prune_stale_relations_for_account(
    db: &DbHandle,
    account_id: AccountId,
    cycle_start: i64,
) -> Result<usize, rusqlite::Error> {
    let conn = db.lock().expect("db poisoned");
    let removed = conn.execute(
        "DELETE FROM pull_request_viewer_relations
            WHERE account_id = ?1 AND last_seen_at < ?2",
        params![account_id as i64, cycle_start],
    )?;
    Ok(removed)
}

fn rfc3339_to_unix(s: &str) -> Option<i64> {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    OffsetDateTime::parse(s, &Rfc3339)
        .ok()
        .map(|t| t.unix_timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use rusqlite::params;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let db = open_at(&dir.path().join("prism.sqlite")).unwrap();
        // Seed an account so foreign keys hold.
        db.lock()
            .unwrap()
            .execute(
                "INSERT INTO accounts (id, label, host, login, scopes, created_at)
                    VALUES (1, 'a', 'github.com', 'alice', '', 0)",
                [],
            )
            .unwrap();
        (dir, db)
    }

    fn make_pr(database_id: i64, number: i64, repo_id: i64, owner: &str) -> DiscoveryPullRequest {
        use crate::github::graphql::{Actor, DiscoveryRepository};
        DiscoveryPullRequest {
            id: format!("PR_{database_id}"),
            database_id,
            number,
            title: format!("Title {number}"),
            url: format!("https://github.com/{owner}/repo/pull/{number}"),
            state: "OPEN".into(),
            is_draft: false,
            created_at: "2026-05-18T10:00:00Z".into(),
            updated_at: "2026-05-19T10:00:00Z".into(),
            author: Some(Actor {
                login: owner.into(),
            }),
            base_ref_name: "main".into(),
            head_ref_name: "feat/x".into(),
            repository: DiscoveryRepository {
                database_id: repo_id,
                owner: Actor {
                    login: owner.into(),
                },
                name: "repo".into(),
                is_private: false,
            },
        }
    }

    #[test]
    fn discovery_relation_query_string_uses_at_me_qualifier() {
        assert!(DiscoveryRelation::Authored
            .query_string()
            .contains("author:@me"));
        assert!(DiscoveryRelation::ReviewRequested
            .query_string()
            .contains("review-requested:@me"));
        assert!(DiscoveryRelation::Involves
            .query_string()
            .contains("involves:@me"));
    }

    #[test]
    fn upsert_repo_inserts_then_updates_in_place() {
        let (_dir, db) = fresh_db();
        let pr = make_pr(101, 1, 50, "owner");

        let id1 = upsert_repo(&db, 1, &pr).unwrap();
        let id2 = upsert_repo(&db, 1, &pr).unwrap();
        assert_eq!(id1, 50);
        assert_eq!(id2, 50);

        let count: i64 = db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM repos WHERE id = 50", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn upsert_pull_request_writes_minimal_columns() {
        let (_dir, db) = fresh_db();
        let pr = make_pr(200, 42, 60, "owner");
        upsert_repo(&db, 1, &pr).unwrap();
        let pr_id = upsert_pull_request(&db, 60, &pr).unwrap();

        let (title, state, base): (String, String, String) = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT title, state, base_ref FROM pull_requests WHERE id = ?1",
                params![pr_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(title, "Title 42");
        assert_eq!(state, "open");
        assert_eq!(base, "main");
    }

    #[test]
    fn upsert_viewer_relation_or_combines_flags_across_relations() {
        // A single PR appearing in two queries (authored + involves) yields one
        // relation row with both flag bits set, plus the latest `last_seen_at`.
        let (_dir, db) = fresh_db();
        let pr = make_pr(300, 7, 70, "owner");
        upsert_repo(&db, 1, &pr).unwrap();
        let pr_id = upsert_pull_request(&db, 70, &pr).unwrap();

        upsert_viewer_relation(&db, 1, pr_id, DiscoveryRelation::Authored, 1000).unwrap();
        upsert_viewer_relation(&db, 1, pr_id, DiscoveryRelation::Involves, 1000).unwrap();

        let (authored, requested, involved): (i64, i64, i64) = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT is_authored, is_review_requested, is_involved
                   FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = ?1",
                params![pr_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(authored, 1);
        assert_eq!(requested, 0);
        assert_eq!(involved, 1);
    }

    #[test]
    fn upsert_viewer_relation_refreshes_last_seen_at() {
        // Calling the upsert twice with different timestamps lifts `last_seen_at`
        // to the latest value (the pruning phase keys off this column).
        let (_dir, db) = fresh_db();
        let pr = make_pr(400, 1, 80, "owner");
        upsert_repo(&db, 1, &pr).unwrap();
        let pr_id = upsert_pull_request(&db, 80, &pr).unwrap();

        upsert_viewer_relation(&db, 1, pr_id, DiscoveryRelation::Authored, 1000).unwrap();
        upsert_viewer_relation(&db, 1, pr_id, DiscoveryRelation::Authored, 2000).unwrap();

        let last_seen: i64 = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT last_seen_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = ?1",
                params![pr_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(last_seen, 2000);
    }

    #[test]
    fn prune_drops_rows_below_cycle_start_and_keeps_fresh_rows() {
        let (_dir, db) = fresh_db();
        let pr = make_pr(500, 1, 90, "owner");
        upsert_repo(&db, 1, &pr).unwrap();
        let pr_id = upsert_pull_request(&db, 90, &pr).unwrap();

        upsert_viewer_relation(&db, 1, pr_id, DiscoveryRelation::Authored, 100).unwrap();
        // A second PR with a fresh timestamp must survive the prune.
        let pr2 = make_pr(501, 2, 90, "owner");
        upsert_pull_request(&db, 90, &pr2).unwrap();
        upsert_viewer_relation(&db, 1, 501, DiscoveryRelation::Involves, 500).unwrap();

        let removed = prune_stale_relations_for_account(&db, 1, 200).unwrap();
        assert_eq!(removed, 1);

        let survivors: i64 = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM pull_request_viewer_relations",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(survivors, 1);
    }

    #[test]
    fn prune_for_account_leaves_other_accounts_untouched() {
        // Account 1's stale row is pruned; account 2's identical-timestamp row
        // survives because the scoped prune ignored it.
        let (_dir, db) = fresh_db();
        db.lock()
            .unwrap()
            .execute(
                "INSERT INTO accounts (id, label, host, login, scopes, created_at)
                    VALUES (2, 'b', 'github.com', 'bob', '', 0)",
                [],
            )
            .unwrap();
        let pr = make_pr(600, 1, 100, "owner");
        upsert_repo(&db, 1, &pr).unwrap();
        let pr_id = upsert_pull_request(&db, 100, &pr).unwrap();

        upsert_viewer_relation(&db, 1, pr_id, DiscoveryRelation::Authored, 100).unwrap();
        upsert_viewer_relation(&db, 2, pr_id, DiscoveryRelation::Involves, 100).unwrap();

        let removed = prune_stale_relations_for_account(&db, 1, 200).unwrap();
        assert_eq!(removed, 1);

        let survivors: i64 = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM pull_request_viewer_relations WHERE account_id = 2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(survivors, 1);
    }
}
