//! Tauri command surface for the conversation module.
//!
//! See `docs/contracts/conversation-depth.md` (sections "Tauri command surface",
//! "Conversation stats math", "Implementation notes") for the shape these
//! commands implement.

use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use tauri::State;

use crate::auth::store::{Account, AccountStore};
use crate::conversation::query;
use crate::conversation::types::{
    ConversationStats, HydratedConversation, PullRequestThread, TimelineEventRecord,
};
use crate::db::DbHandle;
use crate::github::graphql::{
    IssueCommentNode, PrCommentsData, PullRequestComments, ReviewCommentNode, ReviewThreadComments,
    PR_COMMENTS_QUERY,
};
use crate::github::GitHubClient;
use crate::sync::ClientFactory;

/// Shared handle to the production [`ClientFactory`]. Mounted via
/// `tauri::Builder::manage` so the conversation hydrator can build a
/// per-account GitHub client without going through the sync worker.
pub type ClientFactoryHandle = Arc<dyn ClientFactory>;

/// Shared handle to the [`AccountStore`]. Same pattern as
/// [`ClientFactoryHandle`].
pub type AccountStoreHandle = Arc<dyn AccountStore>;

/// Caps on per-thread / per-PR pagination, per the contract.
const MAX_THREAD_COMMENTS: usize = 200;
const MAX_ISSUE_COMMENTS: usize = 200;

/// List per-thread state for a PR. Reads from the local cache only; no network
/// round-trip. The optional `account_id` resolves the `is_involved` marker.
#[tauri::command]
pub fn list_pr_threads(
    pull_request_id: i64,
    account_id: Option<i64>,
    db: State<'_, DbHandle>,
) -> Result<Vec<PullRequestThread>, String> {
    let conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    query::list_pr_threads(&conn, pull_request_id, account_id)
        .map_err(|e| format!("list_pr_threads: {e}"))
}

/// Compute conversation stats for a PR from the local cache.
#[tauri::command]
pub fn get_pr_conversation_stats(
    pull_request_id: i64,
    db: State<'_, DbHandle>,
) -> Result<ConversationStats, String> {
    let conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    query::get_conversation_stats(&conn, pull_request_id)
        .map_err(|e| format!("get_pr_conversation_stats: {e}"))
}

/// List the persisted timeline events for a PR. Reads from the local cache
/// only; no network round-trip. The events are populated by the sync worker
/// each cycle (wipe-and-rewrite) so the list always reflects the latest
/// upstream history at the granularity of the qualifying-event set defined in
/// ADR 0007.
#[tauri::command]
pub fn list_pr_timeline_events(
    pull_request_id: i64,
    db: State<'_, DbHandle>,
) -> Result<Vec<TimelineEventRecord>, String> {
    let conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    query::list_pr_timeline_events(&conn, pull_request_id)
        .map_err(|e| format!("list_pr_timeline_events: {e}"))
}

/// Lazy hydration: fetch full thread replies + issue-comment bodies from
/// GitHub, persist them, return the hydrated DTO.
///
/// All persistence happens in a single transaction so a half-fetched state
/// never leaks. The upserts are keyed on `node_id`, so repeated calls are
/// idempotent. The frontend conversation store caches the hydrated DTO
/// in-memory across re-mounts; the backend itself stays stateless.
#[tauri::command]
pub async fn fetch_pr_conversation(
    pull_request_id: i64,
    db: State<'_, DbHandle>,
    clients: State<'_, ClientFactoryHandle>,
    accounts: State<'_, AccountStoreHandle>,
) -> Result<HydratedConversation, String> {
    let (account, repo_coord) = resolve_pr_context(&db, &accounts, pull_request_id)?;
    let client = clients
        .build(&account)
        .map_err(|e| format!("build client: {e}"))?;

    let payload = fetch_comments_payload(&client, &repo_coord).await?;
    persist_payload(&db, pull_request_id, &payload)?;
    auto_mark_read(&db, pull_request_id, account.id as i64);
    hydrated_response(&db, pull_request_id, account.id as i64)
}

/// Best-effort auto-mark-on-open. Drives the same write path as
/// `triage::commands::mark_pr_read` but runs after the hydration
/// transaction commits so a failure can't unwind the cached payload.
/// Errors are logged and swallowed: a mark-read failure must never break
/// detail-surface hydration.
fn auto_mark_read(db: &DbHandle, pull_request_id: i64, account_id: i64) {
    if let Err(e) = mark_read_in_tx(db, pull_request_id, account_id) {
        eprintln!("auto-mark-on-open failed (pr={pull_request_id}, account={account_id}): {e}");
    }
}

