//! Background sync worker — one task per account, isolated.
//!
//! Lifetime: `spawn_worker` returns a [`WorkerHandle`] the Tauri setup hook
//! stashes in app state. The handle owns one [`CancellationToken`] per
//! account and a parent token that cancels every loop on shutdown. Manual
//! refresh is a `tokio::sync::Notify` per account: nudging it makes the loop
//! short-circuit its sleep and run one cycle immediately (subject to the
//! rate-budget guard).
//!
//! Per-account isolation is enforced by spawning a distinct task per account
//! on Tauri's async runtime: one task panicking or hanging never touches the
//! others, and the parent token cancels them all on app shutdown. We use
//! `tauri::async_runtime::spawn` (not `tokio::spawn`) so the setup hook can
//! start the pool without entering a runtime first.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rusqlite::params;
use tauri::async_runtime::JoinHandle;
use tokio::sync::Notify;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

use crate::auth::store::{Account, AccountStore};
use crate::db::DbHandle;
use crate::github::auth::TokenSource;
use crate::github::{
    list_pr_timeline, AccountHandle, AccountId, EtagStore, GitHubClient, GitHubError, ListTimeline,
    RateResource, RepoCoord, ResourceSnapshot,
};
use crate::notify::{
    format_trigger, BadgeSink, NotificationKind, NotificationSinkHandle, NotificationTrigger,
};
use crate::sync::activity::{
    record as record_activity, ActivityBuffer, ActivityEventBuilder, ActivityKind, ActivityLevel,
    SyncPhaseLabel,
};
use crate::sync::discovery::DiscoveryError;
use crate::sync::events::{
    SyncErrorPayload, SyncRateLimitPayload, SyncStatusPayload, SYNC_ERROR_EVENT,
    SYNC_RATE_LIMIT_EVENT, SYNC_STATUS_EVENT,
};
use crate::sync::scheduler::{SchedulerConfig, RATE_BUDGET_GUARD_PCT};
use crate::sync::state::{
    format_rfc3339, seconds_floor, AccountSyncState, SyncPhase, SyncStateMap,
};

/// Outcome of one sync cycle. Used by tests to assert request budgets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCycleReport {
    pub account_id: AccountId,
    pub repos_visited: usize,
    pub prs_visited: usize,
    pub requests_made: u64,
    pub outcome: CycleOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CycleOutcome {
    Completed,
    Unauthorized,
    RateLimited { reset_in_seconds: Option<u64> },
    Skipped { reason: SkipReason },
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    RateBudgetGuard { pct: u8 },
    NoReposConfigured,
}

/// Abstract sink for Tauri events. The worker writes through this so unit
/// tests can capture emissions without booting a Tauri app.
pub trait EmitSink: Send + Sync {
    fn emit(&self, event: &str, payload: &serde_json::Value);
}

/// `EmitSink` impl that wraps a Tauri `AppHandle`. Used in production.
pub struct AppHandleEmitter<R: tauri::Runtime> {
    handle: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> AppHandleEmitter<R> {
    pub fn new(handle: tauri::AppHandle<R>) -> Self {
        Self { handle }
    }
}

impl<R: tauri::Runtime> EmitSink for AppHandleEmitter<R> {
    fn emit(&self, event: &str, payload: &serde_json::Value) {
        use tauri::Emitter;
        if let Err(err) = self.handle.emit(event, payload) {
            // Logged, not propagated — a failed emission must not stall the
            // sync loop (the next tick will publish a fresh status anyway).
            eprintln!("sync emit {event} failed: {err}");
        }
    }
}

/// Factory for per-account `GitHubClient`s. The default impl is the one used
/// in production; tests inject a custom one that points at wiremock.
pub trait ClientFactory: Send + Sync {
    fn build(&self, account: &Account) -> Result<GitHubClient, GitHubError>;
}

/// Production factory: keychain-backed token source + shared ETag store.
pub struct DefaultClientFactory {
    token_source: Arc<dyn TokenSource>,
    etags: Arc<dyn EtagStore>,
}

impl DefaultClientFactory {
    pub fn new(token_source: Arc<dyn TokenSource>, etags: Arc<dyn EtagStore>) -> Self {
        Self {
            token_source,
            etags,
        }
    }
}

impl ClientFactory for DefaultClientFactory {
    fn build(&self, account: &Account) -> Result<GitHubClient, GitHubError> {
        let handle = AccountHandle::new(account.id, account.host.clone(), account.label.clone());
        GitHubClient::builder()
            .account(handle)
            .token_source(self.token_source.clone())
            .etag_store(self.etags.clone())
            .build()
    }
}

/// Reauth notifier: fires whenever a 401 puts an account into the suspended
/// state. The Tauri layer wires this to `auth::commands::emit_reauth_required`.
pub trait ReauthNotifier: Send + Sync {
    fn notify(&self, account: &Account);
}

/// Reauth notifier impl that emits via a Tauri `AppHandle`.
pub struct AppHandleReauth<R: tauri::Runtime> {
    handle: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> AppHandleReauth<R> {
    pub fn new(handle: tauri::AppHandle<R>) -> Self {
        Self { handle }
    }
}

impl<R: tauri::Runtime> ReauthNotifier for AppHandleReauth<R> {
    fn notify(&self, account: &Account) {
        crate::auth::commands::emit_reauth_required(&self.handle, account);
    }
}

/// Worker context shared between every account loop. Cheap to clone.
#[derive(Clone)]
pub struct WorkerContext {
    pub db: DbHandle,
    pub accounts: Arc<dyn AccountStore>,
    pub clients: Arc<dyn ClientFactory>,
    pub config: Arc<SchedulerConfig>,
    pub state: SyncStateMap,
    pub emit: Arc<dyn EmitSink>,
    pub reauth: Arc<dyn ReauthNotifier>,
    /// macOS dock badge sink (ADR 0017 decision 3). Refreshed once per cycle
    /// after the auto-archive sweep so the per-account fan-out and the sweep
    /// both feed into a single post-cycle update. Non-macOS impls no-op.
    pub badge: Arc<dyn BadgeSink>,
    /// Diagnostic activity buffer (issue #122). Cloned into every cycle so the
    /// status-bar panel sees real-time phase / per-PR / error events alongside
    /// the existing status / error events.
    pub activity: ActivityBuffer,
    /// Notification dispatch sink (ADR 0017, issue #192). The per-PR
    /// enrichment write path collects triggers from
    /// [`crate::triage::query::recompute_needs_attention`] and hands them to
    /// this sink after the DB transaction commits. The sink owns master
    /// switch + per-trigger gating + permission state (ADR 0017 decision 5);
    /// the worker only forwards.
    pub notify_sink: NotificationSinkHandle,
}

/// Public handle to the running worker pool. Holds the per-account
/// `CancellationToken`s + `Notify`s used by manual refresh / shutdown.
pub struct WorkerHandle {
    ctx: WorkerContext,
    parent: CancellationToken,
    accounts: Mutex<HashMap<AccountId, AccountSlot>>,
}

struct AccountSlot {
    cancel: CancellationToken,
    refresh: Arc<Notify>,
    _task: JoinHandle<()>,
}

impl WorkerHandle {
    pub fn context(&self) -> &WorkerContext {
        &self.ctx
    }

    pub fn config(&self) -> &Arc<SchedulerConfig> {
        &self.ctx.config
    }

    pub fn state(&self) -> &SyncStateMap {
        &self.ctx.state
    }

    /// Nudge one account to run a cycle immediately. No-op when the account
    /// isn't currently being tracked.
    pub fn refresh_account(&self, account_id: AccountId) -> bool {
        let guard = self.accounts.lock().expect("worker slots poisoned");
        match guard.get(&account_id) {
            Some(slot) => {
                slot.refresh.notify_one();
                true
            }
            None => false,
        }
    }

    /// Nudge every tracked account to run a cycle immediately.
    pub fn refresh_all(&self) -> usize {
        let guard = self.accounts.lock().expect("worker slots poisoned");
        for slot in guard.values() {
            slot.refresh.notify_one();
        }
        guard.len()
    }

    /// Cancel every running loop. Idempotent.
    pub fn shutdown(&self) {
        self.parent.cancel();
        if let Ok(mut guard) = self.accounts.lock() {
            for (_, slot) in guard.drain() {
                slot.cancel.cancel();
            }
        }
    }

    /// Start polling for a newly-added account. No-op (returns `false`) if the
    /// account is already tracked. Called by `auth::commands::add_account` via
    /// the [`AccountChangeListener`] hook so new accounts sync without a
    /// restart.
    pub fn add_account(&self, account: Account) -> bool {
        let account_id = account.id;
        let mut guard = self.accounts.lock().expect("worker slots poisoned");
        if guard.contains_key(&account_id) {
            return false;
        }
        let (_, slot) = start_account_task(self.ctx.clone(), account, self.parent.clone());
        guard.insert(account_id, slot);
        true
    }

    /// Stop polling for an account and forget its sync state. Returns `false`
    /// if the account wasn't being tracked. Called by
    /// `auth::commands::remove_account` via the listener hook.
    pub fn remove_account(&self, account_id: AccountId) -> bool {
        let removed = {
            let mut guard = self.accounts.lock().expect("worker slots poisoned");
            guard.remove(&account_id)
        };
        match removed {
            Some(slot) => {
                slot.cancel.cancel();
                self.ctx.state.forget(account_id);
                true
            }
            None => false,
        }
    }
}

impl crate::auth::commands::AccountChangeListener for WorkerHandle {
    fn on_added(&self, account: &Account) {
        self.add_account(account.clone());
    }

    fn on_removed(&self, account_id: AccountId) {
        self.remove_account(account_id);
    }

    fn on_token_updated(&self, account_id: AccountId) {
        // Per-account re-auth (issue #59): nudge the loop so a parked
        // `SyncPhase::Unauthorized` slot exits its suspend branch and runs a
        // cycle with the freshly-stored PAT instead of waiting for the next
        // interval tick. Untracked accounts are a no-op.
        self.refresh_account(account_id);
    }
}

/// Spawn one task per currently-known account. Returns the handle the caller
/// stashes in Tauri state. Failures inside individual tasks never stop the
/// others; they emit `sync://error` and continue at the next tick.
pub fn spawn_worker(ctx: WorkerContext) -> WorkerHandle {
    let parent = CancellationToken::new();
    let mut slots = HashMap::new();

    let accounts = match ctx.accounts.list() {
        Ok(list) => list,
        Err(err) => {
            eprintln!("sync worker: failed to list accounts on startup: {err}");
            Vec::new()
        }
    };

    for account in accounts {
        let slot = start_account_task(ctx.clone(), account, parent.clone());
        slots.insert(slot.0, slot.1);
    }

    WorkerHandle {
        ctx,
        parent,
        accounts: Mutex::new(slots),
    }
}

fn start_account_task(
    ctx: WorkerContext,
    account: Account,
    parent: CancellationToken,
) -> (AccountId, AccountSlot) {
    let cancel = parent.child_token();
    let refresh = Arc::new(Notify::new());
    let account_id = account.id;

    // Seed the state map so the UI shows a baseline immediately.
    let initial = ctx.state.update(account_id, |s| {
        s.phase = SyncPhase::Idle;
        s.next_sync_in_seconds = Some(ctx.config.interval_secs());
    });
    emit_status(&ctx.emit, &initial);

    let task = {
        let cancel = cancel.clone();
        let refresh = refresh.clone();
        tauri::async_runtime::spawn(account_loop(ctx, account, cancel, refresh))
    };

    (
        account_id,
        AccountSlot {
            cancel,
            refresh,
            _task: task,
        },
    )
}

async fn account_loop(
    ctx: WorkerContext,
    account: Account,
    cancel: CancellationToken,
    refresh: Arc<Notify>,
) {
    let mut suspended_for_reauth = false;
    loop {
        if cancel.is_cancelled() {
            return;
        }

        // Build the client fresh each cycle so token-source errors surface
        // as a cycle-level failure instead of permanently disabling polling.
        let client = match ctx.clients.build(&account) {
            Ok(c) => c,
            Err(err) => {
                let message = format!("client build: {err}");
                record_failure(&ctx, &account, &message);
                emit_activity_cycle_failed(&ctx, &account, "client_build", &err.to_string());
                wait_for_next(&ctx, account.id, &cancel, &refresh).await;
                continue;
            }
        };

        if suspended_for_reauth {
            // Sleep until cancelled or until the user explicitly nudges us
            // (post-reauth manual refresh).
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = refresh.notified() => {
                    suspended_for_reauth = false;
                    continue;
                }
            }
        }

        let report = run_one_cycle(&ctx, &client, &account).await;
        match &report.outcome {
            CycleOutcome::Unauthorized => {
                suspended_for_reauth = true;
                ctx.reauth.notify(&account);
                // Loop back to the suspend branch above.
                continue;
            }
            CycleOutcome::RateLimited { reset_in_seconds } => {
                // Honour the upstream reset hint if we have one; otherwise
                // fall back to the configured interval.
                let wait = reset_in_seconds
                    .map(Duration::from_secs)
                    .unwrap_or(ctx.config.interval());
                if !sleep_or_refresh(&cancel, &refresh, wait).await {
                    return;
                }
            }
            _ => {
                wait_for_next(&ctx, account.id, &cancel, &refresh).await;
            }
        }
    }
}

/// Sleep until the next interval boundary, watching for cancellation +
/// manual-refresh nudges. Updates the state map's `next_sync_in_seconds`
/// once at the start so the UI countdown is anchored.
async fn wait_for_next(
    ctx: &WorkerContext,
    account_id: AccountId,
    cancel: &CancellationToken,
    refresh: &Arc<Notify>,
) {
    let interval = ctx.config.interval();
    let next_state = ctx.state.update(account_id, |s| {
        s.next_sync_in_seconds = Some(seconds_floor(interval));
    });
    emit_status(&ctx.emit, &next_state);

    let _ = sleep_or_refresh(cancel, refresh, interval).await;
}

/// Sleep for `duration`, short-circuiting on either cancellation (returns
/// `false` so the caller knows to bail) or a `refresh.notify_one()`.
async fn sleep_or_refresh(
    cancel: &CancellationToken,
    refresh: &Arc<Notify>,
    duration: Duration,
) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = refresh.notified() => true,
        _ = sleep(duration) => true,
    }
}

