//! Per-PR enrichment writes: `pr_detail` upserts, conversation-depth tables
//! (`review_threads`, `reviews`, `requested_reviewers`, `issue_comments_count`,
//! `users`), and qualifying timeline events. Each write runs inside a single
//! transaction so partial failures roll back together. Returns the
//! notification triggers produced by the post-write triage recompute.

use rusqlite::params;

use crate::db::DbHandle;
use crate::github::AccountId;
use crate::notify::NotificationTrigger;

use super::rfc3339_to_unix;
use super::triage_recompute::scan_mentions_and_recompute_attention;

mod reviews;
#[cfg(test)]
mod tests;
mod timeline;
mod users;

use reviews::{write_issue_comments, write_review_threads, write_reviews};
use timeline::{qualifying_event_wire_name, write_timeline_events};
use users::write_user_avatars;

/// Apply the freshly-fetched PR detail and timeline events to the local cache.
///
/// Only fields exposed by the v2 schema are updated; everything else is
/// untouched. The status-change derivation (ADR 0007) runs here so the
/// `latest_status_change_*` columns reflect the most recent timeline pull.
/// Requested reviewers are replaced wholesale (delete-then-insert) whenever
/// the detail response carries them so the cached set never drifts past the
/// upstream truth.
///
/// `account_id` drives the per-account involvement bucket split: the cycle
/// runs per-account, so each cycle naturally writes the correct value for the
/// active viewer. Multi-account users see the count for the most recently
/// synced account (ADR 0010 negative consequences; M5 revisits).
pub fn write_pr_updates(
    db: &DbHandle,
    account_id: AccountId,
    repo_id: i64,
    pr_id: i64,
    detail: Option<&crate::github::graphql::PullRequestDetail>,
    events: Option<&[crate::sync::status_timeline::TimelineEvent]>,
) -> Result<Vec<NotificationTrigger>, rusqlite::Error> {
    let mut conn = crate::db::lock_db(db)?;
    let tx = conn.transaction()?;

    if let Some(d) = detail {
        // GitHub's GraphQL returns `state` as upper-cased enum ("OPEN" /
        // "CLOSED"); the dashboard query and the auto-archive sweep both
        // filter on lowercase values (matching how `discovery.rs` writes the
        // initial row). Normalising here keeps every persisted row in the
        // canonical lowercase shape - without this, the enrichment overwrite
        // would flip every freshly-fetched PR out of the open-only default
        // views as the cycle progressed.
        let state = if d.merged {
            "merged".to_string()
        } else {
            d.state.to_lowercase()
        };
        let author = d.author.as_ref().map(|a| a.login.as_str()).unwrap_or("");
        let ci = compute_ci_rollup(d);
        tx.execute(
            "INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref,
                 mergeable, review_decision, additions, deletions, changed_files,
                 ci_state, ci_total, ci_passing)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                        ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                state = excluded.state,
                is_draft = excluded.is_draft,
                author_login = excluded.author_login,
                updated_at = excluded.updated_at,
                base_ref = excluded.base_ref,
                head_ref = excluded.head_ref,
                mergeable = excluded.mergeable,
                review_decision = excluded.review_decision,
                additions = excluded.additions,
                deletions = excluded.deletions,
                changed_files = excluded.changed_files,
                ci_state = excluded.ci_state,
                ci_total = excluded.ci_total,
                ci_passing = excluded.ci_passing",
            params![
                pr_id,
                repo_id,
                d.number,
                d.title,
                state,
                d.is_draft as i64,
                author,
                rfc3339_to_unix(&d.created_at).unwrap_or(0),
                rfc3339_to_unix(&d.updated_at).unwrap_or(0),
                d.base_ref_name,
                d.head_ref_name,
                d.mergeable,
                d.review_decision,
                d.additions,
                d.deletions,
                d.changed_files,
                ci.state,
                ci.total,
                ci.passing,
            ],
        )?;

        if let Some(rr) = d.review_requests.as_ref() {
            tx.execute(
                "DELETE FROM requested_reviewers WHERE pull_request_id = ?1",
                params![pr_id],
            )?;
            for entry in &rr.nodes {
                let Some((reviewer_type, login)) = reviewer_type_and_login(entry) else {
                    continue;
                };
                tx.execute(
                    "INSERT OR IGNORE INTO requested_reviewers
                        (pull_request_id, login, reviewer_type)
                        VALUES (?1, ?2, ?3)",
                    params![pr_id, login, reviewer_type],
                )?;
            }
        }

        write_review_threads(&tx, pr_id, &d.review_threads.nodes)?;

        if let Some(reviews) = d.reviews.as_ref() {
            write_reviews(&tx, pr_id, &reviews.nodes)?;
        }

        if let Some(ic) = d.issue_comments.as_ref() {
            tx.execute(
                "UPDATE pull_requests SET issue_comments_count = ?1 WHERE id = ?2",
                params![ic.total_count, pr_id],
            )?;
            // ADR 0029: sync owns `issue_comments` persistence so the mention
            // scan + the conversation drawer see fresh bodies on every cycle.
            write_issue_comments(&tx, pr_id, &ic.nodes)?;
        }
    }

    if let Some(events) = events {
        if let Some(change) = crate::sync::status_timeline::latest_status_change(events) {
            let event_name = qualifying_event_wire_name(change.event_type);
            let at_secs = change.at.unix_timestamp();
            tx.execute(
                "UPDATE pull_requests
                    SET latest_status_change_at = ?1,
                        latest_status_change_event_type = ?2
                  WHERE id = ?3",
                params![at_secs, event_name, pr_id],
            )?;
        }
        write_timeline_events(&tx, pr_id, events)?;
    }

    // Users cache (ADR 0013 — avatar caching). Walks every (login, avatar_url)
    // pair the detail + events payload surfaced and UPSERTs them into `users`.
    // The dashboard / conversation read queries `LEFT JOIN users` to surface
    // the URL; entries without an avatar URL are skipped so we never overwrite
    // a populated row with a null on a partial payload.
    write_user_avatars(&tx, detail, events)?;

    // ADR 0016 retired the per-cycle threads rollup UPDATE that lived here.
    // The four `pull_requests.threads_*` columns are no longer written or
    // read; the dashboard query (`src-tauri/src/dashboard/query.rs`) computes
    // the same buckets at read time scoped to the in-scope account set. The
    // legacy columns stay on the schema (SQLite column-drop is non-trivial);
    // a future `chore` migration removes them. Removing the UPDATE saves one
    // SQL statement per PR per cycle.

    // Triage scan + needs_attention recompute (M4-B, ADR 0015 / issue #146).
    // Runs after every other write in this transaction so the recompute sees
    // the freshest threads (read directly from `review_threads` /
    // `review_comments` per ADR 0016), requested-reviewers set, and
    // review-decision. A missing relation row (PR not discovered for the
    // active account) is a valid no-op: every UPDATE here matches by
    // (account_id, pull_request_id) and the dashboard query LEFT JOINs the
    // relations table.
    //
    // Returns the (possibly empty) notification triggers for the two ADR 0017
    // transitions observed in this cycle. The caller dispatches after commit.
    let triggers = scan_mentions_and_recompute_attention(&tx, account_id, pr_id)?;

    tx.commit()?;
    Ok(triggers)
}