fn mark_read_in_tx(db: &DbHandle, pull_request_id: i64, account_id: i64) -> Result<(), String> {
    let mut conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    let tx = conn.transaction().map_err(|e| format!("begin tx: {e}"))?;
    crate::triage::query::mark_read(&tx, account_id, pull_request_id)
        .map_err(|e| format!("mark read: {e}"))?;
    crate::triage::query::recompute_needs_attention(&tx, account_id, pull_request_id)
        .map_err(|e| format!("recompute needs_attention: {e}"))?;
    tx.commit().map_err(|e| format!("commit tx: {e}"))?;
    Ok(())
}

/// Resolves the `(account, repo coordinates)` pair needed to build a client and
/// run `PR_COMMENTS_QUERY` for a given local `pull_request_id`.
fn resolve_pr_context(
    db: &DbHandle,
    accounts: &AccountStoreHandle,
    pull_request_id: i64,
) -> Result<(Account, RepoCoord), String> {
    let conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    let coord: Option<(i64, String, String, i64)> = conn
        .query_row(
            "SELECT r.account_id, r.owner, r.name, pr.number
               FROM pull_requests pr
               JOIN repos r ON r.id = pr.repo_id
              WHERE pr.id = ?1",
            params![pull_request_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("resolve pr: {e}"))?;
    drop(conn);

    let (account_id, owner, name, number) =
        coord.ok_or_else(|| "pull request not found".to_string())?;

    let accounts_list = accounts.list().map_err(|e| format!("list accounts: {e}"))?;
    let account = accounts_list
        .into_iter()
        .find(|a| a.id as i64 == account_id)
        .ok_or_else(|| "account not found".to_string())?;

    Ok((
        account,
        RepoCoord {
            owner,
            name,
            number,
        },
    ))
}

#[derive(Debug, Clone)]
struct RepoCoord {
    owner: String,
    name: String,
    number: i64,
}

/// Payload accumulated across paginated `PR_COMMENTS_QUERY` calls. Comment caps
/// (200 per thread / 200 issue comments per PR) are enforced inline.
#[derive(Debug, Default)]
struct CommentsPayload {
    threads: Vec<ReviewThreadComments>,
    issue_comments: Vec<IssueCommentNode>,
}

async fn fetch_comments_payload(
    client: &GitHubClient,
    coord: &RepoCoord,
) -> Result<CommentsPayload, String> {
    let mut payload = CommentsPayload::default();
    let mut threads_after: Option<String> = None;
    let mut issue_after: Option<String> = None;
    // Belt-and-braces: bound the paginator independently of the per-page caps
    // so a malformed cursor loop can't run forever. With 100 threads + 100
    // issue comments per page and the 200-row caps, the worst case is two
    // pages on each side; eight is more than enough headroom.
    const MAX_PAGES: usize = 8;
    let mut threads_done = false;
    let mut issues_done = false;
    for _ in 0..MAX_PAGES {
        let vars = serde_json::json!({
            "owner": coord.owner,
            "name": coord.name,
            "number": coord.number,
            "threadsAfter": threads_after,
            "issueCommentsAfter": issue_after,
        });
        let data: PrCommentsData = client
            .post_graphql(PR_COMMENTS_QUERY, vars)
            .await
            .map_err(|e| format!("pr comments fetch: {e}"))?;
        let Some(pr) = data.repository.and_then(|r| r.pull_request) else {
            return Err("pull request not found upstream".into());
        };

        let PullRequestComments {
            review_threads,
            issue_comments,
        } = pr;

        if !threads_done {
            merge_threads(&mut payload.threads, review_threads.nodes);
            if review_threads.page_info.has_next_page {
                threads_after = review_threads.page_info.end_cursor;
            } else {
                threads_done = true;
            }
        }

        if !issues_done {
            if payload.issue_comments.len() < MAX_ISSUE_COMMENTS {
                let remaining = MAX_ISSUE_COMMENTS - payload.issue_comments.len();
                payload
                    .issue_comments
                    .extend(issue_comments.nodes.into_iter().take(remaining));
            }
            if issue_comments.page_info.has_next_page
                && payload.issue_comments.len() < MAX_ISSUE_COMMENTS
            {
                issue_after = issue_comments.page_info.end_cursor;
            } else {
                issues_done = true;
            }
        }

        if threads_done && issues_done {
            break;
        }
    }
    Ok(payload)
}

/// Merge a freshly-paginated thread set into the running payload. Threads we
/// haven't seen are appended; threads we already have absorb the extra comment
/// page (subject to the per-thread cap).
fn merge_threads(running: &mut Vec<ReviewThreadComments>, fresh: Vec<ReviewThreadComments>) {
    for thread in fresh {
        match running.iter_mut().find(|t| t.id == thread.id) {
            Some(existing) => {
                let remaining = MAX_THREAD_COMMENTS.saturating_sub(existing.comments.nodes.len());
                if remaining > 0 {
                    existing
                        .comments
                        .nodes
                        .extend(thread.comments.nodes.into_iter().take(remaining));
                }
                existing.comments.page_info = thread.comments.page_info;
            }
            None => {
                let mut bounded = thread;
                if bounded.comments.nodes.len() > MAX_THREAD_COMMENTS {
                    bounded.comments.nodes.truncate(MAX_THREAD_COMMENTS);
                }
                running.push(bounded);
            }
        }
    }
}

/// Persist the comments payload inside a single transaction. On error, the
/// previous cached state is preserved.
fn persist_payload(
    db: &DbHandle,
    pull_request_id: i64,
    payload: &CommentsPayload,
) -> Result<(), String> {
    let mut conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    let tx = conn.transaction().map_err(|e| format!("begin tx: {e}"))?;

    for thread in &payload.threads {
        let Some(thread_id) =
            resolve_thread_id(&tx, &thread.id).map_err(|e| format!("resolve thread: {e}"))?
        else {
            // Thread row hasn't been written by the cycle yet (e.g. the user
            // opened the drawer before the first sync cycle landed thread
            // headers). Skip its comments rather than orphan-insert.
            continue;
        };
        for comment in &thread.comments.nodes {
            upsert_review_comment(&tx, thread_id, comment)
                .map_err(|e| format!("upsert review comment: {e}"))?;
        }
    }

    for comment in &payload.issue_comments {
        upsert_issue_comment(&tx, pull_request_id, comment)
            .map_err(|e| format!("upsert issue comment: {e}"))?;
    }

    tx.commit().map_err(|e| format!("commit tx: {e}"))?;
    Ok(())
}

fn resolve_thread_id(
    tx: &rusqlite::Transaction<'_>,
    node_id: &str,
) -> Result<Option<i64>, rusqlite::Error> {
    tx.query_row(
        "SELECT id FROM review_threads WHERE node_id = ?1",
        params![node_id],
        |row| row.get::<_, i64>(0),
    )
    .optional()
}

fn upsert_review_comment(
    tx: &rusqlite::Transaction<'_>,
    thread_id: i64,
    comment: &ReviewCommentNode,
) -> Result<(), rusqlite::Error> {
    let author = comment
        .author
        .as_ref()
        .map(|a| a.login.as_str())
        .unwrap_or("");
    let created_at = parse_rfc3339(&comment.created_at).unwrap_or(0);
    if let Some(actor) = comment.author.as_ref() {
        upsert_user_avatar(tx, &actor.login, actor.avatar_url.as_deref(), created_at)?;
    }
    // The unique constraint on `node_id` is a partial index
    // (`WHERE node_id IS NOT NULL`) so the conflict target needs the matching
    // predicate. The hydrator always writes a non-null `node_id`.
    // `COALESCE(excluded.url, ...)` keeps a previously-persisted url if a
    // later payload happens to omit it (defensive parity with the worker's
    // thread-level url preservation). Same protection applies to
    // `body_html` (ADR 0014, issue #138).
    tx.execute(
        "INSERT INTO review_comments
            (review_thread_id, author_login, body, created_at, node_id,
             database_id, line, side, url, body_html)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(node_id) WHERE node_id IS NOT NULL DO UPDATE SET
            review_thread_id = excluded.review_thread_id,
            author_login     = excluded.author_login,
            body             = excluded.body,
            created_at       = excluded.created_at,
            database_id      = excluded.database_id,
            line             = excluded.line,
            side             = excluded.side,
            url              = COALESCE(excluded.url, review_comments.url),
            body_html        = COALESCE(excluded.body_html, review_comments.body_html)",
        params![
            thread_id,
            author,
            comment.body,
            created_at,
            comment.id,
            comment.database_id,
            comment.line,
            comment.side,
            comment.url,
            comment.body_html,
        ],
    )?;
    Ok(())
}

fn upsert_issue_comment(
    tx: &rusqlite::Transaction<'_>,
    pull_request_id: i64,
    comment: &IssueCommentNode,
) -> Result<(), rusqlite::Error> {
    let author = comment
        .author
        .as_ref()
        .map(|a| a.login.as_str())
        .unwrap_or("");
    let created_at = parse_rfc3339(&comment.created_at).unwrap_or(0);
    if let Some(actor) = comment.author.as_ref() {
        upsert_user_avatar(tx, &actor.login, actor.avatar_url.as_deref(), created_at)?;
    }
    tx.execute(
        "INSERT INTO issue_comments
            (pull_request_id, author_login, body, created_at, node_id,
             database_id, url, body_html)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(node_id) WHERE node_id IS NOT NULL DO UPDATE SET
            pull_request_id = excluded.pull_request_id,
            author_login    = excluded.author_login,
            body            = excluded.body,
            created_at      = excluded.created_at,
            database_id     = excluded.database_id,
            url             = COALESCE(excluded.url, issue_comments.url),
            body_html       = COALESCE(excluded.body_html, issue_comments.body_html)",
        params![
            pull_request_id,
            author,
            comment.body,
            created_at,
            comment.id,
            comment.database_id,
            comment.url,
            comment.body_html,
        ],
    )?;
    Ok(())
}

/// Mirror of the worker's user-cache upsert (ADR 0013) used by the lazy
/// hydrator. The hydrator processes payload comments outside the sync cycle,
/// so populating `users` from here keeps every fresh avatar URL the lazy
/// fetch surfaced (e.g. a reviewer who first commented after the last sync).
/// No-op when `avatar_url` is `None` or empty — we never blank an existing
/// row with a NULL on a partial payload.
fn upsert_user_avatar(
    tx: &rusqlite::Transaction<'_>,
    login: &str,
    avatar_url: Option<&str>,
    last_seen_at: i64,
) -> Result<(), rusqlite::Error> {
    let Some(url) = avatar_url else {
        return Ok(());
    };
    if login.is_empty() || url.is_empty() {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO users (login, avatar_url, last_seen_at)
            VALUES (?1, ?2, ?3)
         ON CONFLICT(login) DO UPDATE SET
            avatar_url = excluded.avatar_url,
            last_seen_at = excluded.last_seen_at",
        params![login, url, last_seen_at],
    )?;
    Ok(())
}

fn parse_rfc3339(s: &str) -> Option<i64> {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    OffsetDateTime::parse(s, &Rfc3339)
        .ok()
        .map(|t| t.unix_timestamp())
}

fn hydrated_response(
    db: &DbHandle,
    pull_request_id: i64,
    account_id: i64,
) -> Result<HydratedConversation, String> {
    let conn = db.lock().map_err(|e| format!("db poisoned: {e}"))?;
    build_hydrated(&conn, pull_request_id, Some(account_id))
        .map_err(|e| format!("hydrate response: {e}"))
}

/// Read the persisted state for a PR back into a `HydratedConversation`. Pulled
/// out so the hydrator and any future cache-only reader share the same shape.
fn build_hydrated(
    conn: &Connection,
    pull_request_id: i64,
    account_id: Option<i64>,
) -> Result<HydratedConversation, rusqlite::Error> {
    let threads = query::list_pr_threads(conn, pull_request_id, account_id)?;
    let thread_comments = query::list_thread_comments(conn, pull_request_id)?;
    let issue_comments = query::list_issue_comments(conn, pull_request_id)?;
    let reviews = query::list_reviews(conn, pull_request_id)?;
    let stats = query::get_conversation_stats(conn, pull_request_id)?;
    Ok(HydratedConversation {
        pull_request_id,
        threads,
        thread_comments,
        issue_comments,
        reviews,
        stats,
    })
}

/// Test-only helpers. Exposed to integration tests under `tests/` so they can
/// drive the hydrator's internal machinery (persistence path, network fetch)
/// without booting Tauri state. Not part of the supported public API.
#[doc(hidden)]
pub mod testing {
    use super::*;

    /// Replay a fully-resolved comments payload through the same persistence
    /// path the live hydrator uses.
    pub fn persist(
        db: &DbHandle,
        pull_request_id: i64,
        threads: Vec<ReviewThreadComments>,
        issue_comments: Vec<IssueCommentNode>,
    ) -> Result<(), String> {
        persist_payload(
            db,
            pull_request_id,
            &CommentsPayload {
                threads,
                issue_comments,
            },
        )
    }

    /// Rebuild a `HydratedConversation` from a connection, matching what the
    /// live command returns after persistence.
    pub fn build_hydrated(
        conn: &Connection,
        pull_request_id: i64,
        account_id: Option<i64>,
    ) -> Result<HydratedConversation, String> {
        super::build_hydrated(conn, pull_request_id, account_id).map_err(|e| e.to_string())
    }

    /// Drive the full hydrator path (network round-trip + persistence +
    /// auto-mark-on-open + hydrated read) against a pre-built `GitHubClient`.
    pub async fn fetch(
        db: &DbHandle,
        client: &GitHubClient,
        pull_request_id: i64,
        owner: &str,
        name: &str,
        number: i64,
        account_id: i64,
    ) -> Result<HydratedConversation, String> {
        let coord = RepoCoord {
            owner: owner.into(),
            name: name.into(),
            number,
        };
        let payload = fetch_comments_payload(client, &coord).await?;
        persist_payload(db, pull_request_id, &payload)?;
        super::auto_mark_read(db, pull_request_id, account_id);
        hydrated_response(db, pull_request_id, account_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::graphql::{Actor, PageInfo, ReviewCommentConnection};

    fn page_info(has_next: bool, cursor: Option<&str>) -> PageInfo {
        PageInfo {
            has_next_page: has_next,
            end_cursor: cursor.map(str::to_string),
        }
    }

    fn comment(id: &str, db: i64, body: &str, login: &str) -> ReviewCommentNode {
        ReviewCommentNode {
            id: id.into(),
            url: None,
            database_id: Some(db),
            author: Some(Actor::new(login)),
            body: body.into(),
            body_html: None,
            body_text: body.into(),
            created_at: "2026-05-19T10:00:00Z".into(),
            path: None,
            line: None,
            original_line: None,
            side: None,
        }
    }

    fn thread(id: &str, comments: Vec<ReviewCommentNode>) -> ReviewThreadComments {
        ReviewThreadComments {
            id: id.into(),
            comments: ReviewCommentConnection {
                page_info: page_info(false, None),
                nodes: comments,
            },
        }
    }

    #[test]
    fn merge_threads_appends_new_threads() {
        let mut running = vec![thread("PRRT_1", vec![comment("c1", 1, "a", "alice")])];
        merge_threads(
            &mut running,
            vec![thread("PRRT_2", vec![comment("c2", 2, "b", "bob")])],
        );
        assert_eq!(running.len(), 2);
        assert_eq!(running[0].id, "PRRT_1");
        assert_eq!(running[1].id, "PRRT_2");
    }

    #[test]
    fn merge_threads_extends_existing_thread_comments() {
        let mut running = vec![thread("PRRT_1", vec![comment("c1", 1, "a", "alice")])];
        merge_threads(
            &mut running,
            vec![thread("PRRT_1", vec![comment("c2", 2, "b", "bob")])],
        );
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].comments.nodes.len(), 2);
    }

    #[test]
    fn merge_threads_respects_per_thread_cap() {
        let mut running = vec![thread(
            "PRRT_1",
            (0..MAX_THREAD_COMMENTS)
                .map(|i| comment(&format!("c{i}"), i as i64, "x", "alice"))
                .collect(),
        )];
        merge_threads(
            &mut running,
            vec![thread(
                "PRRT_1",
                vec![comment("over", 999, "extra", "alice")],
            )],
        );
        assert_eq!(running[0].comments.nodes.len(), MAX_THREAD_COMMENTS);
        // The extra comment should not have been appended.
        assert!(!running[0].comments.nodes.iter().any(|c| c.id == "over"));
    }

    #[test]
    fn merge_threads_truncates_initial_oversized_thread() {
        let oversized: Vec<ReviewCommentNode> = (0..MAX_THREAD_COMMENTS + 50)
            .map(|i| comment(&format!("c{i}"), i as i64, "x", "alice"))
            .collect();
        let mut running: Vec<ReviewThreadComments> = Vec::new();
        merge_threads(&mut running, vec![thread("PRRT_1", oversized)]);
        assert_eq!(running[0].comments.nodes.len(), MAX_THREAD_COMMENTS);
    }

    #[test]
    fn parse_rfc3339_round_trips_known_value() {
        // 2026-01-01T00:00:00Z = 1767225600
        assert_eq!(parse_rfc3339("2026-01-01T00:00:00Z"), Some(1_767_225_600));
        assert!(parse_rfc3339("nope").is_none());
    }
}
