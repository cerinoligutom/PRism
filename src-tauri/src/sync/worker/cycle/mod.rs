//! Per-cycle orchestration: discovery, enrichment fan-out, pruning, sweeps,
//! and rate-budget gating. Drives `sync_repo`'s per-PR loop and reports the
//! resulting [`SyncCycleReport`] back to the account loop in `mod.rs`.
//!
//! Slightly over the 700-line CLAUDE.md budget because the `run_one_cycle`
//! state machine reads cleanest as a single function - splitting its phases
//! out forces parameter threading that obscures the flow.

use std::time::{Duration, SystemTime};

use tokio::time::timeout;

use crate::auth::store::Account;
use crate::github::{
    list_pr_timeline, GitHubClient, GitHubError, ListTimeline, RateResource, RepoCoord,
    ResourceSnapshot,
};
use crate::sync::activity::SyncPhaseLabel;
use crate::sync::discovery::DiscoveryError;
use crate::sync::events::{SyncRateLimitPayload, SYNC_RATE_LIMIT_EVENT};
use crate::sync::scheduler::RATE_BUDGET_GUARD_PCT;
use crate::sync::state::{format_rfc3339, SyncPhase};

use super::dispatch::dispatch_triggers;
use super::enrichment::write_pr_updates;
use super::{
    emit_status, next_sync_hint, record_failure, rfc3339_to_unix, unix_now, CycleOutcome,
    SkipReason, SyncCycleReport, WorkerContext,
};

mod activity;
mod repos;
mod sweeps;
#[cfg(test)]
mod tests;

pub(super) use activity::emit_activity_cycle_failed;
pub use repos::{list_prs_for_repo, list_repos_for_account, PrRow, RepoRow};

use activity::{
    emit_activity_cycle_completed, emit_activity_cycle_started, emit_activity_phase_completed,
    emit_activity_phase_completed_with_skips, emit_activity_phase_progress,
    emit_activity_phase_started, emit_activity_pr_detail_empty, emit_activity_pr_fetched,
    emit_activity_pr_skipped_no_change, emit_activity_rate_pause,
};
use repos::{
    detail_state_appears_empty, pr_detail_marker_bytes, pr_detail_marker_key,
    pr_detail_repair_marker_key,
};
use sweeps::{count_prs_across_repos, run_archive_retention_sweep, run_auto_archive_sweep};

/// Compute the percentage of budget remaining, clamped to 0-100. Returns
/// `None` when the rate-budget hasn't been observed yet (no requests issued).
fn rate_remaining_pct(remaining: i64, limit: i64) -> Option<u8> {
    if limit <= 0 || remaining < 0 {
        return None;
    }
    let pct = (remaining * 100) / limit;
    Some(pct.clamp(0, 100) as u8)
}

/// Per-resource budget snapshot percentage. Mirrors [`rate_remaining_pct`]
/// against a [`ResourceSnapshot`] so call sites can pick the right sub-bucket
/// (search for discovery, graphql for enrichment, core for timeline).
fn resource_remaining_pct(snap: ResourceSnapshot) -> Option<u8> {
    rate_remaining_pct(snap.remaining, snap.limit)
}

/// Whether the rate-budget snapshot is below the guard threshold. Treats
/// "no observation yet" as "do not gate" (returns `false`) so a fresh
/// account isn't blocked before its first response arrives.
fn under_guard(snap: ResourceSnapshot, guard_pct: u8) -> bool {
    match resource_remaining_pct(snap) {
        Some(pct) => pct < guard_pct,
        None => false,
    }
}

/// Short label used in the cycle's status / activity messages. Lets the UI
/// distinguish "search budget low" from a generic "rate limited" while still
/// matching the wire-form `x-ratelimit-resource` value.
fn resource_label(resource: RateResource) -> &'static str {
    resource.as_str()
}

/// Cap on the body excerpt surfaced through the null-detail diagnostic
/// (#402). Wide enough to carry GitHub's typical `errors` array (one
/// `message + path + extensions`) without becoming a wall of text in the
/// activity row.
const NULL_DETAIL_BODY_PREFIX_LIMIT: usize = 256;