fn emit_status(emit: &Arc<dyn EmitSink>, state: &AccountSyncState) {
    let payload = SyncStatusPayload::new(state.clone());
    emit.emit(
        SYNC_STATUS_EVENT,
        &serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null),
    );
}

fn record_failure(ctx: &WorkerContext, account: &Account, message: &str) {
    let state = ctx.state.update(account.id, |s| {
        s.phase = SyncPhase::Error;
        s.message = Some(short_error_message(message));
        s.next_sync_in_seconds = Some(ctx.config.interval_secs());
    });
    emit_status(&ctx.emit, &state);
    ctx.emit.emit(
        SYNC_ERROR_EVENT,
        &serde_json::to_value(SyncErrorPayload {
            account_id: account.id,
            message: short_error_message(message),
        })
        .unwrap_or(serde_json::Value::Null),
    );
}

fn short_error_message(raw: &str) -> String {
    const MAX: usize = 160;
    if raw.len() <= MAX {
        raw.to_string()
    } else {
        format!("{}…", &raw[..MAX])
    }
}

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
            s.next_sync_in_seconds = Some(ctx.config.interval_secs());
        });
        emit_status(&ctx.emit, &state);
        return SyncCycleReport {
            account_id: account.id,
            repos_visited: 0,
            prs_visited: 0,
            requests_made: 0,
            outcome: CycleOutcome::Skipped {
                reason: SkipReason::RateBudgetGuard { pct },
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
                s.next_sync_in_seconds = reset_in.or(Some(ctx.config.interval_secs()));
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
                    s.next_sync_in_seconds = reset_in.or(Some(ctx.config.interval_secs()));
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
            eprintln!("sync prune (account {}): {err}", account.id);
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

/// Wrap [`crate::triage::query::auto_archive_sweep`] in a transaction and
/// log the affected row count at INFO level (the project's `eprintln!` is
/// the current logging convention; structured `tracing` is on the M7
/// polish list). A failure inside the sweep is logged and swallowed: the
/// archive sweep is cosmetic and the next cycle retries.
fn run_auto_archive_sweep(db: &DbHandle) {
    let mut conn = match db.lock() {
        Ok(g) => g,
        Err(err) => {
            eprintln!("auto-archive sweep: db poisoned: {err}");
            return;
        }
    };
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(err) => {
            eprintln!("auto-archive sweep: begin tx: {err}");
            return;
        }
    };
    let archived = match crate::triage::query::auto_archive_sweep(&tx) {
        Ok(n) => n,
        Err(err) => {
            eprintln!("auto-archive sweep: update: {err}");
            return;
        }
    };
    if let Err(err) = tx.commit() {
        eprintln!("auto-archive sweep: commit: {err}");
        return;
    }
    eprintln!("auto-archive sweep complete: archived={archived}");
}

/// Wrap [`crate::triage::query::archive_retention_sweep`] in a transaction and
/// log the affected row count. Hard-deletes PRs whose every viewer relation
/// has been archived for more than 60 days; the FK cascade drops review
/// threads, comments, timeline events, and the relations themselves. A
/// failure inside the sweep is logged and swallowed - the sweep is
/// best-effort housekeeping and the next cycle retries.
fn run_archive_retention_sweep(db: &DbHandle) {
    let mut conn = match db.lock() {
        Ok(g) => g,
        Err(err) => {
            eprintln!("archive retention sweep: db poisoned: {err}");
            return;
        }
    };
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(err) => {
            eprintln!("archive retention sweep: begin tx: {err}");
            return;
        }
    };
    let deleted = match crate::triage::query::archive_retention_sweep(&tx) {
        Ok(n) => n,
        Err(err) => {
            eprintln!("archive retention sweep: delete: {err}");
            return;
        }
    };
    if let Err(err) = tx.commit() {
        eprintln!("archive retention sweep: commit: {err}");
        return;
    }
    if deleted > 0 {
        eprintln!("archive retention sweep complete: deleted={deleted}");
    }
}

fn count_prs_across_repos(db: &DbHandle, repos: &[RepoRow]) -> u32 {
    let conn = match db.lock() {
        Ok(g) => g,
        Err(_) => return 0,
    };
    let mut total: u32 = 0;
    for repo in repos {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pull_requests WHERE repo_id = ?1",
                params![repo.id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        total = total.saturating_add(count.max(0) as u32);
    }
    total
}

fn emit_activity_cycle_started(ctx: &WorkerContext, account: &Account) {
    let message = format!("Cycle started for {}", account.login);
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::CycleStarted,
            message,
        )
        .build(),
    );
}

fn emit_activity_phase_started(ctx: &WorkerContext, account: &Account, phase: SyncPhaseLabel) {
    let message = match phase {
        SyncPhaseLabel::Discovery => format!("Discovering for {}", account.login),
        SyncPhaseLabel::Enrichment => "Fetching pull request detail".to_string(),
        SyncPhaseLabel::Pruning => "Pruning stale relations".to_string(),
    };
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::PhaseStarted { phase },
            message,
        )
        .build(),
    );
}

fn emit_activity_phase_progress(
    ctx: &WorkerContext,
    account: &Account,
    phase: SyncPhaseLabel,
    current: u32,
    total: u32,
) {
    let label = phase.as_str();
    let message = if total > 0 {
        format!("{label} ({current}/{total})")
    } else {
        format!("{label} ({current})")
    };
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::PhaseProgress {
                phase,
                current,
                total,
            },
            message,
        )
        .build(),
    );
}

fn emit_activity_pr_fetched(
    ctx: &WorkerContext,
    account: &Account,
    owner: &str,
    name: &str,
    number: i64,
    url: &str,
) {
    let message = format!("Fetched detail for {owner}/{name}#{number}");
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::PrFetched {
                number,
                owner: owner.to_string(),
                name: name.to_string(),
                url: url.to_string(),
            },
            message,
        )
        .build(),
    );
}

fn emit_activity_pr_skipped_no_change(
    ctx: &WorkerContext,
    account: &Account,
    owner: &str,
    name: &str,
    number: i64,
    url: &str,
) {
    let message = format!("Skipped {owner}/{name}#{number} (no change)");
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::PrSkippedNoChange {
                number,
                owner: owner.to_string(),
                name: name.to_string(),
                url: url.to_string(),
            },
            message,
        )
        .build(),
    );
}

fn emit_activity_phase_completed(
    ctx: &WorkerContext,
    account: &Account,
    phase: SyncPhaseLabel,
    summary: impl Into<String>,
) {
    emit_activity_phase_completed_with_skips(ctx, account, phase, summary, 0);
}

fn emit_activity_phase_completed_with_skips(
    ctx: &WorkerContext,
    account: &Account,
    phase: SyncPhaseLabel,
    summary: impl Into<String>,
    cache_skips: u32,
) {
    let summary = summary.into();
    let message = format!("{} complete - {}", phase.as_str(), summary);
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::PhaseCompleted {
                phase,
                summary,
                cache_skips,
            },
            message,
        )
        .build(),
    );
}

fn emit_activity_cycle_completed(
    ctx: &WorkerContext,
    account: &Account,
    prs_visited: u32,
    summary: impl Into<String>,
) {
    let summary = summary.into();
    let message = format!("Cycle complete for {} - {}", account.login, summary);
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::CycleCompleted {
                prs_visited,
                summary,
            },
            message,
        )
        .build(),
    );
}

fn emit_activity_cycle_failed(
    ctx: &WorkerContext,
    account: &Account,
    error_kind: &str,
    error_message: &str,
) {
    let truncated = short_error_message(error_message);
    let message = format!("Cycle failed ({error_kind}): {truncated}");
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Error,
            Some(account.id),
            ActivityKind::CycleFailed {
                error_message: truncated,
                error_kind: error_kind.to_string(),
            },
            message,
        )
        .build(),
    );
}

