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
    let mut report = SyncCycleReport {
        account_id: account.id,
        repos_visited: 0,
        prs_visited: 0,
        requests_made: 0,
        outcome: CycleOutcome::Completed,
    };

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
        let finished_at = SystemTime::now();
        finish_completed(ctx, account, client, finished_at);
        report.outcome = CycleOutcome::Skipped {
            reason: SkipReason::NoReposConfigured,
        };
        return report;
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

    let finished_at = SystemTime::now();
    finish_completed(ctx, account, client, finished_at);
    finalise_with_budget(report, client, pre_used, pre_remaining)
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
/// Only fields exposed by the v1 schema are updated; everything else is
/// untouched. The status-change derivation (ADR 0007) runs here so the
/// `latest_status_change_*` columns reflect the most recent timeline pull.
pub fn write_pr_updates(
    db: &DbHandle,
    repo_id: i64,
    pr_id: i64,
    detail: Option<&crate::github::graphql::PullRequestDetail>,
    events: Option<&[crate::sync::status_timeline::TimelineEvent]>,
) -> Result<(), rusqlite::Error> {
    let conn = db.lock().expect("db poisoned");

    if let Some(d) = detail {
        let state = if d.merged { "merged" } else { d.state.as_str() };
        let author = d.author.as_ref().map(|a| a.login.as_str()).unwrap_or("");
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
            ],
        )?;
    }

    if let Some(events) = events {
        if let Some(change) = crate::sync::status_timeline::latest_status_change(events) {
            let event_name = qualifying_event_wire_name(change.event_type);
            let at_secs = change.at.unix_timestamp();
            conn.execute(
                "UPDATE pull_requests
                    SET latest_status_change_at = ?1,
                        latest_status_change_event_type = ?2
                  WHERE id = ?3",
                params![at_secs, event_name, pr_id],
            )?;
        }
    }
    Ok(())
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
}