/// Build a UTF-8-lossy excerpt of a GraphQL response body for diagnostic
/// surfaces. Truncated to [`NULL_DETAIL_BODY_PREFIX_LIMIT`] bytes with an
/// ellipsis appended when the source overflowed. Used by the null-detail
/// diagnostic only (#402); the body is sourced from the response we already
/// hold in memory, so there's no extra read cost.
fn body_prefix_for_log(body: &bytes::Bytes) -> String {
    let limit = NULL_DETAIL_BODY_PREFIX_LIMIT;
    let slice = if body.len() <= limit {
        &body[..]
    } else {
        &body[..limit]
    };
    let mut out = String::from_utf8_lossy(slice).into_owned();
    if body.len() > limit {
        out.push('\u{2026}');
    }
    out
}

/// Run a single sync cycle for one account. Public for integration tests.
pub async fn run_one_cycle(
    ctx: &WorkerContext,
    client: &GitHubClient,
    account: &Account,
) -> SyncCycleReport {
    let snapshot = client.rate().snapshot();
    // The cycle opens with discovery (Search API). Gate on the search
    // sub-budget so a tight 30 req/min search bucket doesn't get masked by
    // the much larger core / graphql buckets sitting at 100%.
    let entry_bucket = RateResource::Search;
    let entry_snap = snapshot.for_bucket(entry_bucket);
    if under_guard(entry_snap, RATE_BUDGET_GUARD_PCT) {
        let pct = resource_remaining_pct(entry_snap).unwrap_or(0);
        emit_rate_limit(
            ctx,
            account,
            pct,
            entry_snap.limit,
            entry_snap.time_until_reset(),
            Some(entry_bucket),
        );
        emit_activity_rate_pause(ctx, account, entry_snap.time_until_reset(), pct);
        let label = resource_label(entry_bucket);
        let state = ctx.state.update(account.id, |s| {
            s.phase = SyncPhase::RateLimited;
            s.message = Some(format!("{label} budget {pct}%, skipping cycle"));
            s.rate_remaining_pct = Some(pct);
            s.rate_limit = Some(entry_snap.limit);
            s.next_sync_in_seconds = next_sync_hint(ctx, None);
        });
        emit_status(&ctx.emit, &state);
        return SyncCycleReport {
            account_id: account.id,
            repos_visited: 0,
            prs_visited: 0,
            requests_made: 0,
            outcome: CycleOutcome::Skipped {
                reason: SkipReason::RateBudgetGuard {
                    rate_remaining_pct: pct,
                },
            },
        };
    }

    // Mark cycle as in-flight.
    let state = ctx.state.update(account.id, |s| {
        s.phase = SyncPhase::Syncing;
        s.next_sync_in_seconds = None;
        s.message = None;
    });
    emit_status(&ctx.emit, &state);
    emit_activity_cycle_started(ctx, account);

    // Per-bucket pre-cycle snapshots so `finalise_with_budget` can compute
    // a sane delta across the three independent sub-budgets. Using the
    // top-level "most constrained" view would flip mid-cycle as different
    // buckets bottom out, producing nonsense `requests_made` numbers.
    let pre_budget = PreCycleBudget::from_snapshot(&snapshot);
    let cycle_start = unix_now();
    let mut report = SyncCycleReport {
        account_id: account.id,
        repos_visited: 0,
        prs_visited: 0,
        requests_made: 0,
        outcome: CycleOutcome::Completed,
    };

    // Phase 1: Discovery. Search-API fan-out, ADR 0009. Failure here is
    // treated like any other phase failure: don't run enrichment, don't prune.
    emit_activity_phase_started(ctx, account, SyncPhaseLabel::Discovery);
    match crate::sync::discovery::discover_account(
        &ctx.db,
        client,
        account.id,
        &account.login,
        cycle_start,
    )
    .await
    {
        Ok((discovered, discovery_report)) => {
            let summary = if discovery_report.pages_skipped_via_cache > 0 {
                format!(
                    "discovered {} pull request(s) ({} page(s) cached)",
                    discovered.len(),
                    discovery_report.pages_skipped_via_cache,
                )
            } else {
                format!("discovered {} pull request(s)", discovered.len())
            };
            emit_activity_phase_completed_with_skips(
                ctx,
                account,
                SyncPhaseLabel::Discovery,
                summary,
                discovery_report.pages_skipped_via_cache as u32,
            );
        }
        Err(DiscoveryError::GitHub(GitHubError::Unauthorized))
        | Err(DiscoveryError::GitHub(GitHubError::Auth(
            crate::github::auth::AuthError::Missing(_) | crate::github::auth::AuthError::Empty(_),
        ))) => {
            let state = ctx.state.update(account.id, |s| {
                s.phase = SyncPhase::Unauthorized;
                s.message = Some("token rejected; reauthenticate".into());
                s.next_sync_in_seconds = None;
            });
            emit_status(&ctx.emit, &state);
            emit_activity_cycle_failed(ctx, account, "discovery", "token rejected; reauthenticate");
            report.outcome = CycleOutcome::Unauthorized;
            return finalise_with_budget(report, client, pre_budget);
        }
        Err(DiscoveryError::GitHub(GitHubError::RateLimited { retry_after })) => {
            // Discovery hit the search bucket - surface that hint so the
            // status bar shows "search budget low" instead of the generic
            // "rate limited" message a multi-account viewer can't act on.
            let bucket = RateResource::Search;
            let reset_in = retry_after.map(|d| d.as_secs());
            let label = resource_label(bucket);
            let state = ctx.state.update(account.id, |s| {
                s.phase = SyncPhase::RateLimited;
                s.message = Some(format!("{label} budget low; upstream throttled"));
                s.next_sync_in_seconds = next_sync_hint(ctx, reset_in);
            });
            emit_status(&ctx.emit, &state);
            let bucket_snap = client.rate().snapshot().for_bucket(bucket);
            emit_rate_limit(
                ctx,
                account,
                0,
                bucket_snap.limit,
                retry_after,
                Some(bucket),
            );
            emit_activity_rate_pause(ctx, account, retry_after, 0);
            report.outcome = CycleOutcome::RateLimited {
                reset_in_seconds: reset_in,
            };
            return finalise_with_budget(report, client, pre_budget);
        }
        Err(err) => {
            let message = format!("discovery: {err}");
            record_failure(ctx, account, &message);
            emit_activity_cycle_failed(ctx, account, "discovery", &err.to_string());
            report.outcome = CycleOutcome::Failed { message };
            return finalise_with_budget(report, client, pre_budget);
        }
    }

    // Re-read repos after discovery so freshly-upserted rows feed the
    // enrichment loop within the same cycle.
    let repos = match list_repos_for_account(&ctx.db, account.id) {
        Ok(r) => r,
        Err(err) => {
            record_failure(ctx, account, &format!("read repos: {err}"));
            emit_activity_cycle_failed(ctx, account, "enrichment", &err.to_string());
            report.outcome = CycleOutcome::Failed {
                message: err.to_string(),
            };
            return report;
        }
    };

    if repos.is_empty() {
        // Discovery completed but found no PRs and no repos were pre-seeded.
        // Still prune so a viewer who just dropped their last relation gets a
        // clean slate on this cycle.
        let _ = crate::sync::discovery::prune_stale_relations_for_account(
            &ctx.db,
            account.id,
            cycle_start,
        );
        // Run the auto-archive sweep even on the empty-repos path: the
        // predicate reads global `pull_requests.state` + `updated_at`, so
        // another account's cycle may have refreshed the state of PRs
        // whose relations also live under this empty-repo account. Skipping
        // the sweep here would leave a single-account-no-repos viewer with
        // stale archive coverage.
        run_auto_archive_sweep(&ctx.db);
        run_archive_retention_sweep(&ctx.db);
        ctx.badge.refresh();
        let finished_at = SystemTime::now();
        finish_completed(ctx, account, client, finished_at);
        emit_activity_cycle_completed(ctx, account, 0, "no repos tracked");
        report.outcome = CycleOutcome::Skipped {
            reason: SkipReason::NoReposConfigured,
        };
        return finalise_with_budget(report, client, pre_budget);
    }

    let total_prs = count_prs_across_repos(&ctx.db, &repos);
    emit_activity_phase_started(ctx, account, SyncPhaseLabel::Enrichment);
    let mut enriched: u32 = 0;
    let mut detail_cache_skips: u32 = 0;
    for repo in &repos {
        report.repos_visited += 1;
        match sync_repo(
            ctx,
            client,
            account,
            repo,
            total_prs,
            &mut enriched,
            &mut detail_cache_skips,
        )
        .await
        {
            Ok(prs_visited) => {
                report.prs_visited += prs_visited;
            }
            Err(SyncRepoError::Unauthorized) => {
                let state = ctx.state.update(account.id, |s| {
                    s.phase = SyncPhase::Unauthorized;
                    s.message = Some("token rejected; reauthenticate".into());
                    s.next_sync_in_seconds = None;
                });
                emit_status(&ctx.emit, &state);
                emit_activity_cycle_failed(
                    ctx,
                    account,
                    "enrichment",
                    "token rejected; reauthenticate",
                );
                report.outcome = CycleOutcome::Unauthorized;
                return finalise_with_budget(report, client, pre_budget);
            }
            Err(SyncRepoError::RateLimited {
                retry_after,
                resource,
            }) => {
                let reset_in = retry_after.map(|d| d.as_secs());
                let label = resource_label(resource);
                let state = ctx.state.update(account.id, |s| {
                    s.phase = SyncPhase::RateLimited;
                    s.message = Some(format!("{label} budget low; upstream throttled"));
                    s.next_sync_in_seconds = next_sync_hint(ctx, reset_in);
                });
                emit_status(&ctx.emit, &state);
                let bucket_snap = client.rate().snapshot().for_bucket(resource);
                emit_rate_limit(
                    ctx,
                    account,
                    0,
                    bucket_snap.limit,
                    retry_after,
                    Some(resource),
                );
                emit_activity_rate_pause(ctx, account, retry_after, 0);
                report.outcome = CycleOutcome::RateLimited {
                    reset_in_seconds: reset_in,
                };
                return finalise_with_budget(report, client, pre_budget);
            }
            Err(SyncRepoError::Other(message)) => {
                record_failure(ctx, account, &message);
                emit_activity_cycle_failed(ctx, account, "enrichment", &message);
                report.outcome = CycleOutcome::Failed { message };
                return finalise_with_budget(report, client, pre_budget);
            }
        }
    }
    let enrichment_summary = if detail_cache_skips > 0 {
        format!("fetched detail for {enriched} pull request(s) ({detail_cache_skips} cached)",)
    } else {
        format!("fetched detail for {enriched} pull request(s)")
    };
    emit_activity_phase_completed_with_skips(
        ctx,
        account,
        SyncPhaseLabel::Enrichment,
        enrichment_summary,
        detail_cache_skips,
    );

    // Phase final: Pruning. Runs only when enrichment completes so a transient
    // discovery hiccup doesn't drop everything (the contract calls this out).
    emit_activity_phase_started(ctx, account, SyncPhaseLabel::Pruning);
    let pruned = match crate::sync::discovery::prune_stale_relations_for_account(
        &ctx.db,
        account.id,
        cycle_start,
    ) {
        Ok(n) => n,
        Err(err) => {
            // A prune failure is logged, not fatal: stale rows are merely cosmetic
            // and the next cycle's prune will retry.
            tracing::warn!(account_id = account.id, %err, "sync prune failed");
            0
        }
    };
    emit_activity_phase_completed(
        ctx,
        account,
        SyncPhaseLabel::Pruning,
        format!("removed {pruned} stale relation(s)"),
    );

    // Auto-archive sweep (ADR 0018). The predicate is account-agnostic - it
    // reads `pull_requests.state` and `updated_at`, which every cycle writes
    // to from its own per-account perspective. Running once per cycle (even
    // when N accounts are tracked, that's N runs per global cycle) is fine
    // because the `archived_at IS NULL` predicate makes the sweep idempotent:
    // the second account's cycle skips rows the first account's cycle
    // already archived. A failed sweep is logged and the cycle still
    // completes; the next cycle retries.
    run_auto_archive_sweep(&ctx.db);

    // Hard-delete PRs whose every viewer relation has been archived for
    // more than 60 days, plus everything that cascades from them. Bounds
    // DB growth without affecting recently-archived rows or open PRs.
    // Best-effort; a failure logs and continues.
    run_archive_retention_sweep(&ctx.db);

    // Dock badge refresh (ADR 0017 decision 3). Sits after both sweeps so
    // the count reflects the per-account fan-out, the archive flip, and
    // any retention-driven deletes in a single update. The non-macOS sink
    // is a no-op; the macOS sink writes the global unread count to the
    // main window.
    ctx.badge.refresh();

    let finished_at = SystemTime::now();
    finish_completed(ctx, account, client, finished_at);
    emit_activity_cycle_completed(
        ctx,
        account,
        enriched,
        format!("synced {enriched} pull request(s)"),
    );
    finalise_with_budget(report, client, pre_budget)
}

