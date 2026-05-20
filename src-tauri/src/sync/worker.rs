//! Background sync worker — one task per account, isolated.
//!
//! Lifetime: `spawn_worker` returns a [`WorkerHandle`] the Tauri setup hook
//! stashes in app state. The handle owns one [`CancellationToken`] per
//! account and a parent token that cancels every loop on shutdown. Manual
//! refresh is a `tokio::sync::Notify` per account: nudging it makes the loop
//! short-circuit its sleep and run one cycle immediately (subject to the
//! rate-budget guard).
//!
//! Per-account isolation is enforced by `tokio::spawn`'ing a distinct task
//! per account: one task panicking or hanging never touches the others, and
//! the parent token cancels them all on app shutdown.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rusqlite::params;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

use crate::auth::store::{Account, AccountStore};
use crate::db::DbHandle;
use crate::github::auth::TokenSource;
use crate::github::{
    list_pr_timeline, AccountHandle, AccountId, EtagStore, GitHubClient, GitHubError, ListTimeline,
    RepoCoord,
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
        tokio::spawn(account_loop(ctx, account, cancel, refresh))
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
                record_failure(&ctx, &account, &format!("client build: {err}"));
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

/// Whether the current rate-budget snapshot is below the guard threshold.
fn under_guard(remaining: i64, limit: i64, guard_pct: u8) -> bool {
    match rate_remaining_pct(remaining, limit) {
        Some(pct) => pct < guard_pct,
        None => false,
    }
}

/// Run a single sync cycle for one account. Public for integration tests.
pub async fn run_one_cycle(
    ctx: &WorkerContext,
    client: &GitHubClient,
    account: &Account,
) -> SyncCycleReport {
    let snapshot = client.rate().snapshot();
    if under_guard(snapshot.remaining, snapshot.limit, RATE_BUDGET_GUARD_PCT) {
        let pct = rate_remaining_pct(snapshot.remaining, snapshot.limit).unwrap_or(0);
        emit_rate_limit(
            ctx,
            account,
            pct,
            snapshot.limit,
            snapshot.time_until_reset(),
        );
        let state = ctx.state.update(account.id, |s| {
            s.phase = SyncPhase::RateLimited;
            s.message = Some(format!("budget {pct}%, skipping cycle"));
            s.rate_remaining_pct = Some(pct);
            s.rate_limit = Some(snapshot.limit);
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

    let pre_used = snapshot.used.max(0);
    let pre_remaining = snapshot.remaining;
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
    match crate::sync::discovery::discover_account(&ctx.db, client, account.id, cycle_start).await {
        Ok((_, _discovery_report)) => {}
        Err(DiscoveryError::GitHub(GitHubError::Unauthorized)) => {
            let state = ctx.state.update(account.id, |s| {
                s.phase = SyncPhase::Unauthorized;
                s.message = Some("token rejected; reauthenticate".into());
                s.next_sync_in_seconds = None;
            });
            emit_status(&ctx.emit, &state);
            report.outcome = CycleOutcome::Unauthorized;
            return finalise_with_budget(report, client, pre_used, pre_remaining);
        }
        Err(DiscoveryError::GitHub(GitHubError::RateLimited { retry_after })) => {
            let reset_in = retry_after.map(|d| d.as_secs());
            let state = ctx.state.update(account.id, |s| {
                s.phase = SyncPhase::RateLimited;
                s.message = Some("upstream throttled".into());
                s.next_sync_in_seconds = reset_in.or(Some(ctx.config.interval_secs()));
            });
            emit_status(&ctx.emit, &state);
            emit_rate_limit(ctx, account, 0, client.rate().snapshot().limit, retry_after);
            report.outcome = CycleOutcome::RateLimited {
                reset_in_seconds: reset_in,
            };
            return finalise_with_budget(report, client, pre_used, pre_remaining);
        }
        Err(err) => {
            let message = format!("discovery: {err}");
            record_failure(ctx, account, &message);
            report.outcome = CycleOutcome::Failed { message };
            return finalise_with_budget(report, client, pre_used, pre_remaining);
        }
    }

    // Re-read repos after discovery so freshly-upserted rows feed the
    // enrichment loop within the same cycle.
    let repos = match list_repos_for_account(&ctx.db, account.id) {
        Ok(r) => r,
        Err(err) => {
            record_failure(ctx, account, &format!("read repos: {err}"));
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
        let finished_at = SystemTime::now();
        finish_completed(ctx, account, client, finished_at);
        report.outcome = CycleOutcome::Skipped {
            reason: SkipReason::NoReposConfigured,
        };
        return finalise_with_budget(report, client, pre_used, pre_remaining);
    }

    for repo in &repos {
        report.repos_visited += 1;
        match sync_repo(ctx, client, account, repo).await {
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
                report.outcome = CycleOutcome::Unauthorized;
                return finalise_with_budget(report, client, pre_used, pre_remaining);
            }
            Err(SyncRepoError::RateLimited { retry_after }) => {
                let reset_in = retry_after.map(|d| d.as_secs());
                let state = ctx.state.update(account.id, |s| {
                    s.phase = SyncPhase::RateLimited;
                    s.message = Some("upstream throttled".into());
                    s.next_sync_in_seconds = reset_in.or(Some(ctx.config.interval_secs()));
                });
                emit_status(&ctx.emit, &state);
                emit_rate_limit(ctx, account, 0, client.rate().snapshot().limit, retry_after);
                report.outcome = CycleOutcome::RateLimited {
                    reset_in_seconds: reset_in,
                };
                return finalise_with_budget(report, client, pre_used, pre_remaining);
            }
            Err(SyncRepoError::Other(message)) => {
                record_failure(ctx, account, &message);
                report.outcome = CycleOutcome::Failed { message };
                return finalise_with_budget(report, client, pre_used, pre_remaining);
            }
        }
    }

    // Phase final: Pruning. Runs only when enrichment completes so a transient
    // discovery hiccup doesn't drop everything (the contract calls this out).
    if let Err(err) =
        crate::sync::discovery::prune_stale_relations_for_account(&ctx.db, account.id, cycle_start)
    {
        // A prune failure is logged, not fatal: stale rows are merely cosmetic
        // and the next cycle's prune will retry.
        eprintln!("sync prune (account {}): {err}", account.id);
    }

    let finished_at = SystemTime::now();
    finish_completed(ctx, account, client, finished_at);
    finalise_with_budget(report, client, pre_used, pre_remaining)
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

fn finalise_with_budget(
    mut report: SyncCycleReport,
    client: &GitHubClient,
    pre_used: i64,
    pre_remaining: i64,
) -> SyncCycleReport {
    let snap = client.rate().snapshot();
    // Prefer `used` delta; fall back to `remaining` delta if `used` isn't
    // surfaced by Enterprise hosts.
    let by_used = (snap.used.max(0) - pre_used).max(0);
    let by_remaining = (pre_remaining - snap.remaining).max(0);
    let delta = by_used.max(by_remaining);
    report.requests_made = delta as u64;
    report
}

fn emit_rate_limit(
    ctx: &WorkerContext,
    account: &Account,
    pct: u8,
    limit: i64,
    reset_in: Option<Duration>,
) {
    let payload = SyncRateLimitPayload {
        account_id: account.id,
        pct,
        limit: if limit > 0 { Some(limit) } else { None },
        reset_in_seconds: reset_in.map(|d| d.as_secs()),
    };
    ctx.emit.emit(
        SYNC_RATE_LIMIT_EVENT,
        &serde_json::to_value(payload).unwrap_or(serde_json::Value::Null),
    );
}

#[derive(Debug)]
enum SyncRepoError {
    Unauthorized,
    RateLimited { retry_after: Option<Duration> },
    Other(String),
}

impl From<GitHubError> for SyncRepoError {
    fn from(err: GitHubError) -> Self {
        match err {
            GitHubError::Unauthorized => SyncRepoError::Unauthorized,
            GitHubError::RateLimited { retry_after } => SyncRepoError::RateLimited { retry_after },
            other => SyncRepoError::Other(other.to_string()),
        }
    }
}

/// Sync one repo's known PRs. v1 reads PR rows already in the DB; repo
/// discovery lands in M2 (see PR body).
async fn sync_repo(
    ctx: &WorkerContext,
    client: &GitHubClient,
    _account: &Account,
    repo: &RepoRow,
) -> Result<usize, SyncRepoError> {
    let prs = list_prs_for_repo(&ctx.db, repo.id)
        .map_err(|e| SyncRepoError::Other(format!("read prs: {e}")))?;

    let mut visited = 0usize;
    for pr in &prs {
        visited += 1;
        // PR detail (GraphQL) — primary surface per ADR 0006.
        // Wrapped in `timeout` so a hung upstream call doesn't stall the loop.
        let detail = timeout(
            Duration::from_secs(30),
            client.pr_detail(crate::github::graphql::PrCoord {
                owner: &repo.owner,
                name: &repo.name,
                number: pr.number,
            }),
        )
        .await
        .map_err(|_| SyncRepoError::Other(format!("pr_detail timeout for #{}", pr.number)))?
        .map_err(SyncRepoError::from)?;

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
        .map_err(SyncRepoError::from)?;

        let events = match timeline {
            ListTimeline::Events(e) => Some(e),
            ListTimeline::NotModified => None,
        };

        // Persist whatever new data we have.
        write_pr_updates(&ctx.db, repo.id, pr.id, detail.as_ref(), events.as_deref())
            .map_err(|e| SyncRepoError::Other(format!("persist PR #{}: {e}", pr.number)))?;
    }
    Ok(visited)
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
}

pub fn list_repos_for_account(
    db: &DbHandle,
    account_id: AccountId,
) -> Result<Vec<RepoRow>, rusqlite::Error> {
    let conn = db.lock().expect("db poisoned");
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
    let conn = db.lock().expect("db poisoned");
    let mut stmt = conn.prepare("SELECT id, number FROM pull_requests WHERE repo_id = ?1")?;
    let rows = stmt
        .query_map(params![repo_id], |row| {
            Ok(PrRow {
                id: row.get(0)?,
                number: row.get(1)?,
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
pub fn write_pr_updates(
    db: &DbHandle,
    repo_id: i64,
    pr_id: i64,
    detail: Option<&crate::github::graphql::PullRequestDetail>,
    events: Option<&[crate::sync::status_timeline::TimelineEvent]>,
) -> Result<(), rusqlite::Error> {
    let mut conn = db.lock().expect("db poisoned");
    let tx = conn.transaction()?;

    if let Some(d) = detail {
        let state = if d.merged { "merged" } else { d.state.as_str() };
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
    }
    tx.commit()
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
        RequestedReviewer::User { login } => Some(("user", login.as_str())),
        RequestedReviewer::Team { slug } => Some(("team", slug.as_str())),
        RequestedReviewer::Other => None,
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

    #[test]
    fn under_guard_fires_below_threshold() {
        // 19% < 20% guard → fires.
        assert!(under_guard(999, 5000, 20));
        // 20% == guard → does not fire (threshold is "below").
        assert!(!under_guard(1000, 5000, 20));
        // Unobserved → no skip.
        assert!(!under_guard(-1, -1, 20));
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
            author: Some(Actor {
                login: "alice".into(),
            }),
            base_ref_name: "main".into(),
            head_ref_name: "feat/thing".into(),
            review_decision: review_decision.map(str::to_string),
            additions,
            deletions,
            changed_files,
            review_requests,
            commits,
            review_threads: empty_review_threads(),
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

        write_pr_updates(&db, repo_id, pr_id, Some(&detail), None).unwrap();

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

        write_pr_updates(&db, repo_id, pr_id, Some(&detail), None).unwrap();

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
        write_pr_updates(&db, repo_id, pr_id, Some(&detail), None).unwrap();

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
        write_pr_updates(&db, repo_id, pr_id, Some(&detail), None).unwrap();

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

        write_pr_updates(&db, repo_id, pr_id, Some(&detail), None).unwrap();

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
                        }),
                    },
                ],
            }),
            None,
        );
        write_pr_updates(&db, repo_id, pr_id, Some(&detail), None).unwrap();

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
}
