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
use std::time::Duration;

use tauri::async_runtime::JoinHandle;
use tokio::sync::Notify;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::auth::store::{Account, AccountStore};
use crate::db::DbHandle;
use crate::github::auth::TokenSource;
use crate::github::{AccountHandle, AccountId, EtagStore, GitHubClient, GitHubError};
use crate::notify::{BadgeSink, NotificationSinkHandle};
use crate::sync::activity::ActivityBuffer;
use crate::sync::events::{
    SyncErrorPayload, SyncStatusPayload, SYNC_ERROR_EVENT, SYNC_STATUS_EVENT,
};
use crate::sync::scheduler::SchedulerConfig;
use crate::sync::state::{seconds_floor, AccountSyncState, SyncPhase, SyncStateMap};

mod cycle;
mod dispatch;
mod enrichment;
mod triage_recompute;

pub use cycle::{list_prs_for_repo, list_repos_for_account, run_one_cycle, PrRow, RepoRow};
pub use enrichment::write_pr_updates;

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
    RateBudgetGuard { rate_remaining_pct: u8 },
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
            // Logged, not propagated - a failed emission must not stall the
            // sync loop (the next tick will publish a fresh status anyway).
            tracing::warn!(event, %err, "sync emit failed");
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
            tracing::error!(%err, "sync worker: failed to list accounts on startup");
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

    // Seed the state map so the UI shows a baseline immediately. Manual
    // mode (interval == 0) leaves `next_sync_in_seconds` as `None` so the
    // status-bar hides the "next in" chip until a manual refresh runs.
    let initial = ctx.state.update(account_id, |s| {
        s.phase = SyncPhase::Idle;
        s.next_sync_in_seconds = next_sync_hint(&ctx, None);
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
                cycle::emit_activity_cycle_failed(&ctx, &account, "client_build", &err.to_string());
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
                // Honour the upstream reset hint if we have one. Without a
                // hint, fall back to the configured interval — or park
                // entirely in Manual mode (interval == 0) since there's no
                // auto cadence to wait on.
                match reset_in_seconds {
                    Some(secs) => {
                        if !sleep_or_refresh(&cancel, &refresh, Duration::from_secs(*secs)).await {
                            return;
                        }
                    }
                    None if ctx.config.is_manual() => {
                        if !park_until_refresh(&cancel, &refresh).await {
                            return;
                        }
                    }
                    None => {
                        if !sleep_or_refresh(&cancel, &refresh, ctx.config.interval()).await {
                            return;
                        }
                    }
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
/// once at the start so the UI countdown is anchored. In Manual mode
/// (`interval_secs == 0`) the loop parks until an explicit refresh nudge or
/// cancellation; `next_sync_in_seconds` is cleared so the UI hides the
/// "next in" chip.
async fn wait_for_next(
    ctx: &WorkerContext,
    account_id: AccountId,
    cancel: &CancellationToken,
    refresh: &Arc<Notify>,
) {
    if ctx.config.is_manual() {
        let next_state = ctx.state.update(account_id, |s| {
            s.next_sync_in_seconds = None;
        });
        emit_status(&ctx.emit, &next_state);
        let _ = park_until_refresh(cancel, refresh).await;
        return;
    }

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

/// Park indefinitely until cancelled or nudged via `refresh.notify_one()`.
/// Used in Manual mode (issue #358) where there's no auto cadence to drive
/// the next cycle. Returns `false` on cancellation, `true` on refresh.
async fn park_until_refresh(cancel: &CancellationToken, refresh: &Arc<Notify>) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = refresh.notified() => true,
    }
}

/// Convert the configured interval into the `next_sync_in_seconds` value
/// surfaced to the frontend. Manual mode (interval == 0) returns `None` so
/// the status-bar hides the "next in" chip; auto mode returns the configured
/// interval. Callers may pass `override_seconds` (e.g. an upstream rate-reset
/// hint) which always wins when set.
pub(super) fn next_sync_hint(ctx: &WorkerContext, override_seconds: Option<u64>) -> Option<u64> {
    if let Some(secs) = override_seconds {
        return Some(secs);
    }
    if ctx.config.is_manual() {
        None
    } else {
        Some(ctx.config.interval_secs())
    }
}

pub(super) fn emit_status(emit: &Arc<dyn EmitSink>, state: &AccountSyncState) {
    let payload = SyncStatusPayload::new(state.clone());
    emit.emit(
        SYNC_STATUS_EVENT,
        &serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null),
    );
}

pub(super) fn record_failure(ctx: &WorkerContext, account: &Account, message: &str) {
    let state = ctx.state.update(account.id, |s| {
        s.phase = SyncPhase::Error;
        s.message = Some(short_error_message(message));
        s.next_sync_in_seconds = next_sync_hint(ctx, None);
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

pub(super) fn short_error_message(raw: &str) -> String {
    const MAX: usize = 160;
    if raw.len() <= MAX {
        raw.to_string()
    } else {
        format!("{}…", &raw[..MAX])
    }
}

pub(super) fn unix_now() -> i64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub(super) fn rfc3339_to_unix(s: &str) -> Option<i64> {
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