fn finish_completed(
    ctx: &WorkerContext,
    account: &Account,
    client: &GitHubClient,
    finished_at: SystemTime,
) {
    let snap = client.rate().snapshot();
    // Top-level snapshot mirrors the most-constrained sub-bucket so the
    // status bar's single budget label surfaces the worst-case across
    // core / search / graphql instead of whatever bucket was updated last.
    let pct = rate_remaining_pct(snap.remaining, snap.limit);
    let synced_at = format_rfc3339(finished_at);
    let state = ctx.state.update(account.id, |s| {
        s.phase = SyncPhase::Synced;
        s.last_synced_at = synced_at.clone();
        s.next_sync_in_seconds = next_sync_hint(ctx, None);
        s.message = None;
        if pct.is_some() {
            s.rate_remaining_pct = pct;
        }
        if snap.limit > 0 {
            s.rate_limit = Some(snap.limit);
        }
    });
    emit_status(&ctx.emit, &state);
}

/// Per-bucket "before the cycle" view used to compute `requests_made` after
/// the cycle finishes. Captures each sub-budget independently because the
/// top-level snapshot now mirrors the most-constrained bucket and would flip
/// mid-cycle as different buckets bottom out, producing nonsense deltas.
#[derive(Debug, Clone, Copy)]
struct PreCycleBudget {
    core_used: i64,
    search_used: i64,
    graphql_used: i64,
    core_remaining: i64,
    search_remaining: i64,
    graphql_remaining: i64,
}