/// Pre-aggregated CI rollup persisted to the `ci_*` columns.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CiRollup {
    state: Option<String>,
    total: Option<i64>,
    passing: Option<i64>,
}

/// Walk `commits.nodes[0].commit.statusCheckRollup` and return the dashboard
/// CI summary. `passing` counts `CheckRun.conclusion == "SUCCESS"` and
/// `StatusContext.state == "SUCCESS"`; a `null` `CheckRun.conclusion` means
/// the run is still in progress (counted in `total` only, never in `passing`).
fn compute_ci_rollup(detail: &crate::github::graphql::PullRequestDetail) -> CiRollup {
    let Some(commit) = detail
        .commits
        .as_ref()
        .and_then(|c| c.nodes.first())
        .map(|n| &n.commit)
    else {
        return CiRollup {
            state: None,
            total: None,
            passing: None,
        };
    };
    let Some(rollup) = commit.status_check_rollup.as_ref() else {
        return CiRollup {
            state: None,
            total: None,
            passing: None,
        };
    };

    use crate::github::graphql::StatusCheckContext;
    let passing = rollup
        .contexts
        .nodes
        .iter()
        .filter(|ctx| match ctx {
            StatusCheckContext::CheckRun { conclusion, .. } => {
                conclusion.as_deref() == Some("SUCCESS")
            }
            StatusCheckContext::StatusContext { state } => state == "SUCCESS",
            StatusCheckContext::Other => false,
        })
        .count() as i64;

    CiRollup {
        state: Some(rollup.state.clone()),
        total: Some(rollup.contexts.total_count),
        passing: Some(passing),
    }
}

/// Map a `ReviewRequest` node to the `(reviewer_type, login)` pair persisted
/// to `requested_reviewers`. Returns `None` when the node has no reviewer
/// (deleted user/team) or the reviewer is neither a `User` nor a `Team`.
fn reviewer_type_and_login(
    request: &crate::github::graphql::ReviewRequest,
) -> Option<(&'static str, &str)> {
    use crate::github::graphql::RequestedReviewer;
    match request.requested_reviewer.as_ref()? {
        RequestedReviewer::User { login, .. } => Some(("user", login.as_str())),
        RequestedReviewer::Team { slug } => Some(("team", slug.as_str())),
        RequestedReviewer::Other => None,
    }
}