fn emit_activity_rate_pause(
    ctx: &WorkerContext,
    account: &Account,
    reset_in: Option<Duration>,
    pct: u8,
) {
    let reset_in_seconds = reset_in.map(|d| d.as_secs()).unwrap_or(0);
    let message = if reset_in_seconds > 0 {
        format!(
            "Rate limit guard paused {} ({}% remaining, resets in {}s)",
            account.login, pct, reset_in_seconds
        )
    } else {
        format!(
            "Rate limit guard paused {} ({}% remaining)",
            account.login, pct
        )
    };
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Warn,
            Some(account.id),
            ActivityKind::RateLimitPause { reset_in_seconds },
            message,
        )
        .build(),
    );
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
        s.next_sync_in_seconds = Some(ctx.config.interval_secs());
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
    pct: u8,
    limit: i64,
    reset_in: Option<Duration>,
    resource: Option<RateResource>,
) {
    let payload = SyncRateLimitPayload {
        account_id: account.id,
        pct,
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

        // Pre-flight skip (issue #232): if discovery just wrote a
        // `pull_requests.updated_at` that matches the previous-cycle marker
        // for this PR, skip the GraphQL PR-detail round trip entirely. The
        // GraphQL endpoint doesn't honour `If-None-Match`, so this is how we
        // recover the "nothing changed" saving REST already gets from ETag
        // 304s. Timeline still runs (REST-conditional, ADR 0004) so the
        // latest-status-change derivation stays current.
        let pr_detail_marker = pr_detail_marker_key(pr.id);
        let skip_detail = client
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
            // Stamp the issue #232 marker so the next cycle's pre-flight
            // comparison sees the freshly-persisted `updated_at`. Falling back
            // to `pr.updated_at` keeps the marker aligned when GraphQL returns
            // a thin payload (no `updatedAt` field).
            let marker_for_next_cycle = fetched
                .as_ref()
                .and_then(|d| rfc3339_to_unix(&d.updated_at))
                .unwrap_or(pr.updated_at);
            client.cache_graphql_body(
                &pr_detail_marker,
                &pr_detail_marker_bytes(marker_for_next_cycle),
            );
            (fetched, body)
        };

        // Post-flight body-hash cache (ADR 0004, issue #234): only relevant
        // when we actually made the call. On a byte-identical detail body,
        // skip the detail-driven DB writes (the prior cycle's values are
        // still authoritative). Timeline still runs (REST ETag) so the
        // latest-status-change derivation picks up new events.
        let detail_for_write = if skip_detail {
            None
        } else {
            let detail_cache_key = format!("pr_detail:{}/{}#{}", repo.owner, repo.name, pr.number);
            let detail_cache_hit = client.graphql_body_unchanged(&detail_cache_key, &detail_body);
            if detail_cache_hit {
                *detail_cache_skips = detail_cache_skips.saturating_add(1);
                None
            } else {
                detail.as_ref()
            }
        };

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

/// Cache key for the previous-cycle `updated_at` marker, scoped per PR. The
/// helper hides the format so callers don't grow string-formatting copies.
fn pr_detail_marker_key(pr_id: i64) -> String {
    format!("pr-detail:{pr_id}")
}

/// Canonical bytes for the previous-cycle marker. Big-endian gives a stable
/// representation across hosts; we hash these bytes to compare against the
/// `body_sha256` slot the GraphQL cache stores.
fn pr_detail_marker_bytes(updated_at: i64) -> [u8; 8] {
    updated_at.to_be_bytes()
}

#[derive(Debug)]
pub struct RepoRow {
    pub id: i64,
    pub owner: String,
    pub name: String,
}

#[derive(Debug)]
pub struct PrRow {
    pub id: i64,
    pub number: i64,
    /// Mirror of `pull_requests.updated_at` (unix seconds) at the moment the
    /// enrichment loop reads the row. Compared against the previous-cycle
    /// `pr-detail:{pr_id}` marker (stored via `client.cache_graphql_body`) to
    /// skip the GraphQL PR-detail round trip when nothing upstream has moved
    /// (issue #232).
    pub updated_at: i64,
}

pub fn list_repos_for_account(
    db: &DbHandle,
    account_id: AccountId,
) -> Result<Vec<RepoRow>, rusqlite::Error> {
    let conn = crate::db::lock_db(db)?;
    let mut stmt = conn
        .prepare("SELECT id, owner, name FROM repos WHERE account_id = ?1 ORDER BY owner, name")?;
    let rows = stmt
        .query_map(params![account_id as i64], |row| {
            Ok(RepoRow {
                id: row.get(0)?,
                owner: row.get(1)?,
                name: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn list_prs_for_repo(db: &DbHandle, repo_id: i64) -> Result<Vec<PrRow>, rusqlite::Error> {
    let conn = crate::db::lock_db(db)?;
    let mut stmt =
        conn.prepare("SELECT id, number, updated_at FROM pull_requests WHERE repo_id = ?1")?;
    let rows = stmt
        .query_map(params![repo_id], |row| {
            Ok(PrRow {
                id: row.get(0)?,
                number: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

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
                (id, repo_id, number, title, state, draft, author_login,
                 created_at, updated_at, base_ref, head_ref,
                 mergeable, review_decision, additions, deletions, changed_files,
                 ci_state, ci_total, ci_passing)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                        ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                state = excluded.state,
                draft = excluded.draft,
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

/// Format any [`NotificationTrigger`] produced by the per-PR write path and
/// hand the result to the sink. Runs outside the enclosing write transaction
/// so a plugin call can't block the DB lock; the formatter takes a separate
/// short-lived `&Connection`. A formatter miss (PR row deleted in the gap
/// between commit and dispatch) is logged and skipped; the in-app badge keeps
/// surfacing attention.
fn dispatch_triggers(
    db: &DbHandle,
    sink: &NotificationSinkHandle,
    triggers: &[NotificationTrigger],
) {
    if triggers.is_empty() {
        return;
    }
    let formatted: Vec<_> = {
        let conn = match db.lock() {
            Ok(g) => g,
            Err(err) => {
                eprintln!("notify dispatch: db poisoned: {err}");
                return;
            }
        };
        triggers
            .iter()
            .map(|t| (t, format_trigger(&conn, t)))
            .collect()
    };
    for (trigger, notification) in formatted {
        match notification {
            Some(n) => {
                eprintln!(
                    "notify dispatch: kind={:?} account={} pr={}",
                    trigger.kind, trigger.account_id, trigger.pull_request_id,
                );
                sink.dispatch(&n);
            }
            None => eprintln!(
                "notify dispatch: skipping, PR row missing (account={}, pr={})",
                trigger.account_id, trigger.pull_request_id,
            ),
        }
    }
}

/// Count new `@<viewer-login>` mentions across the PR's comment bodies since
/// the per-(account, PR) watermark, bump the unread counter by that count,
/// advance the watermark to now, then recompute the four-signal
/// `needs_attention` composite. See ADR 0015 and `docs/contracts/triage-ux.md`.
///
/// Watermark advance runs unconditionally so a cycle with zero new comments
/// still moves the cursor forward and the next scan starts from now.
///
/// Host isolation (issue #169): GitHub logins are unique per host, not
/// globally. Two PRism accounts can share the same login on different hosts
/// (e.g. `ada` on github.com and `ada` on github.acme.corp) but they are
/// different identities. The scan + recompute therefore matches on the
/// viewer's `(login, host)` pair, derived from `accounts WHERE id = ?1` and
/// the PR's owning host from `repos -> accounts`. A relation row whose viewer
/// host differs from the PR's host is treated as a no-op so cross-host login
/// collisions never inflate counters or flip `needs_attention`.
fn scan_mentions_and_recompute_attention(
    tx: &rusqlite::Transaction<'_>,
    account_id: AccountId,
    pr_id: i64,
) -> Result<Vec<NotificationTrigger>, rusqlite::Error> {
    let account_id = account_id as i64;

    // Viewer (login, host). The relation row may not exist on this account
    // (Team-view path where the active account has no discovered relation to
    // the PR); in that case the UPDATE matches zero rows and the scan is a
    // clean no-op.
    let viewer: Option<(String, String)> = tx
        .query_row(
            "SELECT login, host FROM accounts WHERE id = ?1",
            params![account_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .ok();
    let Some((viewer_login, viewer_host)) = viewer else {
        return Ok(Vec::new());
    };

    // PR's owning host: the host of the account that owns the repo. Used to
    // confirm the viewer's identity lives on this PR's host before counting
    // mentions or matching the PR author / requested reviewer. A missing PR
    // row reads the same as "no relation" - clean no-op.
    let pr_owner_host: Option<String> = tx
        .query_row(
            "SELECT acc.host
               FROM pull_requests pr
               JOIN repos r ON r.id = pr.repo_id
               JOIN accounts acc ON acc.id = r.account_id
              WHERE pr.id = ?1",
            params![pr_id],
            |r| r.get::<_, String>(0),
        )
        .ok();
    let Some(pr_owner_host) = pr_owner_host else {
        return Ok(Vec::new());
    };
    if viewer_host != pr_owner_host {
        return Ok(Vec::new());
    }

    // Snapshot the row before the scan + recompute so we can spot the two
    // ADR 0017 transitions (0 -> 1 on `needs_attention`, strict increase on
    // `mentioned_count_unread`). The mention counter snapshot has to come
    // _before_ the UPDATE below bumps it; the attention snapshot can come
    // either side of that bump since the recompute UPDATE that follows is
    // the only thing that writes `needs_attention`.
    let before: Option<(i64, i64)> = tx
        .query_row(
            "SELECT needs_attention, mentioned_count_unread
               FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok();

    // Read the prior watermark. NULL or missing relation row reads as 0 so the
    // first cycle counts every comment newer than the epoch.
    let watermark: i64 = tx
        .query_row(
            "SELECT COALESCE(mention_scan_watermark_at, 0)
               FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0);

    // Pull bodies from review + issue comments newer than the watermark and
    // not authored by the viewer. Scan in Rust (word-boundary aware) rather
    // than via SQLite REGEXP so the worker doesn't need to register a custom
    // SQL function. Bodies are bounded by the per-PR comment volume on the
    // GitHub side; for v1 sizes a memory pass is cheap.
    let mut new_mentions: i64 = 0;
    {
        let mut review_stmt = tx.prepare(
            "SELECT c.body
               FROM review_comments c
               JOIN review_threads t ON t.id = c.review_thread_id
              WHERE t.pull_request_id = ?1
                AND c.author_login != ?2
                AND c.created_at > ?3",
        )?;
        let bodies = review_stmt.query_map(params![pr_id, viewer_login, watermark], |row| {
            row.get::<_, String>(0)
        })?;
        for body in bodies {
            if mentions_viewer(&body?, &viewer_login) {
                new_mentions += 1;
            }
        }
    }
    {
        let mut issue_stmt = tx.prepare(
            "SELECT ic.body
               FROM issue_comments ic
              WHERE ic.pull_request_id = ?1
                AND ic.author_login != ?2
                AND ic.created_at > ?3",
        )?;
        let bodies = issue_stmt.query_map(params![pr_id, viewer_login, watermark], |row| {
            row.get::<_, String>(0)
        })?;
        for body in bodies {
            if mentions_viewer(&body?, &viewer_login) {
                new_mentions += 1;
            }
        }
    }

    // Bump counter and advance watermark. Watermark moves forward on every
    // cycle (idempotency cursor) so re-runs without new comments stay flat.
    let now = unix_now();
    tx.execute(
        "UPDATE pull_request_viewer_relations
            SET mentioned_count_unread = mentioned_count_unread + ?1,
                mention_scan_watermark_at = ?2
          WHERE account_id = ?3 AND pull_request_id = ?4",
        params![new_mentions, now, account_id, pr_id],
    )?;

    // Composite recompute. Mirrors the formula in ADR 0015. Short-lived
    // duplication with `triage::query::recompute_needs_attention` (M4-A);
    // ADR 0015 calls out the intentional overlap.
    //
    // Identity match uses the viewer's `(login, host)` pair against the PR's
    // owning host. The early-exit above guarantees `viewer_host` equals the
    // PR's host, so the EXISTS subqueries only need to verify `pr.author_login
    // = ?3` and `rr.login = ?3` (login string equality) against PR rows on
    // the matching host - captured by the `pr_host_acc.host = ?4` join below.
    //
    // Signal #1 (`threads_unresolved_involved > 0` on an authored PR) reads
    // `review_threads` + `review_comments` directly per ADR 0016 - the
    // pre-aggregated column went away with the dashboard rollup. The
    // involvement test scopes by `a.id = ?1` (the active account), matching
    // the dashboard query's single-account semantics.
    tx.execute(
        "UPDATE pull_request_viewer_relations
            SET needs_attention = CASE WHEN (
                EXISTS (
                    SELECT 1 FROM pull_requests pr
                     JOIN repos r ON r.id = pr.repo_id
                     JOIN accounts pr_host_acc ON pr_host_acc.id = r.account_id
                     JOIN review_threads t ON t.pull_request_id = pr.id
                     WHERE pr.id = ?2
                       AND pr.author_login = ?3
                       AND pr_host_acc.host = ?4
                       AND t.is_resolved = 0
                       AND EXISTS (
                           SELECT 1 FROM review_comments c
                            JOIN accounts a ON a.login = c.author_login
                            WHERE c.review_thread_id = t.id
                              AND a.id = ?1
                       )
                )
                OR EXISTS (
                    SELECT 1 FROM requested_reviewers rr
                     JOIN pull_requests pr ON pr.id = rr.pull_request_id
                     JOIN repos r ON r.id = pr.repo_id
                     JOIN accounts pr_host_acc ON pr_host_acc.id = r.account_id
                     WHERE rr.pull_request_id = ?2
                       AND rr.login = ?3
                       AND pr_host_acc.host = ?4
                )
                OR (mentioned_count_unread > 0)
                OR EXISTS (
                    SELECT 1 FROM pull_requests pr
                     JOIN repos r ON r.id = pr.repo_id
                     JOIN accounts pr_host_acc ON pr_host_acc.id = r.account_id
                     WHERE pr.id = ?2
                       AND pr.author_login = ?3
                       AND pr_host_acc.host = ?4
                       AND pr.review_decision = 'CHANGES_REQUESTED'
                )
            ) THEN 1 ELSE 0 END
          WHERE account_id = ?1 AND pull_request_id = ?2",
        params![account_id, pr_id, viewer_login, viewer_host],
    )?;

    // Compare to the pre-write snapshot. A missing relation row before the
    // write means the recompute UPDATE matched zero rows; no trigger fires.
    let Some((before_attention, before_mentions)) = before else {
        return Ok(Vec::new());
    };
    let after: Option<(i64, i64)> = tx
        .query_row(
            "SELECT needs_attention, mentioned_count_unread
               FROM pull_request_viewer_relations
              WHERE account_id = ?1 AND pull_request_id = ?2",
            params![account_id, pr_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok();
    let Some((after_attention, after_mentions)) = after else {
        return Ok(Vec::new());
    };

    let mut triggers = Vec::new();
    if before_attention == 0 && after_attention == 1 {
        triggers.push(NotificationTrigger {
            account_id,
            pull_request_id: pr_id,
            kind: NotificationKind::NeedsAttention,
        });
    }
    if after_mentions > before_mentions {
        triggers.push(NotificationTrigger {
            account_id,
            pull_request_id: pr_id,
            kind: NotificationKind::Mention,
        });
    }
    Ok(triggers)
}

/// Count `@<viewer>` matches in `body`, treating a match as terminated by
/// whitespace, EOL, ASCII punctuation, or end-of-string. Case-insensitive
/// because GitHub logins normalise that way. Rejects subword extensions like
/// `@<viewer>-bot` or `@<viewer>123`. See ADR 0015 and the M4 contract for
/// the word-boundary spec.
///
/// Returns `true` if at least one match is found. Callers count comment rows
/// that match (so two mentions in the same comment count as one increment),
/// matching the contract's row-count semantics in `docs/contracts/triage-ux.md`.
fn mentions_viewer(body: &str, viewer_login: &str) -> bool {
    if viewer_login.is_empty() || body.is_empty() {
        return false;
    }
    let needle = viewer_login.to_lowercase();
    let body_lower = body.to_lowercase();
    let needle_bytes = needle.as_bytes();
    let body_bytes = body_lower.as_bytes();
    let nlen = needle_bytes.len();
    let blen = body_bytes.len();

    let mut cursor = 0;
    while cursor < blen {
        let Some(at_offset) = body_bytes[cursor..].iter().position(|&b| b == b'@') else {
            return false;
        };
        let login_start = cursor + at_offset + 1;
        let login_end = login_start + nlen;
        if login_end <= blen && &body_bytes[login_start..login_end] == needle_bytes {
            let trailing = body_bytes.get(login_end).copied();
            if is_mention_boundary(trailing) {
                return true;
            }
        }
        // Advance past this `@` regardless of match outcome to find the next.
        cursor = login_start;
    }
    false
}

/// Trailing-character predicate for the word-boundary spec. `None` means EOL.
/// Whitespace, common ASCII punctuation, and closing brackets all terminate a
/// mention; alphanumerics, hyphens, and underscores continue it (so
/// `@alice-bot` rejects when viewer is `alice`). Non-ASCII bytes fall through
/// as non-boundary to stay conservative against partial UTF-8 sequences.
fn is_mention_boundary(c: Option<u8>) -> bool {
    let Some(c) = c else {
        return true;
    };
    matches!(
        c,
        b' ' | b'\t'
            | b'\n'
            | b'\r'
            | b'.'
            | b','
            | b';'
            | b':'
            | b'!'
            | b'?'
            | b')'
            | b']'
            | b'}'
            | b'\''
            | b'"'
            | b'`'
            | b'/'
            | b'\\'
            | b'<'
            | b'>'
    )
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

/// Collect every `(login, avatar_url)` pair surfaced by this cycle's payload
/// and UPSERT them into `users`. Only entries with a populated `avatar_url`
/// are written: we never store NULLs, so a partial payload (e.g. an older
/// fixture or a comment-edit response that drops the avatar field) can't
/// blank a row a previous cycle populated.
///
/// Dedup happens via the SQL UPSERT itself; collecting into a HashMap first
/// would also work but every login on a typical PR (author + reviewers +
/// thread/issue comment heads + review submitters + timeline actors) hits a
/// small bound, so the cycle-time win isn't worth the extra allocation.
fn write_user_avatars(
    tx: &rusqlite::Transaction<'_>,
    detail: Option<&crate::github::graphql::PullRequestDetail>,
    events: Option<&[crate::sync::status_timeline::TimelineEvent]>,
) -> Result<(), rusqlite::Error> {
    use crate::github::graphql::RequestedReviewer;

    let now = unix_now();
    let upsert = |login: &str, avatar_url: &Option<String>| -> Result<(), rusqlite::Error> {
        let Some(url) = avatar_url.as_deref() else {
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
            params![login, url, now],
        )?;
        Ok(())
    };

    if let Some(d) = detail {
        if let Some(author) = d.author.as_ref() {
            upsert(&author.login, &author.avatar_url)?;
        }
        if let Some(rr) = d.review_requests.as_ref() {
            for entry in &rr.nodes {
                // Team reviewers have no avatar URL on the User branch; the
                // `Team` and `Other` variants skip cleanly.
                if let Some(RequestedReviewer::User { login, avatar_url }) =
                    entry.requested_reviewer.as_ref()
                {
                    upsert(login, avatar_url)?;
                }
            }
        }
        for thread in &d.review_threads.nodes {
            for comment in &thread.comments.nodes {
                if let Some(actor) = comment.author.as_ref() {
                    upsert(&actor.login, &actor.avatar_url)?;
                }
            }
        }
        if let Some(reviews) = d.reviews.as_ref() {
            for review in &reviews.nodes {
                if let Some(actor) = review.author.as_ref() {
                    upsert(&actor.login, &actor.avatar_url)?;
                }
            }
        }
    }

    if let Some(events) = events {
        for event in events {
            if let (Some(login), Some(_)) = (
                event.actor_login.as_deref(),
                event.actor_avatar_url.as_ref(),
            ) {
                upsert(login, &event.actor_avatar_url)?;
            }
        }
    }
    Ok(())
}

/// Upsert per-thread state. Tracks transitions on `is_resolved` so
/// `resolved_at` is set when a thread becomes resolved and cleared when it
/// flips back. Prunes any prior thread for this PR whose `node_id` is absent
/// from the fetched set; cascading deletes on `review_comments` follow.
fn write_review_threads(
    tx: &rusqlite::Transaction<'_>,
    pr_id: i64,
    threads: &[crate::github::graphql::ReviewThread],
) -> Result<(), rusqlite::Error> {
    use std::collections::HashMap;

    // Snapshot the existing rows so we can detect resolve transitions
    // (set `resolved_at` only on the cycle the flag flips) and preserve
    // `created_at` once it's stamped.
    let mut existing: HashMap<String, ExistingThread> = HashMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT node_id, is_resolved, resolved_at, created_at
               FROM review_threads
              WHERE pull_request_id = ?1 AND node_id IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![pr_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                ExistingThread {
                    is_resolved: r.get::<_, i64>(1)? != 0,
                    resolved_at: r.get::<_, Option<i64>>(2)?,
                    created_at: r.get::<_, Option<i64>>(3)?,
                },
            ))
        })?;
        for row in rows {
            let (node_id, info) = row?;
            existing.insert(node_id, info);
        }
    }

    for thread in threads {
        let head = thread.comments.nodes.first();
        let head_created_at = head.and_then(|c| rfc3339_to_unix(&c.created_at));
        let head_author = head.and_then(|c| c.author.as_ref().map(|a| a.login.as_str()));
        let head_body = head.map(|c| c.body_text.as_str());
        // `PullRequestReviewThread` has no `url` field on GitHub's GraphQL
        // schema (issue #115). The thread permalink is the head comment's
        // url; absent a head comment, leave the column NULL.
        let head_url = head.and_then(|c| c.url.as_deref());

        let prior = existing.remove(&thread.id);
        let created_at = prior
            .as_ref()
            .and_then(|p| p.created_at)
            .or(head_created_at);

        // Resolved-at follows the resolved flag transition: set on the cycle
        // it flips true, clear on the cycle it flips back. Preserve when the
        // state is unchanged.
        let resolved_at = match (prior.as_ref().map(|p| p.is_resolved), thread.is_resolved) {
            (Some(true), true) => prior.as_ref().and_then(|p| p.resolved_at),
            (Some(false), true) | (None, true) => Some(unix_now()),
            (_, false) => None,
        };

        // The reply count denormalises the post-head replies. `totalCount`
        // covers head + replies; one comment means zero replies.
        let reply_count = (thread.comments.total_count - 1).max(0);

        // The unique constraint on review_threads.node_id is a partial index
        // (WHERE node_id IS NOT NULL from migration 0004). SQLite requires the
        // ON CONFLICT target to repeat the WHERE clause for partial indexes.
        tx.execute(
            "INSERT INTO review_threads
                (pull_request_id, node_id, is_resolved, is_outdated, path,
                 line, start_line, original_line, created_at, resolved_at,
                 last_reply_at, reply_count, head_comment_author_login,
                 head_comment_body_text, head_comment_created_at, url)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(node_id) WHERE node_id IS NOT NULL DO UPDATE SET
                pull_request_id = excluded.pull_request_id,
                is_resolved = excluded.is_resolved,
                is_outdated = excluded.is_outdated,
                path = excluded.path,
                line = excluded.line,
                start_line = excluded.start_line,
                original_line = excluded.original_line,
                created_at = COALESCE(review_threads.created_at, excluded.created_at),
                resolved_at = excluded.resolved_at,
                last_reply_at = excluded.last_reply_at,
                reply_count = excluded.reply_count,
                head_comment_author_login = excluded.head_comment_author_login,
                head_comment_body_text = excluded.head_comment_body_text,
                head_comment_created_at = excluded.head_comment_created_at,
                url = COALESCE(excluded.url, review_threads.url)",
            params![
                pr_id,
                thread.id,
                thread.is_resolved as i64,
                thread.is_outdated as i64,
                thread.path,
                thread.line,
                thread.start_line,
                thread.original_line,
                created_at,
                resolved_at,
                head_created_at,
                reply_count,
                head_author,
                head_body,
                head_created_at,
                head_url,
            ],
        )?;
    }

    // Pruning: any thread row left in the snapshot wasn't present in the
    // latest fetch, so the thread has been removed on GitHub. Comments
    // cascade via the existing FK.
    for stale in existing.keys() {
        tx.execute(
            "DELETE FROM review_threads
              WHERE pull_request_id = ?1 AND node_id = ?2",
            params![pr_id, stale],
        )?;
    }

    Ok(())
}

#[derive(Debug)]
struct ExistingThread {
    is_resolved: bool,
    resolved_at: Option<i64>,
    created_at: Option<i64>,
}

/// Upsert submitted reviews and prune any prior row whose `node_id` is absent
/// from the fetched set.
fn write_reviews(
    tx: &rusqlite::Transaction<'_>,
    pr_id: i64,
    reviews: &[crate::github::graphql::PullRequestReviewNode],
) -> Result<(), rusqlite::Error> {
    use std::collections::HashSet;

    let mut existing: HashSet<String> = HashSet::new();
    {
        let mut stmt = tx.prepare(
            "SELECT node_id FROM reviews
              WHERE pull_request_id = ?1 AND node_id IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![pr_id], |r| r.get::<_, String>(0))?;
        for row in rows {
            existing.insert(row?);
        }
    }

    for review in reviews {
        let author = review
            .author
            .as_ref()
            .map(|a| a.login.as_str())
            .unwrap_or("");
        let submitted_at = review.submitted_at.as_deref().and_then(rfc3339_to_unix);

        // Same partial-index conflict target shape as review_threads.
        // `body_html` is COALESCEd so a payload that omits the field doesn't
        // blank a previously-populated row (ADR 0014, issue #138). The same
        // protection applies for `body` already today.
        tx.execute(
            "INSERT INTO reviews
                (pull_request_id, node_id, reviewer_login, state, submitted_at,
                 body, body_html, url)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(node_id) WHERE node_id IS NOT NULL DO UPDATE SET
                pull_request_id = excluded.pull_request_id,
                reviewer_login = excluded.reviewer_login,
                state = excluded.state,
                submitted_at = excluded.submitted_at,
                body = excluded.body,
                body_html = COALESCE(excluded.body_html, reviews.body_html),
                url = COALESCE(excluded.url, reviews.url)",
            params![
                pr_id,
                review.id,
                author,
                review.state,
                submitted_at,
                review.body,
                review.body_html,
                review.url,
            ],
        )?;

        existing.remove(&review.id);
    }

    // Pruning: any review row whose node_id wasn't in the latest fetch is
    // gone upstream; drop it locally.
    for stale in &existing {
        tx.execute(
            "DELETE FROM reviews
              WHERE pull_request_id = ?1 AND node_id = ?2",
            params![pr_id, stale],
        )?;
    }

    Ok(())
}

/// Persist the qualifying timeline events for a PR.
///
/// Wipe-and-rewrite per cycle: GitHub timelines are append-only on the server,
/// so the latest fetch is authoritative for the PR's history. The wipe handles
/// rare cases where GitHub itself surfaces a corrected event ordering (e.g. a
/// backfill after support intervention) and keeps the table consistent with the
/// derivation that runs alongside this call.
///
/// `payload` stores per-event JSON for fields not modelled as dedicated
/// columns. Today the only consumer is `reviewed` events, which persist
/// `{"state": "APPROVED" | "CHANGES_REQUESTED" | ...}` so the timeline tab can
/// render the right badge without parsing the event type plus an out-of-band
/// state column.
fn write_timeline_events(
    tx: &rusqlite::Transaction<'_>,
    pr_id: i64,
    events: &[crate::sync::status_timeline::TimelineEvent],
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "DELETE FROM timeline_events WHERE pull_request_id = ?1",
        params![pr_id],
    )?;
    for event in events {
        let payload = timeline_event_payload(event);
        tx.execute(
            "INSERT INTO timeline_events
                (pull_request_id, event_type, actor_login, created_at, payload)
                VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                pr_id,
                event.event,
                event.actor_login,
                event.created_at.unix_timestamp(),
                payload,
            ],
        )?;
    }
    Ok(())
}

/// Build the `payload` JSON column for one timeline event.
///
/// `reviewed` events carry the review state (`APPROVED`, `CHANGES_REQUESTED`,
/// `COMMENTED`, `DISMISSED`); all other qualifying events produce `{}` because
/// no auxiliary field exists for them today. Persisting a value rather than
/// NULL keeps the `payload` column's NOT NULL invariant in 0001_init.sql.
fn timeline_event_payload(event: &crate::sync::status_timeline::TimelineEvent) -> String {
    match event.review_state.as_deref() {
        Some(state) => serde_json::json!({ "state": state }).to_string(),
        None => "{}".to_string(),
    }
}

fn qualifying_event_wire_name(ev: crate::sync::status_timeline::QualifyingEvent) -> &'static str {
    use crate::sync::status_timeline::QualifyingEvent::*;
    match ev {
        ReadyForReview => "ready_for_review",
        ConvertToDraft => "convert_to_draft",
        ReviewRequested => "review_requested",
        Reviewed => "reviewed",
        Merged => "merged",
        Closed => "closed",
        Reopened => "reopened",
    }
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

    #[test]
    fn rate_remaining_pct_handles_unobserved_budget() {
        assert_eq!(rate_remaining_pct(-1, -1), None);
        assert_eq!(rate_remaining_pct(0, 0), None);
    }

    #[test]
    fn rate_remaining_pct_computes_correct_percentage() {
        assert_eq!(rate_remaining_pct(5000, 5000), Some(100));
        assert_eq!(rate_remaining_pct(999, 5000), Some(19));
        assert_eq!(rate_remaining_pct(1000, 5000), Some(20));
        assert_eq!(rate_remaining_pct(0, 5000), Some(0));
    }

    fn snap(limit: i64, remaining: i64) -> ResourceSnapshot {
        ResourceSnapshot {
            limit,
            remaining,
            used: (limit - remaining).max(0),
            reset_at: std::time::UNIX_EPOCH,
        }
    }

    #[test]
    fn under_guard_fires_below_threshold() {
        // 19% < 20% guard - fires.
        assert!(under_guard(snap(5000, 999), 20));
        // 20% == guard - does not fire (threshold is "below").
        assert!(!under_guard(snap(5000, 1000), 20));
        // Unobserved - no skip.
        assert!(!under_guard(snap(-1, -1), 20));
    }

    #[test]
    fn under_guard_keys_against_active_resource_bucket() {
        // The worker passes the snapshot for whichever resource the next
        // call will hit. A tight search bucket must trip the guard even if
        // core / graphql are still healthy - and vice versa.
        use crate::github::rate_limit::RateBudget;
        use http::{HeaderMap, HeaderName, HeaderValue};

        let b = RateBudget::new();
        let mut h = HeaderMap::new();
        h.insert(
            HeaderName::from_static("x-ratelimit-resource"),
            HeaderValue::from_static("search"),
        );
        h.insert(
            HeaderName::from_static("x-ratelimit-limit"),
            HeaderValue::from_static("30"),
        );
        h.insert(
            HeaderName::from_static("x-ratelimit-remaining"),
            HeaderValue::from_static("5"),
        );
        h.insert(
            HeaderName::from_static("x-ratelimit-used"),
            HeaderValue::from_static("25"),
        );
        h.insert(
            HeaderName::from_static("x-ratelimit-reset"),
            HeaderValue::from_static("9999999999"),
        );
        b.update_from_headers(&h);

        let mut h2 = HeaderMap::new();
        h2.insert(
            HeaderName::from_static("x-ratelimit-resource"),
            HeaderValue::from_static("core"),
        );
        h2.insert(
            HeaderName::from_static("x-ratelimit-limit"),
            HeaderValue::from_static("5000"),
        );
        h2.insert(
            HeaderName::from_static("x-ratelimit-remaining"),
            HeaderValue::from_static("4900"),
        );
        h2.insert(
            HeaderName::from_static("x-ratelimit-used"),
            HeaderValue::from_static("100"),
        );
        h2.insert(
            HeaderName::from_static("x-ratelimit-reset"),
            HeaderValue::from_static("9999999999"),
        );
        b.update_from_headers(&h2);

        let snapshot = b.snapshot();
        // search is at ~17% remaining; trips.
        assert!(under_guard(
            snapshot.for_bucket(RateResource::Search),
            RATE_BUDGET_GUARD_PCT,
        ));
        // core is at 98%; clean.
        assert!(!under_guard(
            snapshot.for_bucket(RateResource::Core),
            RATE_BUDGET_GUARD_PCT,
        ));
        // graphql is unobserved; clean (no skip on unknown).
        assert!(!under_guard(
            snapshot.for_bucket(RateResource::Graphql),
            RATE_BUDGET_GUARD_PCT,
        ));
    }

    #[test]
    fn short_error_message_truncates_long_input() {
        let long = "x".repeat(500);
        let got = short_error_message(&long);
        assert!(got.len() <= 165, "got len {}", got.len());
        assert!(got.ends_with('…'));
    }

    #[test]
    fn short_error_message_passes_short_input_through() {
        let got = short_error_message("read repos: locked");
        assert_eq!(got, "read repos: locked");
    }

    #[test]
    fn rfc3339_to_unix_round_trips_a_known_value() {
        // 2026-01-01T00:00:00Z → 1767225600
        let secs = rfc3339_to_unix("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(secs, 1_767_225_600);
    }

    // ===== write_pr_updates persistence tests =====
    //
    // Each test stands up an in-memory SQLite DB at the latest migration,
    // seeds an account + repo + placeholder PR row, then calls
    // `write_pr_updates` with a hand-rolled `PullRequestDetail`.

    use crate::github::graphql::{
        Actor, PrCommit, PrCommitConnection, PrCommitNode, PullRequestDetail, RequestedReviewer,
        ReviewRequest, ReviewRequestConnection, ReviewThreadConnection, StatusCheckContext,
        StatusCheckContexts, StatusCheckRollup,
    };
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    /// Poison the DB mutex by panicking inside a thread that holds the lock,
    /// then assert the worker helpers surface a `rusqlite::Error` instead of
    /// propagating the panic up the cycle. The cycle's caller already converts
    /// these into `CycleOutcome::Failed` so the worker loop continues on the
    /// next interval (issue #238).
    #[test]
    fn list_repos_returns_error_when_mutex_poisoned() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        let db: DbHandle = Arc::new(Mutex::new(conn));

        let db_clone = db.clone();
        let handle = std::thread::spawn(move || {
            let _guard = db_clone.lock().expect("acquire lock");
            panic!("poison the mutex");
        });
        let _ = handle.join();
        assert!(
            db.lock().is_err(),
            "background panic should have poisoned the mutex"
        );

        let result = list_repos_for_account(&db, 1);
        assert!(
            result.is_err(),
            "list_repos_for_account should surface the poison as an error"
        );
    }

    #[test]
    fn list_prs_returns_error_when_mutex_poisoned() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        let db: DbHandle = Arc::new(Mutex::new(conn));

        let db_clone = db.clone();
        let _ = std::thread::spawn(move || {
            let _guard = db_clone.lock().expect("acquire lock");
            panic!("poison the mutex");
        })
        .join();

        let result = list_prs_for_repo(&db, 1);
        assert!(result.is_err());
    }

    fn empty_review_threads() -> ReviewThreadConnection {
        ReviewThreadConnection {
            page_info: crate::github::graphql::PageInfo {
                has_next_page: false,
                end_cursor: None,
            },
            nodes: vec![],
        }
    }

    fn seed_db_with_pr() -> (DbHandle, i64, i64) {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 42, 'placeholder', 'open', 0, '', 0, 0, 'main', 'feat');",
        )
        .unwrap();
        (Arc::new(Mutex::new(conn)), 10, 100)
    }

    fn detail_with(
        additions: Option<i64>,
        deletions: Option<i64>,
        changed_files: Option<i64>,
        mergeable: &str,
        review_decision: Option<&str>,
        review_requests: Option<ReviewRequestConnection>,
        commits: Option<PrCommitConnection>,
    ) -> PullRequestDetail {
        PullRequestDetail {
            id: "PR_test".into(),
            number: 42,
            title: "Add a thing".into(),
            is_draft: false,
            state: "OPEN".into(),
            merged: false,
            mergeable: mergeable.into(),
            url: "https://github.com/owner/repo/pull/42".into(),
            created_at: "2026-05-18T10:00:00Z".into(),
            updated_at: "2026-05-19T11:00:00Z".into(),
            author: Some(Actor::new("alice")),
            base_ref_name: "main".into(),
            head_ref_name: "feat/thing".into(),
            review_decision: review_decision.map(str::to_string),
            additions,
            deletions,
            changed_files,
            review_requests,
            commits,
            review_threads: empty_review_threads(),
            reviews: None,
            issue_comments: None,
        }
    }

    fn rollup_with(state: &str, total: i64, nodes: Vec<StatusCheckContext>) -> PrCommitConnection {
        PrCommitConnection {
            nodes: vec![PrCommitNode {
                commit: PrCommit {
                    status_check_rollup: Some(StatusCheckRollup {
                        state: state.into(),
                        contexts: StatusCheckContexts {
                            total_count: total,
                            nodes,
                        },
                    }),
                },
            }],
        }
    }

    #[test]
    fn write_pr_updates_persists_every_new_column() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let detail = detail_with(
            Some(120),
            Some(30),
            Some(5),
            "MERGEABLE",
            Some("APPROVED"),
            None,
            Some(rollup_with(
                "SUCCESS",
                3,
                vec![
                    StatusCheckContext::CheckRun {
                        conclusion: Some("SUCCESS".into()),
                        status: Some("COMPLETED".into()),
                    },
                    StatusCheckContext::CheckRun {
                        conclusion: Some("SUCCESS".into()),
                        status: Some("COMPLETED".into()),
                    },
                    StatusCheckContext::StatusContext {
                        state: "SUCCESS".into(),
                    },
                ],
            )),
        );

        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

        let conn = db.lock().unwrap();
        type Row = (
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<String>,
            Option<i64>,
            Option<i64>,
        );
        let row: Row = conn
            .query_row(
                "SELECT mergeable, review_decision, additions, deletions, changed_files,
                        ci_state, ci_total, ci_passing
                   FROM pull_requests WHERE id = ?1",
                params![pr_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("MERGEABLE"));
        assert_eq!(row.1.as_deref(), Some("APPROVED"));
        assert_eq!(row.2, Some(120));
        assert_eq!(row.3, Some(30));
        assert_eq!(row.4, Some(5));
        assert_eq!(row.5.as_deref(), Some("SUCCESS"));
        assert_eq!(row.6, Some(3));
        assert_eq!(row.7, Some(3));
    }

    #[test]
    fn write_pr_updates_replaces_requested_reviewers() {
        let (db, repo_id, pr_id) = seed_db_with_pr();

        // Seed an existing reviewer that should be replaced.
        db.lock()
            .unwrap()
            .execute(
                "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                    VALUES (?1, 'stale-user', 'user')",
                params![pr_id],
            )
            .unwrap();

        let detail = detail_with(
            None,
            None,
            None,
            "UNKNOWN",
            None,
            Some(ReviewRequestConnection {
                nodes: vec![
                    ReviewRequest {
                        requested_reviewer: Some(RequestedReviewer::User {
                            login: "dave".into(),
                            avatar_url: Some("https://avatars/dave".into()),
                        }),
                    },
                    ReviewRequest {
                        requested_reviewer: Some(RequestedReviewer::Team {
                            slug: "platform".into(),
                        }),
                    },
                    // A null reviewer (deleted account) must be silently
                    // dropped, not persisted as an empty-string row.
                    ReviewRequest {
                        requested_reviewer: None,
                    },
                ],
            }),
            None,
        );

        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

        let conn = db.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT login, reviewer_type FROM requested_reviewers
                  WHERE pull_request_id = ?1 ORDER BY reviewer_type, login",
            )
            .unwrap();
        let rows: Vec<(String, String)> = stmt
            .query_map(params![pr_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            rows,
            vec![
                ("platform".to_string(), "team".to_string()),
                ("dave".to_string(), "user".to_string()),
            ],
            "delete-then-insert: stale-user is gone, dave + platform are present"
        );
    }

    #[test]
    fn write_pr_updates_clears_requested_reviewers_when_empty() {
        let (db, repo_id, pr_id) = seed_db_with_pr();

        db.lock()
            .unwrap()
            .execute(
                "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                    VALUES (?1, 'stale-user', 'user')",
                params![pr_id],
            )
            .unwrap();

        // Empty `nodes` array — upstream returned the field, but no reviewers.
        let detail = detail_with(
            None,
            None,
            None,
            "UNKNOWN",
            None,
            Some(ReviewRequestConnection { nodes: vec![] }),
            None,
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

        let count: i64 = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM requested_reviewers WHERE pull_request_id = ?1",
                params![pr_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn write_pr_updates_skips_requested_reviewers_when_absent() {
        let (db, repo_id, pr_id) = seed_db_with_pr();

        db.lock()
            .unwrap()
            .execute(
                "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                    VALUES (?1, 'keeper', 'user')",
                params![pr_id],
            )
            .unwrap();

        // `review_requests` absent from the response (None) — leave existing
        // cache untouched so a partial detail doesn't drop the set.
        let detail = detail_with(None, None, None, "UNKNOWN", None, None, None);
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

        let count: i64 = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM requested_reviewers WHERE pull_request_id = ?1",
                params![pr_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn compute_ci_rollup_tallies_mixed_contexts_and_in_progress() {
        // 4 contexts: 1 SUCCESS CheckRun, 1 in-progress CheckRun (null
        // conclusion), 1 SUCCESS StatusContext, 1 FAILURE StatusContext.
        // Expected: state PENDING (rollup-provided), total 4, passing 2.
        let detail = detail_with(
            None,
            None,
            None,
            "UNKNOWN",
            None,
            None,
            Some(rollup_with(
                "PENDING",
                4,
                vec![
                    StatusCheckContext::CheckRun {
                        conclusion: Some("SUCCESS".into()),
                        status: Some("COMPLETED".into()),
                    },
                    StatusCheckContext::CheckRun {
                        conclusion: None,
                        status: Some("IN_PROGRESS".into()),
                    },
                    StatusCheckContext::StatusContext {
                        state: "SUCCESS".into(),
                    },
                    StatusCheckContext::StatusContext {
                        state: "FAILURE".into(),
                    },
                ],
            )),
        );

        let ci = compute_ci_rollup(&detail);
        assert_eq!(ci.state.as_deref(), Some("PENDING"));
        assert_eq!(ci.total, Some(4));
        assert_eq!(ci.passing, Some(2));
    }

    #[test]
    fn compute_ci_rollup_returns_none_when_rollup_absent() {
        // No commits at all.
        let no_commits = detail_with(None, None, None, "UNKNOWN", None, None, None);
        let ci = compute_ci_rollup(&no_commits);
        assert_eq!(
            ci,
            CiRollup {
                state: None,
                total: None,
                passing: None,
            }
        );

        // Commit present but no rollup attached.
        let no_rollup = detail_with(
            None,
            None,
            None,
            "UNKNOWN",
            None,
            None,
            Some(PrCommitConnection {
                nodes: vec![PrCommitNode {
                    commit: PrCommit {
                        status_check_rollup: None,
                    },
                }],
            }),
        );
        let ci = compute_ci_rollup(&no_rollup);
        assert_eq!(
            ci,
            CiRollup {
                state: None,
                total: None,
                passing: None,
            }
        );
    }

    #[test]
    fn write_pr_updates_persists_ci_rollup_with_in_progress_run() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let detail = detail_with(
            None,
            None,
            None,
            "MERGEABLE",
            None,
            None,
            Some(rollup_with(
                "PENDING",
                3,
                vec![
                    StatusCheckContext::CheckRun {
                        conclusion: Some("SUCCESS".into()),
                        status: Some("COMPLETED".into()),
                    },
                    StatusCheckContext::CheckRun {
                        conclusion: None,
                        status: Some("IN_PROGRESS".into()),
                    },
                    StatusCheckContext::StatusContext {
                        state: "SUCCESS".into(),
                    },
                ],
            )),
        );

        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

        let (state, total, passing): (Option<String>, Option<i64>, Option<i64>) = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT ci_state, ci_total, ci_passing FROM pull_requests WHERE id = ?1",
                params![pr_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(state.as_deref(), Some("PENDING"));
        assert_eq!(total, Some(3));
        assert_eq!(passing, Some(2));
    }

    #[test]
    fn write_pr_updates_skips_unknown_reviewer_typenames() {
        let (db, repo_id, pr_id) = seed_db_with_pr();

        let detail = detail_with(
            None,
            None,
            None,
            "UNKNOWN",
            None,
            Some(ReviewRequestConnection {
                nodes: vec![
                    ReviewRequest {
                        requested_reviewer: Some(RequestedReviewer::Other),
                    },
                    ReviewRequest {
                        requested_reviewer: Some(RequestedReviewer::User {
                            login: "alice".into(),
                            avatar_url: None,
                        }),
                    },
                ],
            }),
            None,
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

        let logins: Vec<String> = {
            let conn = db.lock().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT login FROM requested_reviewers
                      WHERE pull_request_id = ?1 ORDER BY login",
                )
                .unwrap();
            stmt.query_map(params![pr_id], |r| r.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(logins, vec!["alice".to_string()]);
    }

    // ===== Conversation depth (M3-A) tests =====

    use crate::github::graphql::{
        CommentConnection as GqlCommentConnection, IssueCommentConnection, PageInfo,
        PullRequestReviewConnection, PullRequestReviewNode, ReviewThread,
    };

    struct ThreadSpec<'a> {
        node_id: &'a str,
        is_resolved: bool,
        is_outdated: bool,
        path: &'a str,
        line: Option<i64>,
        start_line: Option<i64>,
        original_line: Option<i64>,
        head: Option<(&'a str, &'a str, &'a str)>,
        total_count: i64,
        /// Head comment's `url`. The thread permalink is derived from this at
        /// write time (issue #115).
        head_url: Option<&'a str>,
    }

    impl<'a> ThreadSpec<'a> {
        fn open(node_id: &'a str, path: &'a str, head: (&'a str, &'a str, &'a str)) -> Self {
            Self {
                node_id,
                is_resolved: false,
                is_outdated: false,
                path,
                line: None,
                start_line: None,
                original_line: None,
                head: Some(head),
                total_count: 1,
                head_url: None,
            }
        }

        fn resolved(mut self, resolved: bool) -> Self {
            self.is_resolved = resolved;
            self
        }

        fn outdated(mut self, outdated: bool) -> Self {
            self.is_outdated = outdated;
            self
        }

        fn lines(mut self, line: Option<i64>, start: Option<i64>, original: Option<i64>) -> Self {
            self.line = line;
            self.start_line = start;
            self.original_line = original;
            self
        }

        fn total_count(mut self, count: i64) -> Self {
            self.total_count = count;
            self
        }

        fn head_url(mut self, url: &'a str) -> Self {
            self.head_url = Some(url);
            self
        }
    }

    fn thread(spec: ThreadSpec<'_>) -> ReviewThread {
        let head_url = spec.head_url.map(str::to_string);
        let head_node = spec
            .head
            .map(|(id, login, created_at)| crate::github::graphql::Comment {
                id: id.into(),
                url: head_url,
                author: Some(Actor::new(login)),
                body_text: "head body".into(),
                created_at: created_at.into(),
            });
        ReviewThread {
            id: spec.node_id.into(),
            is_resolved: spec.is_resolved,
            is_outdated: spec.is_outdated,
            path: Some(spec.path.into()),
            line: spec.line,
            start_line: spec.start_line,
            original_line: spec.original_line,
            comments: GqlCommentConnection {
                total_count: spec.total_count,
                nodes: head_node.into_iter().collect(),
            },
        }
    }

    fn empty_thread(node_id: &str, path: &str) -> ReviewThread {
        ReviewThread {
            id: node_id.into(),
            is_resolved: false,
            is_outdated: false,
            path: Some(path.into()),
            line: None,
            start_line: None,
            original_line: None,
            comments: GqlCommentConnection {
                total_count: 0,
                nodes: vec![],
            },
        }
    }

    fn review_threads(nodes: Vec<ReviewThread>) -> ReviewThreadConnection {
        ReviewThreadConnection {
            page_info: PageInfo {
                has_next_page: false,
                end_cursor: None,
            },
            nodes,
        }
    }

    fn detail_with_threads(
        threads: ReviewThreadConnection,
        reviews: Option<PullRequestReviewConnection>,
        issue_comments: Option<IssueCommentConnection>,
    ) -> PullRequestDetail {
        let mut d = detail_with(None, None, None, "MERGEABLE", None, None, None);
        d.review_threads = threads;
        d.reviews = reviews;
        d.issue_comments = issue_comments;
        d
    }

    #[test]
    fn write_pr_updates_upserts_review_threads_with_line_range_and_head_snapshot() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let detail = detail_with_threads(
            review_threads(vec![thread(
                ThreadSpec::open(
                    "PRRT_1",
                    "src/lib.rs",
                    ("PRRC_1", "alice", "2026-05-18T10:00:00Z"),
                )
                .lines(Some(42), Some(40), Some(41))
                .total_count(3),
            )]),
            None,
            None,
        );

        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

        let conn = db.lock().unwrap();
        type Row = (
            String,
            i64,
            i64,
            Option<String>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            i64,
            Option<String>,
            Option<String>,
            Option<i64>,
        );
        let row: Row = conn
            .query_row(
                "SELECT node_id, is_resolved, is_outdated, path, line, start_line,
                        original_line, created_at, resolved_at, last_reply_at, reply_count,
                        head_comment_author_login, head_comment_body_text, head_comment_created_at
                   FROM review_threads
                  WHERE pull_request_id = ?1 AND node_id = 'PRRT_1'",
                params![pr_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                        r.get(11)?,
                        r.get(12)?,
                        r.get(13)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(row.0, "PRRT_1");
        assert_eq!(row.1, 0); // is_resolved
        assert_eq!(row.2, 0); // is_outdated
        assert_eq!(row.3.as_deref(), Some("src/lib.rs"));
        assert_eq!(row.4, Some(42));
        assert_eq!(row.5, Some(40));
        assert_eq!(row.6, Some(41));
        // created_at + last_reply_at derived from the head comment's createdAt.
        assert_eq!(row.7, rfc3339_to_unix("2026-05-18T10:00:00Z"));
        assert_eq!(row.8, None); // resolved_at — unresolved on first write.
        assert_eq!(row.9, rfc3339_to_unix("2026-05-18T10:00:00Z"));
        assert_eq!(row.10, 2); // reply_count = totalCount(3) - 1
        assert_eq!(row.11.as_deref(), Some("alice"));
        assert_eq!(row.12.as_deref(), Some("head body"));
        assert_eq!(row.13, rfc3339_to_unix("2026-05-18T10:00:00Z"));
    }

    #[test]
    fn write_pr_updates_persists_review_thread_url_from_head_comment() {
        // Issue #115: `PullRequestReviewThread` has no `url` field on GitHub's
        // GraphQL schema, so the worker derives `review_threads.url` from the
        // head comment's `url` at write time. Confirm the derivation happens
        // on first insert and that a later payload with no head url leaves
        // the previously-persisted value intact (`COALESCE` in the upsert).
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let detail = detail_with_threads(
            review_threads(vec![thread(
                ThreadSpec::open(
                    "PRRT_URL",
                    "src/lib.rs",
                    ("PRRC_U1", "alice", "2026-05-18T10:00:00Z"),
                )
                .head_url("https://github.com/owner/repo/pull/1#discussion_r42"),
            )]),
            None,
            None,
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();
        let url: Option<String> = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT url FROM review_threads WHERE node_id = 'PRRT_URL'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/owner/repo/pull/1#discussion_r42")
        );

        // Cycle 2: same thread, head comment url absent. The COALESCE in the
        // upsert keeps the previously-persisted url rather than blanking it.
        let detail2 = detail_with_threads(
            review_threads(vec![thread(ThreadSpec::open(
                "PRRT_URL",
                "src/lib.rs",
                ("PRRC_U1", "alice", "2026-05-18T10:00:00Z"),
            ))]),
            None,
            None,
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail2), None).unwrap();
        let url_after: Option<String> = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT url FROM review_threads WHERE node_id = 'PRRT_URL'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            url_after.as_deref(),
            Some("https://github.com/owner/repo/pull/1#discussion_r42"),
            "thread url must survive a payload with no head-comment url"
        );
    }

    #[test]
    fn write_pr_updates_thread_url_stays_null_without_head_comment() {
        // Defensive: a thread that arrives with no head comment leaves
        // `review_threads.url` NULL rather than blowing up.
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let detail = detail_with_threads(
            review_threads(vec![empty_thread("PRRT_empty_url", "x.rs")]),
            None,
            None,
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();
        let url: Option<String> = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT url FROM review_threads WHERE node_id = 'PRRT_empty_url'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(url.is_none());
    }

    #[test]
    fn write_pr_updates_tracks_resolved_at_transitions() {
        let (db, repo_id, pr_id) = seed_db_with_pr();

        // Cycle 1: unresolved.
        let d1 = detail_with_threads(
            review_threads(vec![thread(
                ThreadSpec::open(
                    "PRRT_1",
                    "src/lib.rs",
                    ("PRRC_1", "alice", "2026-05-18T10:00:00Z"),
                )
                .lines(Some(1), None, None),
            )]),
            None,
            None,
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&d1), None).unwrap();
        let resolved_at: Option<i64> = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT resolved_at FROM review_threads WHERE node_id = 'PRRT_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolved_at, None);

        // Cycle 2: resolved. resolved_at must be set.
        let d2 = detail_with_threads(
            review_threads(vec![thread(
                ThreadSpec::open(
                    "PRRT_1",
                    "src/lib.rs",
                    ("PRRC_1", "alice", "2026-05-18T10:00:00Z"),
                )
                .lines(Some(1), None, None)
                .resolved(true),
            )]),
            None,
            None,
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&d2), None).unwrap();
        let resolved_at: Option<i64> = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT resolved_at FROM review_threads WHERE node_id = 'PRRT_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            resolved_at.is_some(),
            "resolved_at must be stamped on transition to resolved"
        );
        let stamped = resolved_at.unwrap();

        // Cycle 3: still resolved. resolved_at preserved (not bumped).
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&d2), None).unwrap();
        let resolved_at: Option<i64> = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT resolved_at FROM review_threads WHERE node_id = 'PRRT_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            resolved_at,
            Some(stamped),
            "resolved_at must be preserved when state is unchanged"
        );

        // Cycle 4: thread flips back to unresolved. resolved_at must clear.
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&d1), None).unwrap();
        let resolved_at: Option<i64> = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT resolved_at FROM review_threads WHERE node_id = 'PRRT_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolved_at, None);

        // Cycle 5: thread becomes outdated (still unresolved). Outdated flag
        // recorded, resolved_at remains null.
        let d3 = detail_with_threads(
            review_threads(vec![thread(
                ThreadSpec::open(
                    "PRRT_1",
                    "src/lib.rs",
                    ("PRRC_1", "alice", "2026-05-18T10:00:00Z"),
                )
                .lines(Some(1), None, None)
                .outdated(true),
            )]),
            None,
            None,
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&d3), None).unwrap();
        let (is_outdated, resolved_at): (i64, Option<i64>) = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT is_outdated, resolved_at FROM review_threads WHERE node_id = 'PRRT_1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(is_outdated, 1);
        assert_eq!(resolved_at, None);
    }

    #[test]
    fn write_pr_updates_prunes_removed_threads_and_reviews() {
        let (db, repo_id, pr_id) = seed_db_with_pr();

        // Cycle 1: two threads + two reviews persisted.
        let d1 = detail_with_threads(
            review_threads(vec![
                thread(
                    ThreadSpec::open(
                        "PRRT_keep",
                        "a.rs",
                        ("PRRC_a", "alice", "2026-05-18T10:00:00Z"),
                    )
                    .lines(Some(1), None, None),
                ),
                thread(
                    ThreadSpec::open(
                        "PRRT_drop",
                        "b.rs",
                        ("PRRC_b", "bob", "2026-05-18T11:00:00Z"),
                    )
                    .lines(Some(2), None, None),
                ),
            ]),
            Some(PullRequestReviewConnection {
                nodes: vec![
                    PullRequestReviewNode {
                        id: "PRR_keep".into(),
                        state: "APPROVED".into(),
                        body: Some("LGTM".into()),
                        body_html: None,
                        submitted_at: Some("2026-05-18T12:00:00Z".into()),
                        url: None,
                        author: Some(Actor::new("alice")),
                    },
                    PullRequestReviewNode {
                        id: "PRR_drop".into(),
                        state: "COMMENTED".into(),
                        body: None,
                        body_html: None,
                        submitted_at: Some("2026-05-18T13:00:00Z".into()),
                        url: None,
                        author: Some(Actor::new("bob")),
                    },
                ],
            }),
            None,
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&d1), None).unwrap();

        let thread_count: i64 = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM review_threads WHERE pull_request_id = ?1",
                params![pr_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(thread_count, 2);
        let review_count: i64 = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM reviews WHERE pull_request_id = ?1",
                params![pr_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(review_count, 2);

        // Cycle 2: only the "keep" thread + review remain upstream.
        let d2 = detail_with_threads(
            review_threads(vec![thread(
                ThreadSpec::open(
                    "PRRT_keep",
                    "a.rs",
                    ("PRRC_a", "alice", "2026-05-18T10:00:00Z"),
                )
                .lines(Some(1), None, None),
            )]),
            Some(PullRequestReviewConnection {
                nodes: vec![PullRequestReviewNode {
                    id: "PRR_keep".into(),
                    state: "APPROVED".into(),
                    body: Some("LGTM".into()),
                    body_html: None,
                    submitted_at: Some("2026-05-18T12:00:00Z".into()),
                    url: None,
                    author: Some(Actor::new("alice")),
                }],
            }),
            None,
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&d2), None).unwrap();

        let surviving_threads: Vec<String> = {
            let conn = db.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT node_id FROM review_threads WHERE pull_request_id = ?1")
                .unwrap();
            stmt.query_map(params![pr_id], |r| r.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(surviving_threads, vec!["PRRT_keep".to_string()]);

        let surviving_reviews: Vec<String> = {
            let conn = db.lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT node_id FROM reviews WHERE pull_request_id = ?1")
                .unwrap();
            stmt.query_map(params![pr_id], |r| r.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(surviving_reviews, vec!["PRR_keep".to_string()]);
    }

    #[test]
    fn write_pr_updates_clamps_reply_count_to_zero_on_empty_thread() {
        // Defensive: GraphQL shouldn't surface totalCount = 0 for a populated
        // thread, but guard against negative reply_count if it ever does.
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let detail = detail_with_threads(
            review_threads(vec![empty_thread("PRRT_empty", "x.rs")]),
            None,
            None,
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

        let reply_count: i64 = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT reply_count FROM review_threads WHERE node_id = 'PRRT_empty'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reply_count, 0);
    }

    #[test]
    fn write_pr_updates_writes_issue_comments_count() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let detail = detail_with_threads(
            empty_review_threads(),
            None,
            Some(IssueCommentConnection { total_count: 17 }),
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

        let count: i64 = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT issue_comments_count FROM pull_requests WHERE id = ?1",
                params![pr_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 17);
    }

    #[test]
    fn write_pr_updates_persists_reviews_with_optional_body() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let detail = detail_with_threads(
            empty_review_threads(),
            Some(PullRequestReviewConnection {
                nodes: vec![
                    PullRequestReviewNode {
                        id: "PRR_a".into(),
                        state: "APPROVED".into(),
                        body: Some("LGTM".into()),
                        body_html: None,
                        submitted_at: Some("2026-05-18T12:00:00Z".into()),
                        url: None,
                        author: Some(Actor::new("alice")),
                    },
                    PullRequestReviewNode {
                        id: "PRR_b".into(),
                        state: "COMMENTED".into(),
                        body: None,
                        body_html: None,
                        submitted_at: None,
                        url: None,
                        author: None,
                    },
                ],
            }),
            None,
        );
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();

        type ReviewRow = (String, String, Option<String>, Option<i64>, String);
        let rows: Vec<ReviewRow> = {
            let conn = db.lock().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT node_id, state, body, submitted_at, reviewer_login
                       FROM reviews
                      WHERE pull_request_id = ?1
                      ORDER BY node_id",
                )
                .unwrap();
            stmt.query_map(params![pr_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "PRR_a");
        assert_eq!(rows[0].1, "APPROVED");
        assert_eq!(rows[0].2.as_deref(), Some("LGTM"));
        assert_eq!(rows[0].3, rfc3339_to_unix("2026-05-18T12:00:00Z"));
        assert_eq!(rows[0].4, "alice");
        assert_eq!(rows[1].0, "PRR_b");
        assert_eq!(rows[1].1, "COMMENTED");
        assert!(rows[1].2.is_none());
        assert!(rows[1].3.is_none());
        assert_eq!(rows[1].4, "");
    }

    // ADR 0016 retired the worker's threads rollup UPDATE; the four
    // `threads_*` columns are no longer written or read. The dashboard
    // query computes the buckets at read time; coverage lives in
    // `src-tauri/tests/dashboard_query.rs` and
    // `src-tauri/src/dashboard/query.rs`. The legacy column tests that
    // sat here have been removed alongside the UPDATE.

    // ===== M4-B: mention scan + needs_attention recompute (ADR 0015) =====
    //
    // Each test seeds a relation row directly so the scan + recompute have a
    // target row to update. Comments are inserted by direct SQL because the
    // worker is the one that owns the persistence path for the scan, not the
    // hydrator. The active account's login is `me` (set by `seed_db_with_pr`).

    fn seed_relation(db: &DbHandle, account_id: i64, pr_id: i64) {
        db.lock()
            .unwrap()
            .execute(
                "INSERT INTO pull_request_viewer_relations
                    (account_id, pull_request_id, is_authored, is_review_requested,
                     is_involved, last_seen_at, mentioned_count_unread,
                     mention_scan_watermark_at, needs_attention)
                    VALUES (?1, ?2, 0, 0, 0, 0, 0, 0, 0)",
                params![account_id, pr_id],
            )
            .unwrap();
    }

    fn read_mention_count(db: &DbHandle, account_id: i64, pr_id: i64) -> i64 {
        db.lock()
            .unwrap()
            .query_row(
                "SELECT mentioned_count_unread FROM pull_request_viewer_relations
                  WHERE account_id = ?1 AND pull_request_id = ?2",
                params![account_id, pr_id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
    }

    fn read_watermark(db: &DbHandle, account_id: i64, pr_id: i64) -> i64 {
        db.lock()
            .unwrap()
            .query_row(
                "SELECT mention_scan_watermark_at FROM pull_request_viewer_relations
                  WHERE account_id = ?1 AND pull_request_id = ?2",
                params![account_id, pr_id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
    }

    fn read_needs_attention(db: &DbHandle, account_id: i64, pr_id: i64) -> i64 {
        db.lock()
            .unwrap()
            .query_row(
                "SELECT needs_attention FROM pull_request_viewer_relations
                  WHERE account_id = ?1 AND pull_request_id = ?2",
                params![account_id, pr_id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
    }

    // --- Word-boundary unit tests for the in-memory matcher ---

    #[test]
    fn mentions_viewer_matches_bare_login() {
        assert!(mentions_viewer("hey @alice please review", "alice"));
    }

    #[test]
    fn mentions_viewer_matches_at_end_of_string() {
        assert!(mentions_viewer("ping @alice", "alice"));
    }

    #[test]
    fn mentions_viewer_matches_with_trailing_punctuation() {
        for body in [
            "@alice,", "@alice.", "@alice!", "@alice?", "@alice:", "@alice;",
        ] {
            assert!(mentions_viewer(body, "alice"), "body {body:?} should match");
        }
    }

    #[test]
    fn mentions_viewer_rejects_subword_extension() {
        assert!(!mentions_viewer("ping @alice-bot for help", "alice"));
        assert!(!mentions_viewer("@alicia is here", "alice"));
        assert!(!mentions_viewer("@alice_two reviewed", "alice"));
        assert!(!mentions_viewer("@alice123", "alice"));
    }

    #[test]
    fn mentions_viewer_is_case_insensitive() {
        assert!(mentions_viewer("ping @ALICE today", "alice"));
        assert!(mentions_viewer("ping @alice today", "Alice"));
    }

    #[test]
    fn mentions_viewer_returns_false_on_empty_inputs() {
        assert!(!mentions_viewer("", "alice"));
        assert!(!mentions_viewer("hi @alice", ""));
    }

    #[test]
    fn mentions_viewer_skips_past_unrelated_at_signs() {
        assert!(mentions_viewer(
            "email me at user@example.com or @alice",
            "alice"
        ));
    }

    #[test]
    fn mentions_viewer_handles_at_near_end_without_login() {
        assert!(!mentions_viewer("trailing @", "alice"));
        assert!(!mentions_viewer("trailing @al", "alice"));
    }

    // --- write_pr_updates scan integration tests ---

    #[test]
    fn mention_scan_counts_new_review_comment_mentions() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO review_threads
                    (id, pull_request_id, is_resolved, is_outdated, node_id)
                    VALUES (1001, 100, 0, 0, 'RT_m');
                 INSERT INTO review_comments
                    (id, review_thread_id, author_login, body, created_at)
                    VALUES
                    (2001, 1001, 'bob',   'hey @me what do you think', 10),
                    (2002, 1001, 'carol', 'and @me again',             20);",
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(read_mention_count(&db, 1, pr_id), 2);
    }

    #[test]
    fn mention_scan_counts_new_issue_comment_mentions() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO issue_comments
                    (id, pull_request_id, author_login, body, created_at)
                    VALUES
                    (3001, 100, 'bob',   'looks good @me',             10),
                    (3002, 100, 'carol', 'one more nit, @me, then go', 20);",
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(read_mention_count(&db, 1, pr_id), 2);
    }

    #[test]
    fn mention_scan_ignores_viewers_own_comments() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO issue_comments
                    (id, pull_request_id, author_login, body, created_at)
                    VALUES
                    (3001, 100, 'me',   'I am @me writing about myself', 10),
                    (3002, 100, 'me',   'also @me here',                 20);",
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(
            read_mention_count(&db, 1, pr_id),
            0,
            "viewer's own comments must never increment the counter"
        );
    }

    #[test]
    fn mention_scan_ignores_mentions_of_other_logins() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO issue_comments
                    (id, pull_request_id, author_login, body, created_at)
                    VALUES
                    (3001, 100, 'bob',   '@alice please look',       10),
                    (3002, 100, 'carol', '@dave can you take this?', 20);",
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(read_mention_count(&db, 1, pr_id), 0);
    }

    #[test]
    fn mention_scan_word_boundary_rejects_subword_match() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO issue_comments
                    (id, pull_request_id, author_login, body, created_at)
                    VALUES
                    (3001, 100, 'bob', 'pinging @me-bot for CI',     10),
                    (3002, 100, 'bob', 'and @mester is on holiday',  20),
                    (3003, 100, 'bob', 'true mention: @me now',      30);",
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(
            read_mention_count(&db, 1, pr_id),
            1,
            "only the bare @me row counts"
        );
    }

    #[test]
    fn mention_scan_is_idempotent_across_cycles() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO issue_comments
                    (id, pull_request_id, author_login, body, created_at)
                    VALUES (3001, 100, 'bob', 'hi @me', 10);",
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
        let first = read_mention_count(&db, 1, pr_id);
        assert_eq!(first, 1);

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
        let second = read_mention_count(&db, 1, pr_id);
        assert_eq!(
            second, 1,
            "second cycle with no new comments must not re-count"
        );
    }

    #[test]
    fn mention_scan_advances_watermark_even_without_new_mentions() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);

        // No comments at all.
        assert_eq!(read_watermark(&db, 1, pr_id), 0);

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        let watermark = read_watermark(&db, 1, pr_id);
        assert!(
            watermark > 0,
            "watermark must move forward every cycle (got {watermark})"
        );
    }

    #[test]
    fn mention_scan_only_counts_comments_after_watermark() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);

        // Pin the watermark forward of the older comment so only the newer
        // one is counted on this cycle.
        db.lock()
            .unwrap()
            .execute(
                "UPDATE pull_request_viewer_relations
                    SET mention_scan_watermark_at = 15
                  WHERE account_id = 1 AND pull_request_id = ?1",
                params![pr_id],
            )
            .unwrap();
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO issue_comments
                    (id, pull_request_id, author_login, body, created_at)
                    VALUES
                    (3001, 100, 'bob', 'older @me before watermark', 10),
                    (3002, 100, 'bob', 'newer @me after  watermark', 20);",
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(
            read_mention_count(&db, 1, pr_id),
            1,
            "only the post-watermark comment should count"
        );
    }

    // --- needs_attention recompute tests (four signals, ADR 0015) ---

    #[test]
    fn needs_attention_stays_zero_when_no_signal_fires() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(read_needs_attention(&db, 1, pr_id), 0);
    }

    #[test]
    fn needs_attention_fires_on_unresolved_thread_for_pr_author() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);

        // Make 'me' the PR author and add an unresolved + involved thread.
        db.lock()
            .unwrap()
            .execute_batch(
                "UPDATE pull_requests SET author_login = 'me' WHERE id = 100;
                 INSERT INTO review_threads
                    (id, pull_request_id, is_resolved, is_outdated, node_id)
                    VALUES (1001, 100, 0, 0, 'RT_n');
                 INSERT INTO review_comments
                    (id, review_thread_id, author_login, body, created_at)
                    VALUES (2001, 1001, 'me', 'reply', 10);",
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(read_needs_attention(&db, 1, pr_id), 1);
    }

    #[test]
    fn needs_attention_fires_when_viewer_is_requested_reviewer() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                    VALUES (100, 'me', 'user');",
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(read_needs_attention(&db, 1, pr_id), 1);
    }

    #[test]
    fn needs_attention_fires_on_unread_mention() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO issue_comments
                    (id, pull_request_id, author_login, body, created_at)
                    VALUES (3001, 100, 'bob', 'ping @me when free', 10);",
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(read_mention_count(&db, 1, pr_id), 1);
        assert_eq!(read_needs_attention(&db, 1, pr_id), 1);
    }

    #[test]
    fn needs_attention_fires_on_changes_requested_for_pr_author() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);
        db.lock()
            .unwrap()
            .execute(
                "UPDATE pull_requests
                    SET author_login = 'me',
                        review_decision = 'CHANGES_REQUESTED'
                  WHERE id = ?1",
                params![pr_id],
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(read_needs_attention(&db, 1, pr_id), 1);
    }

    #[test]
    fn needs_attention_does_not_fire_on_changes_requested_for_other_author() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);
        db.lock()
            .unwrap()
            .execute(
                "UPDATE pull_requests
                    SET author_login = 'someone-else',
                        review_decision = 'CHANGES_REQUESTED'
                  WHERE id = ?1",
                params![pr_id],
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(
            read_needs_attention(&db, 1, pr_id),
            0,
            "CHANGES_REQUESTED only matters when the viewer is the author"
        );
    }

    #[test]
    fn needs_attention_clears_when_signal_disappears() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                    VALUES (100, 'me', 'user');",
            )
            .unwrap();
        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
        assert_eq!(read_needs_attention(&db, 1, pr_id), 1);

        db.lock()
            .unwrap()
            .execute("DELETE FROM requested_reviewers", [])
            .unwrap();
        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(
            read_needs_attention(&db, 1, pr_id),
            0,
            "removing the only signal must clear the flag"
        );
    }

    #[test]
    fn sync_cycle_flips_needs_attention_via_new_mention() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        seed_relation(&db, 1, pr_id);

        // Cycle 1: no comments, no signals — flag stays 0.
        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
        assert_eq!(read_needs_attention(&db, 1, pr_id), 0);
        let watermark_after_first = read_watermark(&db, 1, pr_id);
        assert!(watermark_after_first > 0);

        // A new mention lands after the first cycle (created_at > watermark).
        db.lock()
            .unwrap()
            .execute(
                "INSERT INTO issue_comments
                    (id, pull_request_id, author_login, body, created_at)
                    VALUES (3001, ?1, 'bob', 'heads up @me', ?2)",
                params![pr_id, watermark_after_first + 60],
            )
            .unwrap();

        // Cycle 2 picks it up and flips the composite.
        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
        assert_eq!(read_mention_count(&db, 1, pr_id), 1);
        assert_eq!(read_needs_attention(&db, 1, pr_id), 1);
    }

    #[test]
    fn mention_scan_is_a_noop_when_relation_row_missing() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        // Deliberately no `seed_relation` — Team-view path where this account
        // has no discovered relation to the PR.
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO issue_comments
                    (id, pull_request_id, author_login, body, created_at)
                    VALUES (3001, 100, 'bob', 'hi @me', 10);",
            )
            .unwrap();

        // Should not error even with no relation row to update.
        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        let count: i64 = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = ?1",
                params![pr_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "missing relation row must remain missing");
    }

    // --- cross-host (login collision) isolation tests (issue #169) ---
    //
    // Two accounts share login `me` on different hosts. The PR is owned by
    // account 1 (github.com). Without the host-aware joins the recompute
    // would flag account 2 as the PR author / requested reviewer / etc.
    // purely because the login string matches, even though account 2 lives
    // on a different host and isn't the same identity.

    /// Seed a fixture where the PR is owned by account 1 (github.com, login
    /// `me`) and a second account on a different host shares the same login.
    /// Both accounts get a relation row to the same PR so the scan + recompute
    /// can run for either.
    fn seed_db_with_cross_host_login_collision() -> (DbHandle, i64, i64) {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO accounts (id, label, host, login, created_at)
                    VALUES (2, 'ghe', 'github.acme.corp', 'me', 0);",
            )
            .unwrap();
        seed_relation(&db, 1, pr_id);
        seed_relation(&db, 2, pr_id);
        (db, repo_id, pr_id)
    }

    #[test]
    fn needs_attention_does_not_fire_cross_host_for_pr_author_match() {
        let (db, repo_id, pr_id) = seed_db_with_cross_host_login_collision();

        // PR sits on github.com (account 1's host); author_login matches both
        // accounts' login string but the identity is only account 1's. Seed
        // an unresolved + involved thread via a `me`-authored comment so the
        // query-time involvement test would otherwise mark the thread as
        // involved for account 2 too (the EXISTS join is login-only). The
        // host-aware guard around signal #1 must reject the match.
        db.lock()
            .unwrap()
            .execute_batch(
                "UPDATE pull_requests SET author_login = 'me' WHERE id = 100;
                 INSERT INTO review_threads
                    (id, pull_request_id, is_resolved, is_outdated, node_id)
                    VALUES (1001, 100, 0, 0, 'RT_x');
                 INSERT INTO review_comments
                    (id, review_thread_id, author_login, body, created_at)
                    VALUES (2001, 1001, 'me', 'reply', 10);",
            )
            .unwrap();

        write_pr_updates(&db, 2, repo_id, pr_id, None, None).unwrap();

        assert_eq!(
            read_needs_attention(&db, 2, pr_id),
            0,
            "account 2 lives on a different host, so the login-only author \
             match must not flag its needs_attention"
        );
    }

    #[test]
    fn needs_attention_does_not_fire_cross_host_for_requested_reviewer_match() {
        let (db, repo_id, pr_id) = seed_db_with_cross_host_login_collision();

        // Requested reviewer `me` on a github.com PR refers to the github.com
        // user, not account 2's ghe identity.
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO requested_reviewers (pull_request_id, login, reviewer_type)
                    VALUES (100, 'me', 'user');",
            )
            .unwrap();

        write_pr_updates(&db, 2, repo_id, pr_id, None, None).unwrap();

        assert_eq!(
            read_needs_attention(&db, 2, pr_id),
            0,
            "the requested reviewer is on the PR's host; cross-host login \
             match must not flag account 2"
        );
    }

    #[test]
    fn needs_attention_does_not_fire_cross_host_for_changes_requested() {
        let (db, repo_id, pr_id) = seed_db_with_cross_host_login_collision();

        db.lock()
            .unwrap()
            .execute(
                "UPDATE pull_requests
                    SET author_login = 'me',
                        review_decision = 'CHANGES_REQUESTED'
                  WHERE id = ?1",
                params![pr_id],
            )
            .unwrap();

        write_pr_updates(&db, 2, repo_id, pr_id, None, None).unwrap();

        assert_eq!(
            read_needs_attention(&db, 2, pr_id),
            0,
            "CHANGES_REQUESTED on a github.com PR doesn't make account 2 \
             (different host) the author"
        );
    }

    #[test]
    fn needs_attention_still_fires_same_host_for_pr_author_match() {
        // Regression guard: the host-aware join must not break the matching
        // account's recompute. Same fixture, but check the account that IS
        // the PR author still gets needs_attention=1.
        let (db, repo_id, pr_id) = seed_db_with_cross_host_login_collision();

        db.lock()
            .unwrap()
            .execute_batch(
                "UPDATE pull_requests SET author_login = 'me' WHERE id = 100;
                 INSERT INTO review_threads
                    (id, pull_request_id, is_resolved, is_outdated, node_id)
                    VALUES (1001, 100, 0, 0, 'RT_y');
                 INSERT INTO review_comments
                    (id, review_thread_id, author_login, body, created_at)
                    VALUES (2001, 1001, 'me', 'reply', 10);",
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(
            read_needs_attention(&db, 1, pr_id),
            1,
            "account 1 IS the PR author (same host, same login) - must flag"
        );
    }

    #[test]
    fn mention_scan_does_not_increment_cross_host_relation_row() {
        // The same `@me` mention applies to whichever identity matches the
        // PR's host. Account 2 (different host) must not see its mention
        // count climb when only the literal `@me` token matches its login
        // string.
        let (db, repo_id, pr_id) = seed_db_with_cross_host_login_collision();
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO issue_comments
                    (id, pull_request_id, author_login, body, created_at)
                    VALUES (3001, 100, 'bob', 'ping @me', 10);",
            )
            .unwrap();

        write_pr_updates(&db, 2, repo_id, pr_id, None, None).unwrap();

        assert_eq!(
            read_mention_count(&db, 2, pr_id),
            0,
            "cross-host account must not see the github.com mention"
        );
    }

    #[test]
    fn mention_scan_still_increments_same_host_relation_row() {
        // Regression guard for the same fixture: the host-matching account
        // still gets the mention counted.
        let (db, repo_id, pr_id) = seed_db_with_cross_host_login_collision();
        db.lock()
            .unwrap()
            .execute_batch(
                "INSERT INTO issue_comments
                    (id, pull_request_id, author_login, body, created_at)
                    VALUES (3001, 100, 'bob', 'ping @me', 10);",
            )
            .unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();

        assert_eq!(read_mention_count(&db, 1, pr_id), 1);
    }

    // ===== timeline_events persistence tests =====

    use crate::sync::status_timeline::TimelineEvent;
    use time::macros::datetime;

    fn tle(
        kind: &str,
        at: time::OffsetDateTime,
        actor: Option<&str>,
        state: Option<&str>,
    ) -> TimelineEvent {
        TimelineEvent {
            event: kind.into(),
            created_at: at,
            actor_login: actor.map(str::to_string),
            actor_avatar_url: None,
            review_state: state.map(str::to_string),
        }
    }

    fn read_timeline_events(
        db: &DbHandle,
        pr_id: i64,
    ) -> Vec<(String, Option<String>, i64, String)> {
        let conn = db.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT event_type, actor_login, created_at, payload
                   FROM timeline_events
                  WHERE pull_request_id = ?1
                  ORDER BY created_at, id",
            )
            .unwrap();
        stmt.query_map(params![pr_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    #[test]
    fn timeline_event_payload_emits_review_state_for_reviewed_events() {
        let payload = timeline_event_payload(&tle(
            "reviewed",
            datetime!(2026-05-03 10:00:00 UTC),
            Some("bob"),
            Some("APPROVED"),
        ));
        assert_eq!(payload, r#"{"state":"APPROVED"}"#);

        let payload = timeline_event_payload(&tle(
            "ready_for_review",
            datetime!(2026-05-02 14:30:00 UTC),
            Some("alice"),
            None,
        ));
        assert_eq!(payload, "{}");
    }

    #[test]
    fn write_pr_updates_persists_qualifying_timeline_events() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let events = vec![
            tle(
                "ready_for_review",
                datetime!(2026-05-02 14:30:00 UTC),
                Some("alice"),
                None,
            ),
            tle(
                "reviewed",
                datetime!(2026-05-03 10:00:00 UTC),
                Some("bob"),
                Some("APPROVED"),
            ),
            tle(
                "merged",
                datetime!(2026-05-06 11:00:00 UTC),
                Some("alice"),
                None,
            ),
        ];
        write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&events)).unwrap();

        let rows = read_timeline_events(&db, pr_id);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].0, "ready_for_review");
        assert_eq!(rows[0].1.as_deref(), Some("alice"));
        assert_eq!(rows[0].3, "{}");
        assert_eq!(rows[1].0, "reviewed");
        assert_eq!(rows[1].3, r#"{"state":"APPROVED"}"#);
        assert_eq!(rows[2].0, "merged");
    }

    #[test]
    fn write_pr_updates_overwrites_existing_timeline_events_on_rerun() {
        let (db, repo_id, pr_id) = seed_db_with_pr();

        let cycle1 = vec![
            tle(
                "ready_for_review",
                datetime!(2026-05-02 14:30:00 UTC),
                Some("alice"),
                None,
            ),
            tle(
                "reviewed",
                datetime!(2026-05-03 10:00:00 UTC),
                Some("bob"),
                Some("APPROVED"),
            ),
        ];
        write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&cycle1)).unwrap();
        assert_eq!(read_timeline_events(&db, pr_id).len(), 2);

        let cycle2 = vec![
            tle(
                "ready_for_review",
                datetime!(2026-05-02 14:30:00 UTC),
                Some("alice"),
                None,
            ),
            tle(
                "reviewed",
                datetime!(2026-05-03 10:00:00 UTC),
                Some("bob"),
                Some("CHANGES_REQUESTED"),
            ),
            tle(
                "merged",
                datetime!(2026-05-06 11:00:00 UTC),
                Some("alice"),
                None,
            ),
        ];
        write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&cycle2)).unwrap();

        let rows = read_timeline_events(&db, pr_id);
        assert_eq!(rows.len(), 3, "wipe-and-rewrite replaces the whole set");
        // The reviewed event's payload state must reflect the second cycle.
        let reviewed = rows
            .iter()
            .find(|r| r.0 == "reviewed")
            .expect("reviewed event present");
        assert_eq!(reviewed.3, r#"{"state":"CHANGES_REQUESTED"}"#);
    }

    #[test]
    fn write_pr_updates_empty_events_clears_existing_timeline_rows() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let cycle1 = vec![tle(
            "ready_for_review",
            datetime!(2026-05-02 14:30:00 UTC),
            Some("alice"),
            None,
        )];
        write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&cycle1)).unwrap();
        assert_eq!(read_timeline_events(&db, pr_id).len(), 1);

        write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&[])).unwrap();
        assert_eq!(
            read_timeline_events(&db, pr_id).len(),
            0,
            "empty fetch clears the table for this PR"
        );
    }

    #[test]
    fn write_pr_updates_none_events_leaves_existing_timeline_rows_intact() {
        // A 304 from the REST timeline endpoint surfaces as `events: None`;
        // we must not touch the cached rows on that path.
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let cycle1 = vec![tle(
            "ready_for_review",
            datetime!(2026-05-02 14:30:00 UTC),
            Some("alice"),
            None,
        )];
        write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&cycle1)).unwrap();

        write_pr_updates(&db, 1, repo_id, pr_id, None, None).unwrap();
        assert_eq!(
            read_timeline_events(&db, pr_id).len(),
            1,
            "None events => no rewrite, no deletion"
        );
    }

    // ===== users cache (ADR 0013) =====

    fn read_user(db: &DbHandle, login: &str) -> Option<String> {
        db.lock()
            .unwrap()
            .query_row(
                "SELECT avatar_url FROM users WHERE login = ?1",
                params![login],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
    }

    fn detail_with_author_avatar(login: &str, url: &str) -> PullRequestDetail {
        let mut d = detail_with(None, None, None, "MERGEABLE", None, None, None);
        d.author = Some(Actor {
            login: login.into(),
            avatar_url: Some(url.into()),
        });
        d
    }

    #[test]
    fn write_pr_updates_upserts_pr_author_avatar_into_users() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let detail = detail_with_author_avatar("alice", "https://avatars/alice.png");
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();
        assert_eq!(
            read_user(&db, "alice").as_deref(),
            Some("https://avatars/alice.png"),
        );
    }

    #[test]
    fn write_pr_updates_skips_users_upsert_when_avatar_missing() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let detail = detail_with(None, None, None, "MERGEABLE", None, None, None);
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();
        // Author is "alice" (from `detail_with`) with `avatar_url = None`; no
        // users row should land because we never store NULL avatars.
        let count: i64 = db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn write_pr_updates_upserts_thread_head_comment_authors() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let mut detail = detail_with_threads(
            review_threads(vec![thread(
                ThreadSpec::open(
                    "PRRT_1",
                    "src/lib.rs",
                    ("PRRC_1", "bob", "2026-05-18T10:00:00Z"),
                )
                .lines(Some(1), None, None),
            )]),
            None,
            None,
        );
        // Stamp an avatar URL onto the head comment's author so the upsert
        // surfaces a populated row.
        detail.review_threads.nodes[0].comments.nodes[0].author = Some(Actor {
            login: "bob".into(),
            avatar_url: Some("https://avatars/bob.png".into()),
        });
        write_pr_updates(&db, 1, repo_id, pr_id, Some(&detail), None).unwrap();
        assert_eq!(
            read_user(&db, "bob").as_deref(),
            Some("https://avatars/bob.png"),
        );
    }

    #[test]
    fn write_pr_updates_upserts_timeline_actor_avatars() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        let events = vec![TimelineEvent {
            event: "reviewed".into(),
            created_at: datetime!(2026-05-03 10:00:00 UTC),
            actor_login: Some("carol".into()),
            actor_avatar_url: Some("https://avatars/carol.png".into()),
            review_state: Some("APPROVED".into()),
        }];
        write_pr_updates(&db, 1, repo_id, pr_id, None, Some(&events)).unwrap();
        assert_eq!(
            read_user(&db, "carol").as_deref(),
            Some("https://avatars/carol.png"),
        );
    }

    #[test]
    fn write_pr_updates_refreshes_avatar_url_on_change() {
        let (db, repo_id, pr_id) = seed_db_with_pr();
        write_pr_updates(
            &db,
            1,
            repo_id,
            pr_id,
            Some(&detail_with_author_avatar(
                "alice",
                "https://avatars/old.png",
            )),
            None,
        )
        .unwrap();
        write_pr_updates(
            &db,
            1,
            repo_id,
            pr_id,
            Some(&detail_with_author_avatar(
                "alice",
                "https://avatars/new.png",
            )),
            None,
        )
        .unwrap();
        assert_eq!(
            read_user(&db, "alice").as_deref(),
            Some("https://avatars/new.png"),
        );
    }

    // --- SyncRepoError auth routing (issue #236) ---
    //
    // A missing or empty keychain entry surfaces as `GitHubError::Auth(...)`
    // from `attach_auth`, which the worker must route through the same
    // `Unauthorized` path as a 401 so the reauth dialog opens. An OS-level
    // keychain failure (`AuthError::Keychain`) stays on the generic-failure
    // path so a transient libsecret blip doesn't trigger reauth.
    //
    // The mapping uses `SyncRepoError::from_err_for(err, resource)` (issue
    // #235); auth routing is independent of the resource bucket, so the
    // tests pass an arbitrary one.

    use crate::auth::keychain::MockKeychain;
    use crate::auth::token_source::KeychainTokenSource;
    use crate::github::auth::{AccountHandle, AuthError, TokenSource};

    #[test]
    fn sync_repo_error_routes_auth_missing_to_unauthorized() {
        let err = GitHubError::Auth(AuthError::Missing(1));
        let mapped = SyncRepoError::from_err_for(err, RateResource::Core);
        assert!(matches!(mapped, SyncRepoError::Unauthorized));
    }

    #[test]
    fn sync_repo_error_routes_auth_empty_to_unauthorized() {
        let err = GitHubError::Auth(AuthError::Empty(1));
        let mapped = SyncRepoError::from_err_for(err, RateResource::Core);
        assert!(matches!(mapped, SyncRepoError::Unauthorized));
    }

    #[test]
    fn sync_repo_error_keeps_auth_keychain_on_generic_failure_path() {
        let err = GitHubError::Auth(AuthError::Keychain("libsecret unavailable".into()));
        let mapped = SyncRepoError::from_err_for(err, RateResource::Core);
        assert!(matches!(mapped, SyncRepoError::Other(_)));
    }

    #[test]
    fn mock_keychain_none_chains_to_sync_repo_unauthorized() {
        // Reproduces the worker-level discovery flow: `KeychainTokenSource`
        // wrapping a `MockKeychain` that returns `Ok(None)` produces
        // `AuthError::Missing`, which `attach_auth` wraps in
        // `GitHubError::Auth(...)`. The `SyncRepoError` conversion must
        // funnel that into the `Unauthorized` arm.
        let src = KeychainTokenSource::new(MockKeychain::new());
        let handle = AccountHandle::new(1, "github.com", "me");

        let auth_err = src.token(&handle).expect_err("missing keychain entry");
        assert!(matches!(auth_err, AuthError::Missing(1)));

        let github_err = GitHubError::Auth(auth_err);
        let mapped = SyncRepoError::from_err_for(github_err, RateResource::Core);
        assert!(matches!(mapped, SyncRepoError::Unauthorized));
    }
}