impl PreCycleBudget {
    fn from_snapshot(snap: &crate::github::RateSnapshot) -> Self {
        Self {
            core_used: snap.core.used.max(0),
            search_used: snap.search.used.max(0),
            graphql_used: snap.graphql.used.max(0),
            core_remaining: snap.core.remaining,
            search_remaining: snap.search.remaining,
            graphql_remaining: snap.graphql.remaining,
        }
    }
}

fn finalise_with_budget(
    mut report: SyncCycleReport,
    client: &GitHubClient,
    pre: PreCycleBudget,
) -> SyncCycleReport {
    let snap = client.rate().snapshot();
    // Sum the deltas across all three buckets so `requests_made` reflects
    // the full cycle's HTTP footprint, not just the most-constrained bucket.
    // Prefer `used` delta per-bucket; fall back to `remaining` delta if
    // `used` isn't surfaced by an Enterprise host.
    let bucket_delta = |post_used: i64, pre_used: i64, post_remaining: i64, pre_remaining: i64| {
        let by_used = (post_used.max(0) - pre_used).max(0);
        let by_remaining = (pre_remaining - post_remaining).max(0);
        by_used.max(by_remaining)
    };
    let delta = bucket_delta(
        snap.core.used,
        pre.core_used,
        snap.core.remaining,
        pre.core_remaining,
    ) + bucket_delta(
        snap.search.used,
        pre.search_used,
        snap.search.remaining,
        pre.search_remaining,
    ) + bucket_delta(
        snap.graphql.used,
        pre.graphql_used,
        snap.graphql.remaining,
        pre.graphql_remaining,
    );
    report.requests_made = delta as u64;
    report
}

fn emit_rate_limit(
    ctx: &WorkerContext,
    account: &Account,
    rate_remaining_pct: u8,
    limit: i64,
    reset_in: Option<Duration>,
    resource: Option<RateResource>,
) {
    let payload = SyncRateLimitPayload {
        account_id: account.id,
        rate_remaining_pct,
        limit: if limit > 0 { Some(limit) } else { None },
        reset_in_seconds: reset_in.map(|d| d.as_secs()),
        resource: resource.map(|r| r.as_str().to_string()),
    };
    ctx.emit.emit(
        SYNC_RATE_LIMIT_EVENT,
        &serde_json::to_value(payload).unwrap_or(serde_json::Value::Null),
    );
}

#[derive(Debug)]
enum SyncRepoError {
    Unauthorized,
    RateLimited {
        retry_after: Option<Duration>,
        resource: RateResource,
    },
    Other(String),
}

impl SyncRepoError {
    /// Map a `GitHubError` produced by a specific phase into the worker's
    /// internal error, tagging rate-limit failures with the bucket the
    /// failing call hit. This lets the enrichment loop report "graphql
    /// budget low" vs "core budget low" without a separate channel.
    fn from_err_for(err: GitHubError, resource: RateResource) -> Self {
        match err {
            GitHubError::Unauthorized => SyncRepoError::Unauthorized,
            GitHubError::Auth(
                crate::github::auth::AuthError::Missing(_)
                | crate::github::auth::AuthError::Empty(_),
            ) => SyncRepoError::Unauthorized,
            GitHubError::RateLimited { retry_after } => SyncRepoError::RateLimited {
                retry_after,
                resource,
            },
            other => SyncRepoError::Other(other.to_string()),
        }
    }
}

/// Sync one repo's known PRs. v1 reads PR rows already in the DB; repo
/// discovery lands in M2 (see PR body).
///
/// `total_prs` and `enriched_so_far` thread the cycle-wide progress through
/// the per-repo loop so activity-feed `PhaseProgress` events surface a single
/// monotonically-increasing counter against the full PR count.
async fn sync_repo(
    ctx: &WorkerContext,
    client: &GitHubClient,
    account: &Account,
    repo: &RepoRow,
    total_prs: u32,
    enriched_so_far: &mut u32,
    detail_cache_skips: &mut u32,
) -> Result<usize, SyncRepoError> {
    let prs = list_prs_for_repo(&ctx.db, repo.id)
        .map_err(|e| SyncRepoError::Other(format!("read prs: {e}")))?;

    let mut visited = 0usize;
    for pr in &prs {
        visited += 1;
        // Per-PR sub-budget gate (issue #235): skip the PR if either of the
        // buckets the next two calls will hit is already below the guard.
        // Returning a tagged `RateLimited` carries the resource hint up to
        // the cycle's error handler so the status bar's message names the
        // right bucket.
        let snapshot = client.rate().snapshot();
        let graphql_snap = snapshot.for_bucket(RateResource::Graphql);
        if under_guard(graphql_snap, RATE_BUDGET_GUARD_PCT) {
            return Err(SyncRepoError::RateLimited {
                retry_after: graphql_snap.time_until_reset(),
                resource: RateResource::Graphql,
            });
        }
        let core_snap = snapshot.for_bucket(RateResource::Core);
        if under_guard(core_snap, RATE_BUDGET_GUARD_PCT) {
            return Err(SyncRepoError::RateLimited {
                retry_after: core_snap.time_until_reset(),
                resource: RateResource::Core,
            });
        }

        // Self-heal probe (issue #397). A prior cycle may have stamped both
        // cache markers without ever populating `requested_reviewers` /
        // `reviews` for this PR (transient empty payload, body-hash collision
        // against a stale cache key, partial failure). When the local detail
        // tables are empty AND a previous cycle's pre-flight marker is
        // already in place AND we haven't already attempted a repair this
        // cache lifetime, force the GraphQL detail fetch + write path so the
        // rows hydrate from a fresh response. Gating on the existing
        // pre-flight marker keeps the very first cycle for a PR on the
        // normal path - state is always empty for a freshly-discovered PR,
        // and the existing cache logic already runs the full fetch + write.
        // The repair marker stops the probe from refetching the same
        // genuinely-empty PR (e.g. a new draft with no reviewers and no
        // reviews) on every subsequent cycle; it's refreshed on every
        // successful detail write so it stays in sync with the cache it
        // gates.
        let pr_detail_marker = pr_detail_marker_key(pr.id);
        let detail_repair_marker = pr_detail_repair_marker_key(pr.id);
        let state_empty = detail_state_appears_empty(&ctx.db, pr.id).map_err(|e| {
            SyncRepoError::Other(format!("probe detail state PR #{}: {e}", pr.number))
        })?;
        let prior_cycle_ran = client.graphql_cache_entry(&pr_detail_marker).is_some();
        let repair_attempted = client.graphql_cache_entry(&detail_repair_marker).is_some();
        let force_repair = state_empty && prior_cycle_ran && !repair_attempted;

        // Pre-flight skip (issue #232): if discovery just wrote a
        // `pull_requests.updated_at` that matches the previous-cycle marker
        // for this PR, skip the GraphQL PR-detail round trip entirely. The
        // GraphQL endpoint doesn't honour `If-None-Match`, so this is how we
        // recover the "nothing changed" saving REST already gets from ETag
        // 304s. Timeline still runs (REST-conditional, ADR 0004) so the
        // latest-status-change derivation stays current.
        let skip_detail = !force_repair
            && client
                .graphql_cache_entry(&pr_detail_marker)
                .and_then(|entry| entry.body_sha256)
                .is_some_and(|stored| {
                    stored == crate::github::client::sha256(&pr_detail_marker_bytes(pr.updated_at))
                });

        let (detail, detail_body) = if skip_detail {
            *detail_cache_skips = detail_cache_skips.saturating_add(1);
            (None, bytes::Bytes::new())
        } else {
            // PR detail (GraphQL) — primary surface per ADR 0006.
            // Wrapped in `timeout` so a hung upstream call doesn't stall the loop.
            let (fetched, body) = timeout(
                Duration::from_secs(30),
                client.pr_detail_with_raw(crate::github::graphql::PrCoord {
                    owner: &repo.owner,
                    name: &repo.name,
                    number: pr.number,
                }),
            )
            .await
            .map_err(|_| SyncRepoError::Other(format!("pr_detail timeout for #{}", pr.number)))?
            .map_err(|err| SyncRepoError::from_err_for(err, RateResource::Graphql))?;
            // Diagnostic (#402): GraphQL responded but `repository.pullRequest`
            // resolved to null. The detail-derived columns + conversation
            // tables stay empty in the DB, and the cache markers stamped
            // below currently lock the empty state in (the marker fix is
            // tracked separately, #403). Surface the body prefix to the
            // activity feed AND tracing so the user can see what GitHub
            // actually sent back without enabling `RUST_LOG`.
            if fetched.is_none() {
                let prefix = body_prefix_for_log(&body);
                tracing::warn!(
                    account_id = account.id,
                    owner = %repo.owner,
                    name = %repo.name,
                    number = pr.number,
                    body_prefix = %prefix,
                    "pr_detail returned null pullRequest"
                );
                emit_activity_pr_detail_empty(
                    ctx,
                    account,
                    &repo.owner,
                    &repo.name,
                    pr.number,
                    &prefix,
                );
            }
            // Stamp the issue #232 marker so the next cycle's pre-flight
            // comparison sees the freshly-persisted `updated_at`. Falling back
            // to `pr.updated_at` keeps the marker aligned when GraphQL returns
            // a thin payload (no `updatedAt` field). Skip the stamp entirely
            // when `fetched` is `None` (#403): a null `repository.pullRequest`
            // means `write_pr_updates` won't touch the detail block, so
            // leaving the marker absent lets the next cycle retry on the
            // normal path rather than locking the empty state in.
            if fetched.is_some() {
                let marker_for_next_cycle = fetched
                    .as_ref()
                    .and_then(|d| rfc3339_to_unix(&d.updated_at))
                    .unwrap_or(pr.updated_at);
                client.cache_graphql_body(
                    &pr_detail_marker,
                    &pr_detail_marker_bytes(marker_for_next_cycle),
                );
            }
            (fetched, body)
        };

        // Post-flight body-hash cache (ADR 0004, issue #234): only relevant
        // when we actually made the call. On a byte-identical detail body,
        // skip the detail-driven DB writes (the prior cycle's values are
        // still authoritative). Timeline still runs (REST ETag) so the
        // latest-status-change derivation picks up new events. When the
        // self-heal probe (#397) flagged the local state as empty, the cache
        // hit is overridden so `write_pr_updates` repopulates the missing
        // `requested_reviewers` / `reviews` rows from the freshly-fetched
        // detail.
        let detail_for_write = if skip_detail {
            None
        } else if detail.is_none() {
            // GraphQL responded with `repository.pullRequest = null` (#403).
            // Don't stamp the body-hash via `graphql_body_unchanged` here -
            // that would persist the empty-payload SHA and lock the empty
            // state in across future cycles. Leave the marker untouched so
            // the next cycle refetches.
            None
        } else {
            let detail_cache_key = format!("pr_detail:{}/{}#{}", repo.owner, repo.name, pr.number);
            let detail_cache_hit = client.graphql_body_unchanged(&detail_cache_key, &detail_body);
            if detail_cache_hit && !force_repair {
                *detail_cache_skips = detail_cache_skips.saturating_add(1);
                None
            } else {
                detail.as_ref()
            }
        };

        // Stamp the repair-attempted marker whenever the self-heal probe
        // ran the fetch (#397). Refreshing it on every successful detail
        // write keeps it aligned with the cache it gates - if a later cycle
        // legitimately writes detail again, the marker survives and the
        // probe stays quiet. The byte stored is irrelevant; only the
        // presence of the entry is read by the probe. Skip the stamp when
        // `detail` is `None` (#403): a null `repository.pullRequest` means
        // the repair didn't actually write anything, so leaving the marker
        // absent lets the next cycle retry instead of suppressing the probe.
        if force_repair && detail.is_some() {
            client.cache_graphql_body(
                &detail_repair_marker,
                &pr_detail_marker_bytes(pr.updated_at),
            );
        }

        // Timeline (REST) — feeds the latest-status-change derivation (ADR 0007).
        let timeline = timeout(
            Duration::from_secs(30),
            list_pr_timeline(
                client,
                RepoCoord {
                    owner: &repo.owner,
                    repo: &repo.name,
                },
                pr.number as u32,
                5,
            ),
        )
        .await
        .map_err(|_| SyncRepoError::Other(format!("timeline timeout for #{}", pr.number)))?
        .map_err(|err| SyncRepoError::from_err_for(err, RateResource::Core))?;

        let events = match timeline {
            ListTimeline::Events(e) => Some(e),
            ListTimeline::NotModified => None,
        };

        // Persist whatever new data we have. When the pre-flight skip
        // (#232) or the post-flight body-hash check (#234) elided detail,
        // `detail_for_write` is `None` and `write_pr_updates` only touches
        // the timeline-derived columns and timeline events.
        let triggers = write_pr_updates(
            &ctx.db,
            account.id,
            repo.id,
            pr.id,
            detail_for_write,
            events.as_deref(),
        )
        .map_err(|e| SyncRepoError::Other(format!("persist PR #{}: {e}", pr.number)))?;

        // Dispatch notification triggers after the per-PR transaction
        // commits. Running the formatter + plugin call outside the
        // transaction keeps the DB lock short - the sink owns its own
        // gating (master switch + per-trigger toggle + permission state)
        // and the formatter only reads the freshly-committed rows. A
        // formatter miss or sink failure is logged inside the helper.
        dispatch_triggers(&ctx.db, &ctx.notify_sink, &triggers);

        // Activity feed: emit per-PR detail or skip event, then a phase
        // progress tick. Detail's URL is the canonical deep-link target;
        // fall back to the GitHub web URL when GraphQL was skipped or the
        // payload was thin.
        *enriched_so_far = enriched_so_far.saturating_add(1);
        let pr_url = detail.as_ref().map(|d| d.url.clone()).unwrap_or_else(|| {
            format!(
                "https://github.com/{}/{}/pull/{}",
                repo.owner, repo.name, pr.number
            )
        });
        if skip_detail {
            emit_activity_pr_skipped_no_change(
                ctx,
                account,
                &repo.owner,
                &repo.name,
                pr.number,
                &pr_url,
            );
        } else {
            emit_activity_pr_fetched(ctx, account, &repo.owner, &repo.name, pr.number, &pr_url);
        }
        emit_activity_phase_progress(
            ctx,
            account,
            SyncPhaseLabel::Enrichment,
            *enriched_so_far,
            total_prs,
        );
    }
    Ok(visited)
}
